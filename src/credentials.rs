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

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cli::{CustomNetworkConfig, NetworkArg};

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
    /// **Deprecated** (ENG-10956) in favour of a `custom_networks` entry selected
    /// by `network` above. Still read, with unchanged precedence — it beats
    /// `network` — and still written by nothing, since `nexus setup` has never
    /// emitted it.
    ///
    /// This is the quieter half of the deprecation and the reason the notice
    /// exists at runtime rather than only in `--help`: a `--base-url` on the
    /// command line is at least visible in the command that used it, whereas this
    /// key keeps redirecting every invocation long after whoever added it has
    /// forgotten, while declaring neither funds nor a credential namespace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Credentials keyed by network name (`testnet`, `mainnet`, `local`, or a
    /// custom network's label).
    ///
    /// `BTreeMap` rather than a struct with one field per network: the key space
    /// is the *file's*, not this build's, so a section for a network this binary
    /// does not know survives a round-trip instead of being silently dropped on
    /// the next write. It is also ordered, so the file's diff is stable.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub networks: BTreeMap<String, NetworkCredentials>,

    /// Caller-declared stages, keyed by label (ENG-9827). A label here is
    /// selectable with `--network <label>` and namespaces its own credentials
    /// under [`networks`](Self::networks).
    ///
    /// The **key is the label**, so there is exactly one place a stage is named
    /// and no way for a key and an inner `label` field to disagree about which
    /// credential slot the stage owns.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub custom_networks: BTreeMap<String, CustomNetworkConfig>,

    /// Networks whose real-funds warning the user has acknowledged, by name. Set
    /// by the one-time prompt on the first trade against each (ENG-6462,
    /// ENG-9827).
    ///
    /// Per network rather than one global flag: a custom stage can declare
    /// `"funds": "real"`, so there is no longer exactly one real-funds target,
    /// and a single flag would let acknowledging mainnet silently disarm the
    /// prompt for every private real-funds stage as well. That is the schema
    /// change `exactly_one_network_moves_real_funds` existed to force a decision
    /// on.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub acknowledged_networks: BTreeSet<String>,

    /// Pre-ENG-9827 spelling of the above: a single bool meaning "mainnet".
    /// Read so an existing acknowledgement is not forgotten, folded into
    /// [`acknowledged_networks`](Self::acknowledged_networks) by
    /// [`FileConfig::migrate_legacy_acknowledgement`], and never written back.
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
    /// The credentials stored under `namespace`, if any.
    ///
    /// Keyed by `&str` rather than by [`NetworkArg`] because a custom network's
    /// namespace is its label, which is a string the SDK has already vetted as a
    /// storage key ([`nexus_exchange::Network::label`]). The only producers of a
    /// namespace are [`crate::cli::Target::namespace`] and
    /// [`NetworkArg::name`], both of which carry that guarantee.
    pub fn credentials_for(&self, namespace: &str) -> Option<&NetworkCredentials> {
        self.networks.get(namespace)
    }

    /// The credential section for `namespace`, creating it if absent.
    pub fn section_mut(&mut self, namespace: &str) -> &mut NetworkCredentials {
        self.networks.entry(namespace.to_string()).or_default()
    }

    /// Whether the real-funds warning for `namespace` has been acknowledged.
    pub fn acknowledged(&self, namespace: &str) -> bool {
        self.acknowledged_networks.contains(namespace)
    }

    /// Fold a pre-ENG-9827 `mainnet_acknowledged` flag into
    /// [`acknowledged_networks`](Self::acknowledged_networks), clearing it so the
    /// next write emits only the keyed layout.
    ///
    /// One-way on purpose: the old flag said "mainnet", so that is the only
    /// network it can grant. It must not be read as blanket consent for a
    /// real-funds stage that did not exist when it was set.
    pub fn migrate_legacy_acknowledgement(&mut self) {
        if std::mem::take(&mut self.mainnet_acknowledged) {
            self.acknowledged_networks
                .insert(NetworkArg::Mainnet.name().to_string());
        }
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
    pub fn migrate_legacy_credentials(&mut self, owner: &NetworkArg) {
        let legacy = NetworkCredentials {
            api_key: self.api_key.take(),
            api_secret: self.api_secret.take(),
            session_token: self.session_token.take(),
        };
        if legacy.is_empty() {
            return;
        }
        let section = self.section_mut(owner.name());
        section.api_key = section.api_key.take().or(legacy.api_key);
        section.api_secret = section.api_secret.take().or(legacy.api_secret);
        section.session_token = section.session_token.take().or(legacy.session_token);
    }
}

impl std::fmt::Debug for FileConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileConfig")
            .field("network", &self.network)
            // Redacted like the secrets below, not printed like the plain fields
            // above: the legacy `base_url` is never validated, so it can carry
            // `user:pass@` userinfo (ENG-10956). Nothing `Debug`-prints a whole
            // `FileConfig` today — this is so that whoever first logs one for
            // diagnostics does not ship a password with it.
            .field(
                "base_url",
                &self.base_url.as_deref().map(crate::cli::redact_userinfo),
            )
            .field("networks", &self.networks)
            .field("custom_networks", &self.custom_networks)
            .field("acknowledged_networks", &self.acknowledged_networks)
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
            cfg.migrate_legacy_credentials(&legacy_credential_owner(&cfg));
            cfg.migrate_legacy_acknowledgement();
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
    crate::cli::declared_network(cfg).unwrap_or(crate::cli::DEFAULT_NETWORK)
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
pub fn save_session_token(namespace: &str, token: &str) -> Result<PathBuf> {
    let mut cfg = load()?.unwrap_or_default();
    cfg.section_mut(namespace).session_token = Some(token.to_string());
    save(&cfg)
}

/// Record that the user has acknowledged `namespace`'s real-funds warning, so
/// the one-time prompt does not reappear on every trade *there*. Preserves every
/// other field. Returns the path it was written to.
///
/// Re-reads the file rather than writing back the caller's copy: another process
/// may have added a network or a credential since this one loaded, and a
/// whole-file overwrite from stale state would drop it.
pub fn save_acknowledged(namespace: &str) -> Result<PathBuf> {
    let mut cfg = load()?.unwrap_or_default();
    cfg.acknowledged_networks.insert(namespace.to_string());
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

    // Never offer a stored value back as the default unless it is still a
    // selectable network: a config written before the release-channel rename
    // holds `stable`/`beta`, and pre-filling that would have the user press Enter
    // and re-save a name that no longer exists.
    let choices = network_choices(&existing);
    let network = prompt_line(
        &format!("Network [{choices}]"),
        existing
            .network
            .as_deref()
            .filter(|n| validate_network_name(n, &existing).is_ok())
            .or(Some("testnet")),
    )?;
    // Validated before the credential prompts so a typo costs nothing — the
    // secret has not been typed yet. Writing an unusable network would push the
    // failure to every later invocation instead of surfacing it here.
    let network = non_empty(network);
    if let Some(name) = network.as_deref() {
        validate_network_name(name, &existing)?;
    }

    // The credentials being typed belong to the network just chosen, not to
    // whatever the file named before: `setup` is how you add a second network's
    // key, so re-running it with a different answer must not overwrite the first.
    // A blank answer means "no preference", which lands on the default.
    let target = network
        .as_deref()
        .and_then(|name| crate::cli::selectable_network(name, &existing))
        .unwrap_or(crate::cli::DEFAULT_NETWORK);
    // The classification is the stage's own, never inferred from its name — a
    // custom stage is real-funds only because it says so.
    if crate::cli::declared_funds(&target, &existing) == nexus_exchange::Funds::Real {
        println!(
            "\n{}\n",
            real_funds_notice(
                target.name(),
                "These credentials will be stored for a real-funds network."
            )
        );
    }

    let stored = existing
        .credentials_for(target.name())
        .cloned()
        .unwrap_or_default();
    println!(
        "\nCredentials for {} (each network needs its own key — a key is valid on \
         exactly one network).",
        target.name()
    );

    let api_key = prompt_line("API key id (nx_...)", stored.api_key.as_deref())?;

    // Read the secret without echoing it to the terminal.
    let api_secret = rpassword::prompt_password("API secret (input hidden, blank keeps current): ")
        .context("failed to read API secret")?;

    let mut cfg = FileConfig {
        network,
        base_url: existing.base_url,
        networks: existing.networks,
        custom_networks: existing.custom_networks,
        acknowledged_networks: existing.acknowledged_networks,
        ..Default::default()
    };
    // (An all-blank network is already `None` — `non_empty` trims first.)
    let section = cfg.section_mut(target.name());
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
            target.name()
        );
    }
    Ok(())
}

/// The real-funds warning, as one string so `setup` and the active-network
/// banner — the two places that first tell a user what they have selected —
/// cannot drift into two different characterisations of the same risk.
///
/// `network` is named rather than hardcoded to `mainnet`: a custom stage that
/// declares `"funds": "real"` moves real money under its own label, and a
/// warning that says "mainnet" while the user is pointed at something else is
/// worse than no warning — it tells them the guard is about a network they are
/// not on.
pub fn real_funds_notice(network: &str, context: &str) -> String {
    format!("⚠  {network} moves REAL FUNDS. {context}")
}

/// The network names `setup` offers, for the prompt. Built-ins first, then any
/// declared custom labels, so the list is what this file can actually select.
///
/// Filtered through the same check the next prompt applies rather than listing
/// the raw map keys: an entry that cannot be selected — a label the SDK refuses,
/// or a stage with no reachable base URL — would be an offer that prompt
/// rejects. The keys are also file-supplied text on its way to a terminal, and a
/// selectable label has passed the SDK's character check, so it carries no
/// control bytes.
///
/// An entry claiming a built-in's name is skipped rather than listed a second
/// time: it resolves to the built-in already above it in this list, and offering
/// the same name twice would suggest the two are different choices.
fn network_choices(existing: &FileConfig) -> String {
    let mut names: Vec<&str> = vec!["mainnet", "testnet", "local"];
    names.extend(
        existing
            .custom_networks
            .keys()
            .filter(|label| {
                crate::cli::selectable_network(label, existing)
                    .filter(|selected| selected.builtin_network().is_none())
                    .is_some_and(|selected| {
                        crate::cli::check_selection(&selected, existing).is_ok()
                    })
            })
            .map(String::as_str),
    );
    names.join("/")
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
///
/// A custom label is accepted only when this same file **declares** it, and
/// declares it *usably*. `setup` writes the `network` key, and a name nothing
/// describes — or describes with no `base_url`, or with one the SDK refuses —
/// would select a stage that cannot be reached on every later invocation. The
/// failure belongs here, where it is one prompt away from being fixed, rather
/// than after a config the user has been told is saved.
fn validate_network_name(name: &str, existing: &FileConfig) -> Result<()> {
    if let Some(selected) = crate::cli::selectable_network(name, existing) {
        // Declared is not the same as usable: `selectable_network` answers only
        // that the file has an entry under this label.
        return crate::cli::check_selection(&selected, existing);
    }
    let hint = match NetworkArg::retired_replacement(name) {
        Some(replacement) => format!(
            " `{name}` was a release channel, not a network; it named a play-funds host, \
             which is now `{replacement}`."
        ),
        None => String::new(),
    };
    anyhow::bail!(
        "unknown network `{name}`; expected `mainnet`, `testnet`, `local`, or a label declared \
         under \"custom_networks\" in the config file.{hint}"
    )
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
        *cfg.section_mut("testnet") = NetworkCredentials {
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
            .credentials_for("testnet")
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
        let existing = FileConfig::default();
        for name in ["mainnet", "testnet", "local", "TESTNET", "  local  "] {
            assert!(
                validate_network_name(name, &existing).is_ok(),
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
            let err = validate_network_name(retired, &FileConfig::default())
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
        let err = validate_network_name("prod", &FileConfig::default())
            .expect_err("an unknown network must be rejected")
            .to_string();
        assert!(err.contains("mainnet") && err.contains("testnet") && err.contains("local"));
        // No retired-name sentence for something that was never a channel.
        assert!(
            !err.contains("release channel"),
            "unexpected retired-name hint for an unrelated value: {err}"
        );
    }

    /// A declared label is only a candidate. `setup` writes the `network` key,
    /// so it has to know the stage is *reachable* — an entry with no `base_url`,
    /// or one the SDK refuses, would otherwise be accepted here and then
    /// hard-error on every command run against the config it just saved.
    #[test]
    fn setup_rejects_a_declared_but_unusable_stage() {
        let usable = crate::cli::CustomNetworkConfig {
            base_url: Some("https://exchange.example.com/api/exchange".into()),
            funds: Some("play".into()),
            ..Default::default()
        };
        let unusable = [
            // Declared, funds and all, but with nowhere to send a request.
            crate::cli::CustomNetworkConfig {
                funds: Some("play".into()),
                ..Default::default()
            },
            // A URL the SDK refuses: userinfo would leak into every log line.
            crate::cli::CustomNetworkConfig {
                base_url: Some("https://user:pass@exchange.example.com".into()),
                funds: Some("play".into()),
                ..Default::default()
            },
        ];

        for declared in unusable {
            let mut file = FileConfig::default();
            file.custom_networks.insert("dev".into(), declared);
            let err = validate_network_name("dev", &file)
                .expect_err("a stage that cannot be reached must not be persisted")
                .to_string();
            assert!(err.contains("dev"), "the error must name the stage: {err}");
            // ...and it is not offered as a choice either, since the prompt
            // would only reject it.
            assert!(
                !network_choices(&file).contains("dev"),
                "an unusable stage must not be offered: {}",
                network_choices(&file)
            );
        }

        // The same label, declared usably, is accepted and offered.
        let mut file = FileConfig::default();
        file.custom_networks.insert("dev".into(), usable);
        assert!(validate_network_name("dev", &file).is_ok());
        assert_eq!(network_choices(&file), "mainnet/testnet/local/dev");
    }

    /// An entry claiming a built-in's name resolves to the built-in, so offering
    /// it twice would present one network as two choices.
    #[test]
    fn setup_offers_a_shadowing_declaration_once() {
        let mut file = FileConfig::default();
        file.custom_networks.insert(
            "mainnet".into(),
            crate::cli::CustomNetworkConfig {
                base_url: Some("https://exchange.example.com/api/exchange".into()),
                funds: Some("play".into()),
                ..Default::default()
            },
        );
        assert_eq!(network_choices(&file), "mainnet/testnet/local");
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

    /// `Debug` masks a password in the legacy `base_url` too.
    ///
    /// The impl is hand-written so that a config can be logged without shipping a
    /// secret, and `base_url` is unvalidated — it can carry `user:pass@` where
    /// the declared `custom_networks` path would have rejected it. Pinned now
    /// because the leak would arrive with whoever first adds a diagnostic log,
    /// long after this line was written.
    #[test]
    fn debug_redacts_userinfo_in_the_legacy_base_url() {
        let cfg = FileConfig {
            base_url: Some("https://alice:hunter2@exchange.example.com".into()),
            ..Default::default()
        };
        let dbg = format!("{cfg:?}");
        assert!(
            !dbg.contains("hunter2") && !dbg.contains("alice"),
            "userinfo leaked via Debug: {dbg}"
        );
        assert!(
            dbg.contains("exchange.example.com"),
            "the host should survive so the field stays useful: {dbg}"
        );
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
        cfg.section_mut("testnet").session_token = Some("tok".into());
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

        let path = save_session_token("testnet", "sess_tok_abc123").unwrap();
        let loaded = load().unwrap().expect("config should exist after save");
        assert_eq!(
            loaded
                .credentials_for("testnet")
                .and_then(|c| c.session_token.as_deref()),
            Some("sess_tok_abc123")
        );

        // Re-saving overwrites only the token, preserving other fields.
        let mut cfg = loaded;
        let section = cfg.section_mut("testnet");
        section.api_key = Some("nx_key".into());
        section.api_secret = Some("secret".into());
        save(&cfg).unwrap();
        let again = save_session_token("testnet", "sess_tok_xyz789").unwrap();
        assert_eq!(again, path);
        let loaded = load().unwrap().unwrap();
        let creds = loaded.credentials_for("testnet").unwrap();
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
        save_session_token("testnet", "testnet-token").unwrap();
        save_session_token("mainnet", "mainnet-token").unwrap();

        let loaded = load().unwrap().unwrap();
        for (network, expected) in [("testnet", "testnet-token"), ("mainnet", "mainnet-token")] {
            assert_eq!(
                loaded
                    .credentials_for(network)
                    .and_then(|c| c.session_token.as_deref()),
                Some(expected)
            );
        }
        // A network that was never configured has nothing, rather than
        // inheriting someone else's token.
        assert!(loaded.credentials_for("local").is_none());
    }

    /// An atomic write leaves only the config file behind — no `.tmp` sibling.
    #[test]
    fn save_leaves_no_temp_file_behind() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("no-temp");
        let path = save_session_token("testnet", "tok").unwrap();
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
        save_session_token("testnet", "seed").unwrap();

        let writers: Vec<_> = (0..4)
            .map(|i| {
                std::thread::spawn(move || {
                    for n in 0..40 {
                        save_session_token("testnet", &format!("tok-{i}-{n}")).unwrap();
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
        assert!(cfg.credentials_for("testnet").is_some());
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
            .credentials_for("local")
            .expect("flat credentials belong to the network the file names");
        assert_eq!(creds.api_key.as_deref(), Some("nx_old"));
        assert_eq!(creds.api_secret.as_deref(), Some("old"));
        assert_eq!(creds.session_token.as_deref(), Some("tok"));

        // Not handed to any other network...
        assert!(cfg.credentials_for("mainnet").is_none());
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
            cfg.credentials_for(crate::cli::DEFAULT_NETWORK.name())
                .and_then(|c| c.api_key.as_deref()),
            Some("nx_old")
        );
        assert_eq!(
            crate::cli::declared_funds(&crate::cli::DEFAULT_NETWORK, &FileConfig::default()),
            nexus_exchange::Funds::Play,
            "the default must stay play-funds or this migration lands a key on mainnet"
        );
        assert!(cfg.credentials_for("mainnet").is_none());
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
                cfg.credentials_for(crate::cli::DEFAULT_NETWORK.name())
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
        let creds = cfg.credentials_for("testnet").unwrap();
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

        let path = save_session_token("testnet", "tok").unwrap();
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

        let path = save_session_token("testnet", "tok").unwrap();
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
        cfg.section_mut("mainnet");
        let path = save(&cfg).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("mainnet"), "empty section written: {raw}");
        assert!(!raw.contains("networks"), "empty map written: {raw}");
    }

    /// The acknowledgement is absent until earned, then persists under the
    /// network it was given for, and never disturbs the credentials sitting
    /// beside it.
    #[test]
    fn the_real_funds_acknowledgement_persists_per_network() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("ack");
        assert!(FileConfig::default().acknowledged_networks.is_empty());
        // Absent from a fresh file rather than written as an empty list.
        let path = save_session_token("testnet", "tok").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("acknowledged_networks"), "got {raw}");

        save_acknowledged("mainnet").unwrap();
        let cfg = load().unwrap().unwrap();
        assert!(cfg.acknowledged("mainnet"));
        // Consent is for the network it was given for and no other — this is the
        // whole reason it is a set rather than the bool it replaced.
        assert!(!cfg.acknowledged("dev"));
        assert_eq!(
            cfg.credentials_for("testnet")
                .and_then(|c| c.session_token.as_deref()),
            Some("tok"),
            "recording the acknowledgement must not disturb stored credentials"
        );
    }

    /// A config written before the acknowledgement was keyed holds a single
    /// `mainnet_acknowledged` bool. It must keep meaning what it said — consent
    /// for mainnet — and nothing more: reading it as blanket consent would let a
    /// pre-ENG-9827 file silently pre-approve a real-funds stage that did not
    /// exist when the flag was set.
    #[test]
    fn a_legacy_acknowledgement_covers_mainnet_and_nothing_else() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("ack-legacy");
        write_raw_config(r#"{"network":"testnet","mainnet_acknowledged":true}"#);

        let cfg = load().unwrap().unwrap();
        assert!(cfg.acknowledged("mainnet"));
        assert!(!cfg.acknowledged("dev"));
        assert!(!cfg.acknowledged("testnet"));
        // ...and the old spelling is cleared, so the next write drops it.
        assert!(!cfg.mainnet_acknowledged);

        let path = save(&cfg).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("mainnet_acknowledged"),
            "the legacy flag should be gone from disk: {raw}"
        );
        assert!(raw.contains("acknowledged_networks"), "got {raw}");
    }

    /// A custom stage declared in the file survives a round trip with every
    /// field of its bundle intact. The bundle is the safety metadata, so a field
    /// silently dropped on write is a target that reads as less dangerous than it
    /// is the next time it is loaded.
    #[test]
    fn a_declared_custom_network_round_trips() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _tmp = TempConfigHome::new("custom-roundtrip");
        let declared = CustomNetworkConfig {
            base_url: Some("https://exchange.example.com/api/exchange".into()),
            direct_base_url: Some("https://direct.example.com".into()),
            funds: Some("real".into()),
            ws_url: Some("wss://stream.example.com/ws".into()),
            faucet: Some(false),
            chain_id: Some(1),
        };
        let mut cfg = FileConfig {
            network: Some("dev".into()),
            ..Default::default()
        };
        cfg.custom_networks.insert("dev".into(), declared.clone());
        save(&cfg).unwrap();

        let loaded = load().unwrap().expect("config should be present");
        assert_eq!(loaded.custom_networks.get("dev"), Some(&declared));
    }

    /// Namespaced secrets must be as unloggable as the flat ones were.
    #[test]
    fn debug_redacts_namespaced_secrets() {
        let mut cfg = FileConfig::default();
        *cfg.section_mut("mainnet") = NetworkCredentials {
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
        let path = save_session_token("testnet", "sess_tok_perm").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "session-token file must be 0600");
    }
}
