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
