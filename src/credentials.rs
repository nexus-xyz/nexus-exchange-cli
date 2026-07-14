//! Credential & config resolution: flags / env (handled by clap) layered over
//! an on-disk config written by `nexus setup`.
//!
//! Precedence, highest first: command-line flag, environment variable, config
//! file, built-in default. Flags and env are merged by clap; this module adds
//! the file layer underneath and owns the interactive `setup` flow.
//!
//! The config file holds an API secret and a wallet session token, so it is
//! created `0600` (owner read/write only) inside a `0700` directory, and the
//! secret is never echoed while typing or printed back out. Writes are atomic
//! (temp file + `rename`) so a concurrent reader or a crash mid-write can never
//! observe — or be left with — a truncated, secret-losing config.
//!
//! There are two credential paths, both persisted here:
//!   - the HMAC API key/secret pair (`api_key` + `api_secret`), used to sign
//!     every authenticated request, and
//!   - a wallet **session token** (`session_token`) minted by `nexus auth
//!     login` (EIP-191 sign-in). It authenticates session-scoped routes and is
//!     written with the same `0600` perms and precedence idiom as the secret.

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Persisted configuration. Every field is optional: the file is purely a
/// fallback layer beneath flags and environment variables.
///
/// `Debug` is implemented by hand so the secret never lands in logs.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct FileConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_secret: Option<String>,
    /// Wallet session token minted by `nexus auth login` (EIP-191 sign-in).
    /// Used to authenticate session-scoped routes; never echoed or printed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
}

impl std::fmt::Debug for FileConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileConfig")
            .field("network", &self.network)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key)
            .field(
                "api_secret",
                &self.api_secret.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Location of the config file: `$XDG_CONFIG_HOME/nexus/config.json`, falling
/// back to `$HOME/.config/nexus/config.json`.
pub fn config_path() -> Result<PathBuf> {
    let dir = if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        PathBuf::from(xdg)
    } else {
        let home = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .context("cannot locate config directory: neither $XDG_CONFIG_HOME nor $HOME is set")?;
        PathBuf::from(home).join(".config")
    };
    Ok(dir.join("nexus").join("config.json"))
}

/// Load the config file if it exists. A missing file is `Ok(None)`; a malformed
/// file is an error so the user finds out rather than silently losing settings.
pub fn load() -> Result<Option<FileConfig>> {
    let path = config_path()?;
    match std::fs::read(&path) {
        Ok(bytes) => {
            let cfg = serde_json::from_slice(&bytes)
                .with_context(|| format!("config file at {} is not valid JSON", path.display()))?;
            Ok(Some(cfg))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Write the config file with owner-only permissions.
pub fn save(cfg: &FileConfig) -> Result<PathBuf> {
    let path = config_path()?;
    let dir = path.parent().expect("config path always has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    harden_dir(dir)?;

    let json = serde_json::to_string_pretty(cfg).expect("FileConfig is always serializable");
    write_private_atomic(&path, json.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

/// Persist a wallet session token (from `nexus auth login`) into the config
/// file, preserving every other field. Loads the existing config, overwrites
/// only `session_token`, and rewrites the file with the same `0600` perms as
/// the API secret. Returns the path it was written to.
pub fn save_session_token(token: &str) -> Result<PathBuf> {
    let mut cfg = load()?.unwrap_or_default();
    cfg.session_token = Some(token.to_string());
    save(&cfg)
}

/// Atomically write `bytes` to `path` with owner-only permissions (`0600`).
///
/// The bytes are written to a fresh sibling temp file (created `0600`), flushed
/// to disk, then `rename`d over `path`. Because a same-directory rename is
/// atomic on POSIX, this closes three hazards on the credential file (which
/// holds the API secret and the wallet session token):
///
///   - **Torn reads.** Another `nexus` process calling [`load`] while `auth
///     login`/`setup` writes always sees either the complete old file or the
///     complete new one — never the empty/partial file a plain truncate-then-
///     write briefly exposes (which would surface as a spurious "not valid
///     JSON" failure on an authenticated command).
///   - **Crash corruption.** A crash/interrupt mid-write leaves the untouched
///     old file in place rather than a truncated one, so a stored secret/token
///     can't be silently lost.
///   - **Interleaved writers.** Two concurrent writers each stage their own
///     uniquely-named temp file, so they resolve to a clean last-writer-wins at
///     file granularity instead of interleaving into a corrupt file.
///
/// The temp file gets a unique name from two parts: the **pid** keeps
/// concurrent writers in *different* processes apart (the counter resets to 0
/// each run, so pid is what guarantees cross-process uniqueness), and a
/// **process-local counter** keeps concurrent writers *within* one process
/// apart. Either way no two concurrent writers share a temp file, and it is
/// removed on any failure so a partial temp file is not left behind.
fn write_private_atomic(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

    let dir = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "config path has no parent directory",
        )
    })?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config.json");
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{file_name}.tmp.{}.{seq}", std::process::id()));

    // Stage the full contents in the temp file (created 0600, flushed to disk),
    // cleaning it up if anything fails so no partial temp file lingers.
    if let Err(e) = write_private(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // Atomically swap it into place; drop the temp file if the rename fails.
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // Best-effort durability (Unix only): flush the directory entry so the
    // rename survives a crash. Non-fatal, and the crash-durability guarantee is
    // therefore Unix-bound — but the rename is already atomic for concurrent
    // readers on every platform, so a torn read can't happen anywhere.
    #[cfg(unix)]
    if let Ok(dir_handle) = std::fs::File::open(dir) {
        let _ = dir_handle.sync_all();
    }
    Ok(())
}

/// Write `bytes` to `path`, ensuring the file is owner-read/write only (`0600`).
fn write_private(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    use std::fs::OpenOptions;
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    // `mode()` only applies on *creation*; tighten an existing file too.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(bytes)?;
    f.sync_all()
}

/// Best-effort tightening of the config directory to `0700`.
fn harden_dir(_dir: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Only tighten if it is too open; ignore if we don't own it.
        if let Ok(meta) = std::fs::metadata(_dir) {
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                let _ = std::fs::set_permissions(_dir, std::fs::Permissions::from_mode(0o700));
            }
        }
    }
    Ok(())
}

/// Interactive `nexus setup`: prompt for network and credentials, then persist
/// them. Refuses to run unless stdin is a terminal — there is nothing to read
/// from a pipe, and silently writing an empty config would be surprising.
pub fn setup() -> Result<()> {
    if !io::stdin().is_terminal() {
        anyhow::bail!("`nexus setup` is interactive; run it from a terminal");
    }

    println!("Configure the Nexus Exchange CLI. Press Enter to accept the default.\n");

    let existing = load()?.unwrap_or_default();

    let network = prompt_line(
        "Network [stable/beta/local]",
        existing.network.as_deref().or(Some("stable")),
    )?;

    let api_key = prompt_line("API key id (nx_...)", existing.api_key.as_deref())?;

    // Read the secret without echoing it to the terminal.
    let api_secret = rpassword::prompt_password("API secret (input hidden, blank keeps current): ")
        .context("failed to read API secret")?;

    let mut cfg = FileConfig {
        network: non_empty(network),
        api_key: non_empty(api_key),
        // Keep an existing secret if the user left the prompt blank.
        api_secret: non_empty(api_secret).or(existing.api_secret),
        base_url: existing.base_url,
        // `setup` doesn't touch the wallet session token; preserve it.
        session_token: existing.session_token,
    };
    // Normalize away an all-blank network so it doesn't shadow the default.
    if cfg.network.as_deref() == Some("") {
        cfg.network = None;
    }

    let path = save(&cfg)?;
    println!("\nSaved to {} (permissions 0600).", path.display());
    if cfg.api_secret.is_none() {
        println!("note: no API secret stored — authenticated commands will be refused.");
    }
    Ok(())
}

/// Prompt for a single line, showing the default in brackets. Returns the
/// default when the user just presses Enter.
fn prompt_line(label: &str, default: Option<&str>) -> Result<String> {
    match default {
        Some(d) => print!("{label} [{d}]: "),
        None => print!("{label}: "),
    }
    io::stdout().flush().ok();

    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("failed to read input")?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        Ok(default.unwrap_or("").to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

fn non_empty(s: String) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

// `IsTerminal` is in the prelude on the MSRV (1.82), but import it explicitly to
// be unambiguous.
use std::io::IsTerminal;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `config_path` reads process-global env (`XDG_CONFIG_HOME` / `HOME`), and
    // the tests mutate it, so they must not run concurrently.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Point the config dir at a fresh temp directory for the duration of a
    /// test, restoring the previous env afterward.
    struct TempConfigHome {
        dir: PathBuf,
        prev_xdg: Option<std::ffi::OsString>,
        prev_home: Option<std::ffi::OsString>,
    }

    impl TempConfigHome {
        fn new(tag: &str) -> Self {
            let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
            let prev_home = std::env::var_os("HOME");
            let dir = std::env::temp_dir().join(format!(
                "nexus-cli-test-{}-{}-{:?}",
                std::process::id(),
                tag,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            std::env::set_var("XDG_CONFIG_HOME", &dir);
            Self {
                dir,
                prev_xdg,
                prev_home,
            }
        }
    }

    impl Drop for TempConfigHome {
        fn drop(&mut self) {
            match &self.prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match &self.prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn config_path_prefers_xdg_then_home() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("path-xdg");
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/xdg-example");
        let p = config_path().unwrap();
        assert_eq!(p, PathBuf::from("/tmp/xdg-example/nexus/config.json"));

        // With XDG unset, fall back to $HOME/.config.
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::set_var("HOME", "/home/someone");
        let p = config_path().unwrap();
        assert_eq!(p, PathBuf::from("/home/someone/.config/nexus/config.json"));
    }

    #[test]
    fn config_path_errors_without_xdg_or_home() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_home = std::env::var_os("HOME");
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("HOME");
        let err = config_path().unwrap_err();
        assert!(err.to_string().contains("config directory"));
        // Restore.
        if let Some(v) = prev_xdg {
            std::env::set_var("XDG_CONFIG_HOME", v);
        }
        if let Some(v) = prev_home {
            std::env::set_var("HOME", v);
        }
    }

    #[test]
    fn load_missing_file_is_none() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("missing");
        // Nothing written yet.
        assert!(load().unwrap().is_none());
    }

    #[test]
    fn save_then_load_round_trips_and_skips_none_fields() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("roundtrip");
        let cfg = FileConfig {
            network: Some("beta".into()),
            base_url: None,
            api_key: Some("nx_abc".into()),
            api_secret: Some("shh".into()),
            session_token: None,
        };
        let path = save(&cfg).unwrap();
        assert!(path.exists());

        // Round-trips through disk.
        let loaded = load().unwrap().expect("config should be present");
        assert_eq!(loaded.network.as_deref(), Some("beta"));
        assert_eq!(loaded.api_key.as_deref(), Some("nx_abc"));
        assert_eq!(loaded.api_secret.as_deref(), Some("shh"));
        assert_eq!(loaded.base_url, None);

        // `None` fields are omitted from the serialized JSON.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("base_url"),
            "None field should be skipped: {raw}"
        );
        assert!(raw.contains("api_key"));
    }

    #[cfg(unix)]
    #[test]
    fn save_writes_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("perms");
        let path = save(&FileConfig {
            api_key: Some("k".into()),
            api_secret: Some("s".into()),
            ..Default::default()
        })
        .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config file must be 0600, was {mode:o}");
    }

    #[test]
    fn load_malformed_json_is_an_error() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("malformed");
        let path = config_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ this is not json").unwrap();
        let err = load().unwrap_err();
        assert!(err.to_string().contains("not valid JSON"));
    }

    #[test]
    fn non_empty_trims_and_nullifies_blank() {
        assert_eq!(non_empty("  hi  ".into()), Some("hi".to_string()));
        assert_eq!(non_empty("   ".into()), None);
        assert_eq!(non_empty("".into()), None);
    }

    #[test]
    fn debug_redacts_the_secret() {
        let cfg = FileConfig {
            api_secret: Some("topsecret".into()),
            api_key: Some("nx_visible".into()),
            ..Default::default()
        };
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("topsecret"), "secret leaked: {dbg}");
        assert!(dbg.contains("<redacted>"));
        assert!(dbg.contains("nx_visible"));
    }

    #[test]
    fn debug_redacts_session_token() {
        let cfg = FileConfig {
            session_token: Some("super-secret-token".into()),
            api_secret: Some("super-secret-secret".into()),
            api_key: Some("nx_visible".into()),
            ..Default::default()
        };
        let dbg = format!("{cfg:?}");
        assert!(
            !dbg.contains("super-secret-token"),
            "session token leaked via Debug: {dbg}"
        );
        assert!(!dbg.contains("super-secret-secret"));
        assert!(dbg.contains("nx_visible"));
        assert!(dbg.contains("<redacted>"));
    }

    /// The persisted JSON must never name a token field unless one is set, and
    /// must use the stable `session_token` key when it is.
    #[test]
    fn session_token_serializes_under_stable_key() {
        let empty = FileConfig::default();
        let json = serde_json::to_string(&empty).unwrap();
        assert!(!json.contains("session_token"), "empty config: {json}");

        let cfg = FileConfig {
            session_token: Some("tok".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"session_token\":\"tok\""), "got {json}");
    }

    #[test]
    fn session_token_round_trips_through_the_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("session-roundtrip");
        // A fresh config has no token.
        assert!(load().unwrap().is_none());

        let path = save_session_token("sess_tok_abc123").unwrap();
        let loaded = load().unwrap().expect("config should exist after save");
        assert_eq!(loaded.session_token.as_deref(), Some("sess_tok_abc123"));

        // Re-saving overwrites only the token, preserving other fields.
        let mut cfg = loaded;
        cfg.api_key = Some("nx_key".into());
        cfg.api_secret = Some("secret".into());
        save(&cfg).unwrap();
        let again = save_session_token("sess_tok_xyz789").unwrap();
        assert_eq!(again, path);
        let loaded = load().unwrap().unwrap();
        assert_eq!(loaded.session_token.as_deref(), Some("sess_tok_xyz789"));
        assert_eq!(loaded.api_key.as_deref(), Some("nx_key"));
        assert_eq!(loaded.api_secret.as_deref(), Some("secret"));
    }

    /// An atomic write leaves only the config file behind — no `.tmp` sibling.
    #[test]
    fn save_leaves_no_temp_file_behind() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("no-temp");
        let path = save_session_token("tok").unwrap();
        let dir = path.parent().unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp file(s) left behind: {leftovers:?}"
        );
    }

    /// Concurrent writers and a reader must never see a torn/partial config: the
    /// temp-file + atomic-rename write path guarantees every `load()` observes a
    /// complete, parseable file (the old one or a new one), never a truncated
    /// one. A plain truncate-then-write would intermittently fail the reader
    /// with "not valid JSON".
    #[test]
    fn concurrent_saves_never_produce_a_torn_read() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("concurrent");
        // Seed a valid file so the reader always has something to parse.
        save_session_token("seed").unwrap();

        let writers: Vec<_> = (0..4)
            .map(|i| {
                std::thread::spawn(move || {
                    for n in 0..40 {
                        save_session_token(&format!("tok-{i}-{n}")).unwrap();
                    }
                })
            })
            .collect();
        let reader = std::thread::spawn(|| {
            for _ in 0..400 {
                // A torn/truncated file would make this parse fail.
                load().expect("config must never be observed torn/partial");
            }
        });

        for w in writers {
            w.join().unwrap();
        }
        reader.join().unwrap();

        // A valid config remains and no temp files linger.
        let cfg = load().unwrap().expect("config should be present");
        assert!(cfg.session_token.is_some());
        let dir = config_path().unwrap().parent().unwrap().to_path_buf();
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(
            leftovers, 0,
            "temp files left behind after concurrent saves"
        );
    }

    #[test]
    #[cfg(unix)]
    fn session_token_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("session-perm");
        let path = save_session_token("sess_tok_perm").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "session-token file must be 0600");
    }
}
