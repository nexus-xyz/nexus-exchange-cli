//! Credential & config resolution: flags / env (handled by clap) layered over
//! an on-disk config written by `nexus setup`.
//!
//! Precedence, highest first: command-line flag, environment variable, config
//! file, built-in default. Flags and env are merged by clap; this module adds
//! the file layer underneath and owns the interactive `setup` flow.
//!
//! The config file holds an API secret, so it is created `0600` (owner
//! read/write only) inside a `0700` directory, and the secret is never echoed
//! while typing or printed back out.

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
    write_private(&path, json.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
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
}
