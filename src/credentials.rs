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
//!
//! Both are stored **per network** (ENG-6462), under a `networks` map keyed by
//! network name. A key minted on one network is invalid on any other — the
//! indexer enforces that server-side (ENG-6443) — so a single flat credential
//! slot could only ever hold one network's key, and pointing it at another
//! network produces an auth failure with no hint as to why. Namespacing also
//! means a testnet key is never *offered* to a real-funds invocation: the
//! sections are selected by the resolved network, so `--network mainnet` reads
//! the mainnet section or nothing at all.
//!
//! Configs written before namespacing hold the credentials at the top level.
//! Those are folded into the section for the network the file itself names (see
//! [`FileConfig::migrate_legacy_credentials`]) — in memory on every [`load`], so
//! an untouched file keeps working, and on disk the next time anything writes.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cli::NetworkArg;

/// The credentials that authenticate against one network.
///
/// `Debug` is implemented by hand so neither secret lands in logs.
#[derive(Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkCredentials {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_secret: Option<String>,
    /// Wallet session token minted by `nexus auth login` (EIP-191 sign-in).
    /// Used to authenticate session-scoped routes; never echoed or printed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
}

impl NetworkCredentials {
    /// Whether this section holds nothing at all. An empty section is dropped
    /// rather than written, so the file never grows a `"mainnet": {}` stub from
    /// a `setup` run the user abandoned.
    fn is_empty(&self) -> bool {
        self.api_key.is_none() && self.api_secret.is_none() && self.session_token.is_none()
    }
}

impl std::fmt::Debug for NetworkCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkCredentials")
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

    /// Credentials keyed by network name (`testnet`, `mainnet`, `local`).
    ///
    /// `BTreeMap` rather than a struct with one field per network: the key space
    /// is the *file's*, not this build's, so a section for a network this binary
    /// does not know survives a round-trip instead of being silently dropped on
    /// the next write. It is also ordered, so the file's diff is stable.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub networks: BTreeMap<String, NetworkCredentials>,

    /// Whether the user has acknowledged that mainnet moves real funds. Set by
    /// the one-time prompt on the first mainnet trade (ENG-6462).
    #[serde(default, skip_serializing_if = "is_false")]
    pub mainnet_acknowledged: bool,

    // ── pre-namespacing layout (ENG-6462) ──
    //
    // Read so an existing config keeps authenticating, folded into `networks` by
    // `migrate_legacy_credentials`, and never written back: `load` clears them,
    // so the next `save` drops them from the file. Public only because the
    // migration and its tests live outside this struct's own impl.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
}

/// `skip_serializing_if` predicate for a `bool` — serde hands the field by
/// reference, which `std::ops::Not::not` does not take.
fn is_false(b: &bool) -> bool {
    !*b
}

impl FileConfig {
    /// The credentials stored for `network`, if any.
    pub fn credentials_for(&self, network: NetworkArg) -> Option<&NetworkCredentials> {
        self.networks.get(network.as_str())
    }

    /// The credential section for `network`, creating it if absent.
    pub fn section_mut(&mut self, network: NetworkArg) -> &mut NetworkCredentials {
        self.networks
            .entry(network.as_str().to_string())
            .or_default()
    }

    /// Drop any section that ended up holding nothing.
    fn prune_empty_sections(&mut self) {
        self.networks.retain(|_, creds| !creds.is_empty());
    }

    /// Fold pre-namespacing top-level credentials into the section for `owner`,
    /// clearing them so the next write emits only the namespaced layout.
    ///
    /// `owner` is the network the *file* names, never the one `--network`
    /// selects. That is the whole safety property: a flat config was written by
    /// a `setup` run against one network, so its key belongs to that network and
    /// must not be handed to `--network mainnet` just because the invocation
    /// asked for mainnet. A file naming no network — or a name this build cannot
    /// parse — resolves to the same default the requests themselves land on, so
    /// the credentials follow the traffic.
    ///
    /// An existing section wins over the legacy field, per-field: the namespaced
    /// value is the newer, more specific statement, and a half-migrated file
    /// (say, a `mainnet` section added by hand next to an old flat testnet key)
    /// must not have the stale value overwrite it.
    pub fn migrate_legacy_credentials(&mut self, owner: NetworkArg) {
        let legacy = NetworkCredentials {
            api_key: self.api_key.take(),
            api_secret: self.api_secret.take(),
            session_token: self.session_token.take(),
        };
        if legacy.is_empty() {
            return;
        }
        let section = self.section_mut(owner);
        section.api_key = section.api_key.take().or(legacy.api_key);
        section.api_secret = section.api_secret.take().or(legacy.api_secret);
        section.session_token = section.session_token.take().or(legacy.session_token);
    }
}

impl std::fmt::Debug for FileConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileConfig")
            .field("network", &self.network)
            .field("base_url", &self.base_url)
            .field("networks", &self.networks)
            .field("mainnet_acknowledged", &self.mainnet_acknowledged)
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
///
/// Pre-namespacing credentials are migrated in memory on the way out, so every
/// caller sees one layout regardless of what is on disk.
pub fn load() -> Result<Option<FileConfig>> {
    let path = config_path()?;
    match std::fs::read(&path) {
        Ok(bytes) => {
            let mut cfg: FileConfig = serde_json::from_slice(&bytes)
                .with_context(|| format!("config file at {} is not valid JSON", path.display()))?;
            cfg.migrate_legacy_credentials(legacy_credential_owner(&cfg));
            Ok(Some(cfg))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Which network a pre-namespacing config's flat credentials belong to: the one
/// the file names, or the default when it names none or names something this
/// build cannot parse. Both fallbacks match where [`Cli::config`] actually sends
/// the requests, so the credentials and the traffic stay on the same network.
///
/// [`Cli::config`]: crate::cli::Cli::config
fn legacy_credential_owner(cfg: &FileConfig) -> NetworkArg {
    cfg.network
        .as_deref()
        .and_then(NetworkArg::parse)
        .unwrap_or(crate::cli::DEFAULT_NETWORK)
}

/// Write the config file with owner-only permissions.
pub fn save(cfg: &FileConfig) -> Result<PathBuf> {
    let path = config_path()?;
    let dir = path.parent().expect("config path always has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    harden_dir(dir)?;

    let mut cfg = cfg.clone();
    cfg.prune_empty_sections();
    let json = serde_json::to_string_pretty(&cfg).expect("FileConfig is always serializable");
    write_private_atomic(&path, json.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

/// Persist a wallet session token (from `nexus auth login`) into `network`'s
/// section, preserving every other field. Loads the existing config, overwrites
/// only that network's `session_token`, and rewrites the file with the same
/// `0600` perms as the API secret. Returns the path it was written to.
///
/// Scoped to the network the login ran against: a session token is minted by a
/// signature against one network's indexer and is no more portable than an API
/// key, so storing it flat would leave `--network mainnet` presenting a testnet
/// token.
pub fn save_session_token(network: NetworkArg, token: &str) -> Result<PathBuf> {
    let mut cfg = load()?.unwrap_or_default();
    cfg.section_mut(network).session_token = Some(token.to_string());
    save(&cfg)
}

/// Record that the user has acknowledged mainnet's real-funds warning, so the
/// one-time prompt does not reappear on every trade. Preserves every other
/// field. Returns the path it was written to.
pub fn save_mainnet_acknowledged() -> Result<PathBuf> {
    let mut cfg = load()?.unwrap_or_default();
    cfg.mainnet_acknowledged = true;
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

    // Never offer a stored value back as the default unless it is still a real
    // network: a config written before the release-channel rename holds
    // `stable`/`beta`, and pre-filling that would have the user press Enter and
    // re-save a name that no longer exists.
    let network = prompt_line(
        "Network [mainnet/testnet/local]",
        existing
            .network
            .as_deref()
            .filter(|n| NetworkArg::parse(n).is_some())
            .or(Some("testnet")),
    )?;
    // Validated before the credential prompts so a typo costs nothing — the
    // secret has not been typed yet. Writing an unusable network would push the
    // failure to every later invocation instead of surfacing it here.
    let network = non_empty(network);
    if let Some(name) = network.as_deref() {
        validate_network_name(name)?;
    }

    // The credentials being typed belong to the network just chosen, not to
    // whatever the file named before: `setup` is how you add a second network's
    // key, so re-running it with a different answer must not overwrite the first.
    // A blank answer means "no preference", which lands on the default.
    let target = network
        .as_deref()
        .and_then(NetworkArg::parse)
        .unwrap_or(crate::cli::DEFAULT_NETWORK);
    if target.is_real_funds() {
        println!(
            "\n{}\n",
            real_funds_notice("These credentials will be stored for a real-funds network.")
        );
    }

    let stored = existing
        .credentials_for(target)
        .cloned()
        .unwrap_or_default();
    println!(
        "\nCredentials for {} (each network needs its own key — a key is valid on \
         exactly one network).",
        target.as_str()
    );

    let api_key = prompt_line("API key id (nx_...)", stored.api_key.as_deref())?;

    // Read the secret without echoing it to the terminal.
    let api_secret = rpassword::prompt_password("API secret (input hidden, blank keeps current): ")
        .context("failed to read API secret")?;

    let mut cfg = FileConfig {
        network,
        base_url: existing.base_url,
        networks: existing.networks,
        mainnet_acknowledged: existing.mainnet_acknowledged,
        ..Default::default()
    };
    // (An all-blank network is already `None` — `non_empty` trims first.)
    let section = cfg.section_mut(target);
    section.api_key = non_empty(api_key);
    // Keep an existing secret if the user left the prompt blank.
    section.api_secret = non_empty(api_secret).or(stored.api_secret);
    // `setup` doesn't touch the wallet session token; preserve it.
    section.session_token = stored.session_token;
    let secret_missing = section.api_secret.is_none();

    let path = save(&cfg)?;
    println!("\nSaved to {} (permissions 0600).", path.display());
    if secret_missing {
        println!(
            "note: no API secret stored for {} — authenticated commands will be refused.",
            target.as_str()
        );
    }
    Ok(())
}

/// The real-funds warning, as one string so `setup` and the active-network
/// banner — the two places that first tell a user what they have selected —
/// cannot drift into two different characterisations of the same risk.
pub fn real_funds_notice(context: &str) -> String {
    format!("⚠  mainnet moves REAL FUNDS. {context}")
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

/// Reject a network name `setup` must not persist. Split out from the
/// interactive flow so the error text — the part a user actually reads — is
/// testable without a terminal.
///
/// A retired release-channel name gets its own sentence: `stable` named a
/// play-funds host, so the reader needs `testnet`, and the intuitive guess
/// (`mainnet`) is the one genuinely dangerous answer.
fn validate_network_name(name: &str) -> Result<()> {
    if NetworkArg::parse(name).is_some() {
        return Ok(());
    }
    let hint = match NetworkArg::retired_replacement(name) {
        Some(replacement) => format!(
            " `{name}` was a release channel, not a network; it named a play-funds host, \
             which is now `{replacement}`."
        ),
        None => String::new(),
    };
    anyhow::bail!("unknown network `{name}`; expected `mainnet`, `testnet` or `local`.{hint}")
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
        let mut cfg = FileConfig {
            network: Some("testnet".into()),
            ..Default::default()
        };
        *cfg.section_mut(NetworkArg::Testnet) = NetworkCredentials {
            api_key: Some("nx_abc".into()),
            api_secret: Some("shh".into()),
            session_token: None,
        };
        let path = save(&cfg).unwrap();
        assert!(path.exists());

        // Round-trips through disk.
        let loaded = load().unwrap().expect("config should be present");
        assert_eq!(loaded.network.as_deref(), Some("testnet"));
        let creds = loaded
            .credentials_for(NetworkArg::Testnet)
            .expect("the testnet section should survive the round trip");
        assert_eq!(creds.api_key.as_deref(), Some("nx_abc"));
        assert_eq!(creds.api_secret.as_deref(), Some("shh"));
        assert_eq!(loaded.base_url, None);

        // `None` fields are omitted from the serialized JSON.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("base_url"),
            "None field should be skipped: {raw}"
        );
        assert!(!raw.contains("session_token"), "None field kept: {raw}");
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
    fn setup_accepts_every_real_network() {
        for name in ["mainnet", "testnet", "local", "TESTNET", "  local  "] {
            assert!(
                validate_network_name(name).is_ok(),
                "{name} is a real network and must be accepted"
            );
        }
    }

    /// ENG-6455: `setup` must not be able to write a name that no longer exists,
    /// and the error has to point at the *right* replacement — `stable` was play
    /// funds, so `testnet`. Naming `mainnet` here would be the dangerous answer.
    #[test]
    fn setup_rejects_retired_release_channels_and_names_the_replacement() {
        for retired in ["stable", "beta"] {
            let err = validate_network_name(retired)
                .expect_err("a retired release channel must not be persisted")
                .to_string();
            assert!(
                err.contains("testnet"),
                "the error for `{retired}` must point at testnet; got: {err}"
            );
            assert!(
                err.contains("release channel"),
                "the error for `{retired}` should explain what it was; got: {err}"
            );
        }
    }

    #[test]
    fn setup_rejects_an_unknown_network_without_a_bogus_hint() {
        let err = validate_network_name("prod")
            .expect_err("an unknown network must be rejected")
            .to_string();
        assert!(err.contains("mainnet") && err.contains("testnet") && err.contains("local"));
        // No retired-name sentence for something that was never a channel.
        assert!(
            !err.contains("release channel"),
            "unexpected retired-name hint for an unrelated value: {err}"
        );
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
    /// must use the stable `session_token` key, inside its network's section,
    /// when it is.
    #[test]
    fn session_token_serializes_under_stable_key() {
        let empty = FileConfig::default();
        let json = serde_json::to_string(&empty).unwrap();
        assert!(!json.contains("session_token"), "empty config: {json}");
        assert!(!json.contains("networks"), "empty config: {json}");

        let mut cfg = FileConfig::default();
        cfg.section_mut(NetworkArg::Testnet).session_token = Some("tok".into());
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(
            json.contains("\"testnet\":{\"session_token\":\"tok\"}"),
            "got {json}"
        );
    }

    #[test]
    fn session_token_round_trips_through_the_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("session-roundtrip");
        // A fresh config has no token.
        assert!(load().unwrap().is_none());

        let path = save_session_token(NetworkArg::Testnet, "sess_tok_abc123").unwrap();
        let loaded = load().unwrap().expect("config should exist after save");
        assert_eq!(
            loaded
                .credentials_for(NetworkArg::Testnet)
                .and_then(|c| c.session_token.as_deref()),
            Some("sess_tok_abc123")
        );

        // Re-saving overwrites only the token, preserving other fields.
        let mut cfg = loaded;
        let section = cfg.section_mut(NetworkArg::Testnet);
        section.api_key = Some("nx_key".into());
        section.api_secret = Some("secret".into());
        save(&cfg).unwrap();
        let again = save_session_token(NetworkArg::Testnet, "sess_tok_xyz789").unwrap();
        assert_eq!(again, path);
        let loaded = load().unwrap().unwrap();
        let creds = loaded.credentials_for(NetworkArg::Testnet).unwrap();
        assert_eq!(creds.session_token.as_deref(), Some("sess_tok_xyz789"));
        assert_eq!(creds.api_key.as_deref(), Some("nx_key"));
        assert_eq!(creds.api_secret.as_deref(), Some("secret"));
    }

    /// A token minted on one network must never be offered on another: that is
    /// the entire point of namespacing, and a session token is exactly as
    /// network-bound as an API key.
    #[test]
    fn a_session_token_stays_on_its_own_network() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("session-per-network");
        save_session_token(NetworkArg::Testnet, "testnet-token").unwrap();
        save_session_token(NetworkArg::Mainnet, "mainnet-token").unwrap();

        let loaded = load().unwrap().unwrap();
        for (network, expected) in [
            (NetworkArg::Testnet, "testnet-token"),
            (NetworkArg::Mainnet, "mainnet-token"),
        ] {
            assert_eq!(
                loaded
                    .credentials_for(network)
                    .and_then(|c| c.session_token.as_deref()),
                Some(expected)
            );
        }
        // A network that was never configured has nothing, rather than
        // inheriting someone else's token.
        assert!(loaded.credentials_for(NetworkArg::Local).is_none());
    }

    /// An atomic write leaves only the config file behind — no `.tmp` sibling.
    #[test]
    fn save_leaves_no_temp_file_behind() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("no-temp");
        let path = save_session_token(NetworkArg::Testnet, "tok").unwrap();
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
        save_session_token(NetworkArg::Testnet, "seed").unwrap();

        let writers: Vec<_> = (0..4)
            .map(|i| {
                std::thread::spawn(move || {
                    for n in 0..40 {
                        save_session_token(NetworkArg::Testnet, &format!("tok-{i}-{n}")).unwrap();
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
        assert!(cfg.credentials_for(NetworkArg::Testnet).is_some());
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

    // ──────────── pre-namespacing config migration (ENG-6462) ────────────

    /// Write a raw config file, bypassing `save`, so the test starts from the
    /// exact bytes an older CLI would have left on disk.
    fn write_raw_config(json: &str) {
        let path = config_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, json).unwrap();
    }

    /// An existing flat config keeps authenticating — against the network it
    /// names, and only that one.
    #[test]
    fn a_flat_config_migrates_to_the_network_it_names() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("migrate-named");
        write_raw_config(
            r#"{"network":"local","api_key":"nx_old","api_secret":"old","session_token":"tok"}"#,
        );

        let cfg = load().unwrap().unwrap();
        let creds = cfg
            .credentials_for(NetworkArg::Local)
            .expect("flat credentials belong to the network the file names");
        assert_eq!(creds.api_key.as_deref(), Some("nx_old"));
        assert_eq!(creds.api_secret.as_deref(), Some("old"));
        assert_eq!(creds.session_token.as_deref(), Some("tok"));

        // Not handed to any other network...
        assert!(cfg.credentials_for(NetworkArg::Mainnet).is_none());
        // ...and the flat fields are cleared, so the next write drops them.
        assert_eq!(cfg.api_key, None);
        assert_eq!(cfg.api_secret, None);
        assert_eq!(cfg.session_token, None);
    }

    /// The dangerous case: a flat config naming no network at all. Its key must
    /// land on the default — where the requests go — and emphatically not become
    /// a credential every network shares, which would hand a testnet key to
    /// `--network mainnet`.
    #[test]
    fn a_flat_config_without_a_network_migrates_to_the_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("migrate-default");
        write_raw_config(r#"{"api_key":"nx_old","api_secret":"old"}"#);

        let cfg = load().unwrap().unwrap();
        assert_eq!(
            cfg.credentials_for(crate::cli::DEFAULT_NETWORK)
                .and_then(|c| c.api_key.as_deref()),
            Some("nx_old")
        );
        assert!(
            !crate::cli::DEFAULT_NETWORK.is_real_funds(),
            "the default must stay play-funds or this migration lands a key on mainnet"
        );
        assert!(cfg.credentials_for(NetworkArg::Mainnet).is_none());
    }

    /// A config naming a network this build cannot parse (a retired channel, a
    /// typo) lands its credentials on the same default the *requests* fall back
    /// to, so the two never diverge.
    #[test]
    fn an_unparseable_network_migrates_where_the_requests_land() {
        let _guard = ENV_LOCK.lock().unwrap();
        for (tag, name) in [("migrate-stale", "stable"), ("migrate-typo", "mainet")] {
            let _tmp = TempConfigHome::new(tag);
            write_raw_config(&format!(
                r#"{{"network":"{name}","api_key":"k","api_secret":"s"}}"#
            ));
            let cfg = load().unwrap().unwrap();
            assert_eq!(
                cfg.credentials_for(crate::cli::DEFAULT_NETWORK)
                    .and_then(|c| c.api_key.as_deref()),
                Some("k"),
                "a config naming {name:?} should land its key on the default network"
            );
        }
    }

    /// A half-migrated file — someone added a `networks` section by hand next to
    /// the old flat fields — must not have the stale value overwrite the newer,
    /// more specific one.
    #[test]
    fn a_namespaced_value_wins_over_the_legacy_field() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("migrate-half");
        write_raw_config(
            r#"{"network":"testnet","api_key":"old","api_secret":"old-secret",
                "networks":{"testnet":{"api_key":"new"}}}"#,
        );

        let cfg = load().unwrap().unwrap();
        let creds = cfg.credentials_for(NetworkArg::Testnet).unwrap();
        assert_eq!(creds.api_key.as_deref(), Some("new"), "the section wins");
        // Per-field, so the legacy half with no namespaced counterpart survives
        // rather than being dropped along with the field that was superseded.
        assert_eq!(creds.api_secret.as_deref(), Some("old-secret"));
    }

    /// The migration reaches disk the next time anything writes, so a config is
    /// converted once rather than re-migrated on every invocation forever.
    #[test]
    fn the_migration_is_persisted_by_the_next_write() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("migrate-persist");
        write_raw_config(r#"{"network":"testnet","api_key":"nx_old","api_secret":"old"}"#);

        let path = save_session_token(NetworkArg::Testnet, "tok").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(
            parsed.get("api_key").is_none(),
            "the flat field should be gone from disk: {raw}"
        );
        assert_eq!(parsed["networks"]["testnet"]["api_key"], "nx_old");
        assert_eq!(parsed["networks"]["testnet"]["session_token"], "tok");
    }

    /// A section for a network this build does not know must survive a
    /// round-trip rather than being silently deleted by the next write — the
    /// file's key space is the file's, not this binary's.
    #[test]
    fn an_unknown_section_survives_a_rewrite() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("unknown-section");
        write_raw_config(r#"{"networks":{"devnet":{"api_key":"nx_devnet"}}}"#);

        let path = save_session_token(NetworkArg::Testnet, "tok").unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed["networks"]["devnet"]["api_key"], "nx_devnet");
    }

    /// An abandoned `setup` must not leave a `"mainnet": {}` stub, which would
    /// read as "mainnet is configured" to anyone looking at the file.
    #[test]
    fn empty_sections_are_not_written() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("empty-section");
        let mut cfg = FileConfig::default();
        cfg.section_mut(NetworkArg::Mainnet);
        let path = save(&cfg).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("mainnet"), "empty section written: {raw}");
        assert!(!raw.contains("networks"), "empty map written: {raw}");
    }

    /// The acknowledgement is absent until earned, then persists, and never
    /// disturbs the credentials sitting beside it.
    #[test]
    fn the_mainnet_acknowledgement_persists() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("ack");
        assert!(!FileConfig::default().mainnet_acknowledged);
        // Absent from a fresh file rather than written as `false`.
        let path = save_session_token(NetworkArg::Testnet, "tok").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("mainnet_acknowledged"), "got {raw}");

        save_mainnet_acknowledged().unwrap();
        let cfg = load().unwrap().unwrap();
        assert!(cfg.mainnet_acknowledged);
        assert_eq!(
            cfg.credentials_for(NetworkArg::Testnet)
                .and_then(|c| c.session_token.as_deref()),
            Some("tok"),
            "recording the acknowledgement must not disturb stored credentials"
        );
    }

    /// Namespaced secrets must be as unloggable as the flat ones were.
    #[test]
    fn debug_redacts_namespaced_secrets() {
        let mut cfg = FileConfig::default();
        *cfg.section_mut(NetworkArg::Mainnet) = NetworkCredentials {
            api_key: Some("nx_visible".into()),
            api_secret: Some("topsecret".into()),
            session_token: Some("supersecrettoken".into()),
        };
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("topsecret"), "secret leaked: {dbg}");
        assert!(!dbg.contains("supersecrettoken"), "token leaked: {dbg}");
        assert!(dbg.contains("nx_visible"));
        assert!(dbg.contains("<redacted>"));
    }

    #[test]
    #[cfg(unix)]
    fn session_token_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("session-perm");
        let path = save_session_token(NetworkArg::Testnet, "sess_tok_perm").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "session-token file must be 0600");
    }
}
