//! Command-line argument parsing and config/credential resolution.

use anyhow::{bail, Result};
use clap::{Parser, Subcommand, ValueEnum};
use nexus_exchange::rest::{MAX_FILLS_LIMIT, MAX_PORTFOLIO_HISTORY_LIMIT};
use nexus_exchange::types::{OrderType, PortfolioWindow, Side, TimeInForce};
use nexus_exchange::{Config, CustomNetwork, Funds, Network, SigningDomain};
use serde::{Deserialize, Serialize};

use crate::credentials::FileConfig;

// Re-export for use in main.rs.
pub use clap_complete::Shell;

/// `User-Agent` the CLI reports to the API so the indexer can attribute traffic
/// to the CLI specifically (vs. the Rust SDK, the web frontend, or raw callers).
///
/// The version is the crate version baked in at compile time, so it can never
/// drift from `Cargo.toml`. The string is a fixed constant with no user- or
/// environment-supplied input, so it carries no HTTP-header-injection risk; the
/// SDK additionally falls back to its own default UA if a value ever contained
/// bytes illegal in a header.
const USER_AGENT: &str = concat!("nexus-cli/", env!("CARGO_PKG_VERSION"));

/// Version string for `nexus --version` / `-V`: the CLI crate version, the spec
/// tag the CLI is compiled against, and the resolved `nexus-exchange` SDK
/// version it links. The CLI carries no transport of its own — it speaks the
/// API through the SDK — so surfacing all three makes it clear which spec the
/// CLI targets (the same tag the SDK sends as `X-Nexus-Api-Version`) and which
/// SDK build backs it. The spec tag and SDK version are injected at build time
/// by `build.rs` (from `.api-version` and `Cargo.lock`), so they can't drift.
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (spec ",
    env!("NEXUS_SPEC_TAG"),
    ", nexus-exchange ",
    env!("NEXUS_SDK_VERSION"),
    ")"
);

/// Command-line interface for the Nexus Exchange API.
#[derive(Debug, Parser)]
#[command(name = "nexus", version = LONG_VERSION, about, long_about = None)]
pub struct Cli {
    /// Which network to target: `mainnet`, `testnet`, `local`, or the label of a
    /// custom network declared under `custom_networks` in the config file
    /// (default: testnet, or the `nexus setup` value).
    ///
    /// `stable` and `beta` named release channels, not networks, and were
    /// retired in `nexus-exchange` 0.8.0. `stable` pointed at a **play-funds**
    /// host, so its replacement is `testnet` — *not* `mainnet`, which is real
    /// funds and is not reachable in this release.
    #[arg(long, global = true, env = "NEXUS_NETWORK", value_parser = NetworkArg::from_flag)]
    pub network: Option<NetworkArg>,

    /// Override the API base URL (takes precedence over `--network`).
    ///
    /// Retained for compatibility, and now sugar for a custom network whose
    /// funds are **undeclared**. It redirects the request without changing which
    /// network's credentials are presented, so commands that move or mint funds
    /// are refused rather than inheriting the named network's safety flags — a
    /// bare URL says nothing about what its host moves.
    ///
    /// Declare the stage under `custom_networks` and select it with `--network
    /// <label>` to get a validated URL, its own credential namespace, and a
    /// funds classification.
    #[arg(long, global = true, env = "NEXUS_BASE_URL")]
    pub base_url: Option<String>,

    /// Output format: human-readable tables or pretty JSON.
    #[arg(long, value_enum, global = true, default_value_t = OutputFormat::Human, env = "NEXUS_OUTPUT")]
    pub output: OutputFormat,

    #[command(flatten)]
    pub credentials: Credentials,

    #[command(subcommand)]
    pub command: Command,
}

/// API credentials. Read from flags, the corresponding environment variables,
/// or the config file written by `nexus setup` (in that order of precedence).
/// Authenticated commands sign requests when both halves are present.
///
/// `Debug` is implemented by hand so the secret never lands in logs.
#[derive(clap::Args)]
pub struct Credentials {
    /// API key id (e.g. `nx_...`).
    #[arg(long, global = true, env = "NEXUS_API_KEY", hide_env_values = true)]
    pub api_key: Option<String>,

    /// API secret. Prefer the env var or `nexus setup` over the flag, since
    /// flags are visible in your shell history and process list.
    #[arg(long, global = true, env = "NEXUS_API_SECRET", hide_env_values = true)]
    pub api_secret: Option<String>,

    /// Wallet session token from `nexus auth login`. Authenticates
    /// session-scoped routes when no HMAC key/secret pair is configured. Prefer
    /// the env var or the stored login over the flag (flags are visible in your
    /// shell history and process list).
    #[arg(
        long,
        global = true,
        env = "NEXUS_SESSION_TOKEN",
        hide_env_values = true
    )]
    pub session_token: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
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

/// Which Nexus Exchange network to target.
///
/// This is the **network** axis the spec formalizes (ENG-6442), not a release
/// channel. The distinction is the whole point: the retired `stable`/`beta`
/// values named deployment channels, which is how a play-funds host came to be
/// labelled "production" and how the SDK's real-funds guard ended up pointed at
/// the wrong network (ENG-6452).
///
/// # Why this is not a `ValueEnum` any more
///
/// [`Custom`](Self::Custom) carries a caller-chosen label, and clap's derived
/// `ValueEnum` can only produce unit variants. The cost is that `--help` no
/// longer enumerates the values, so the help text and every rejection message
/// spell them out by hand instead — see [`from_flag`](Self::from_flag).
///
/// # A label is only a *name*; whether it names anything is a separate question
///
/// A non-built-in value is syntactically indistinguishable from a typo
/// (`mainet`), so this type does not try to tell them apart. It carries the
/// label; [`Cli::target`] decides whether the config file declares it, and says
/// so when it does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkArg {
    /// **Real funds.** Not reachable in this release: the SDK refuses every
    /// request locally rather than guess a host or sign an unverifiable path.
    Mainnet,
    /// **Play funds** credited by the faucet — the default, and the safe target.
    Testnet,
    /// A locally run indexer. Play funds, and never a fallback.
    Local,
    /// A stage declared under `custom_networks` in the config file, named by its
    /// label (ENG-9827). The label — not the URL — is the credential namespace
    /// key, so two stages on the same host still keep separate credentials.
    Custom(String),
}

/// Retired release-channel names, mapped to the network each actually pointed
/// at. `--network` rejects these outright (clap only accepts the variants
/// above); this table exists solely so a **config file** written before the
/// rename gets a real diagnostic instead of a silent fallback.
///
/// The mapping is the part worth stating: `stable` was a *play-funds* host, so
/// it becomes `testnet`. Reading it as `mainnet` — the intuitive guess — has it
/// exactly backwards and is the mislabel this axis exists to correct.
const RETIRED_NETWORKS: &[(&str, &str)] = &[("stable", "testnet"), ("beta", "testnet")];

/// The vocabulary `--network` accepts, spelled once so the flag's help, the
/// parse error and the stale-config warning cannot drift into three different
/// answers. Needed by hand because [`NetworkArg`] is no longer a `ValueEnum`
/// (see its docs).
const NETWORK_VALUES: &str = "mainnet, testnet, local, or the label of a network declared under \
     \"custom_networks\" in the config file";

/// The diagnostic for a config-file `network` this build cannot resolve, or
/// `None` when there is nothing to report. Split out of [`Cli::target`] so the
/// text — the part a user actually reads — is testable without capturing stderr.
///
/// `landing` is where the invocation really ends up, read from the *resolved*
/// default config rather than restated from [`RETIRED_NETWORKS`]. Those are two
/// independent facts that merely coincide today: `stable` named a play-funds host
/// no matter what the SDK default later becomes. Deriving both from the table
/// would let a future default change silently turn this warning into a false
/// statement.
///
/// A blank value reports nothing. `nexus setup` normalizes an empty answer to
/// "no preference", and a hand-edited `"network": ""` means the same thing — it
/// selects nothing, so there is nothing stale to fix.
///
/// A label that *is* declared never reaches here; an undeclared one takes the
/// "not a known network" branch, which is the honest reading — the file names a
/// stage it does not describe.
fn stale_network_warning(name: &str, landing: &str) -> Option<String> {
    if name.trim().is_empty() {
        return None;
    }
    // Every interpolation of `name` uses `{name:?}`, never `{name}`: the value
    // comes from a file and is echoed to a terminal, and `Debug` escapes control
    // bytes, so a config cannot smuggle ESC sequences into this line.
    Some(match NetworkArg::retired_replacement(name) {
        Some(replacement) => format!(
            "warning: config-file network {name:?} was a release channel, not a network, and no \
             longer exists; it named a play-funds host, which is now `{replacement}`. Using the \
             default network ({landing}). Run `nexus setup`, or set \"network\": \
             \"{replacement}\" in the config."
        ),
        None => format!(
            "warning: config-file network {name:?} is not a known network (valid: \
             {NETWORK_VALUES}); using the default network ({landing}). Run `nexus setup`, or fix \
             \"network\" in the config."
        ),
    })
}

/// The network an invocation targets when nothing selects one.
///
/// The SDK's own [`Config::default`] is the authority; this is the CLI's
/// transcription of it, needed because credential namespacing has to name a
/// section (a string key) before any [`Config`] exists, and because
/// [`Network`] is `#[non_exhaustive]` so the SDK default cannot be mapped back
/// to a [`NetworkArg`] infallibly. `the_default_network_matches_the_sdk` is what
/// notices if the two ever disagree — pinning an upstream fact without a check
/// is the failure mode ENG-6452 was.
pub(crate) const DEFAULT_NETWORK: NetworkArg = NetworkArg::Testnet;

/// Placeholder base URL used to run a label past the SDK's validator without
/// describing a real deployment. RFC 2606 reserves `.invalid`, so it can never
/// resolve, and the target built around it is dropped immediately — see
/// [`validate_label`].
const LABEL_PROBE_URL: &str = "https://example.invalid";

/// Validate a custom-network label, returning it trimmed.
///
/// The rules — the character set, the length cap, the refusal of `.`/`..` and of
/// the built-in network names — live in `nexus-exchange`, because that is where
/// [`Network::label`] is documented as safe to key per-network credential
/// storage on. So this does not transcribe them: it hands the label to the real
/// constructor, against a placeholder URL, and keeps only the verdict.
///
/// Transcribing an upstream rule with nothing to notice when upstream moves is
/// what pointed the real-funds guard at the wrong network to begin with
/// (ENG-6452), and here the cost of drifting would be a label the CLI accepts as
/// a credential key that the SDK would have refused.
fn validate_label(label: &str) -> Result<String, String> {
    CustomNetwork::new(label, LABEL_PROBE_URL, Funds::Unknown)
        .map(|probe| probe.label().to_string())
        .map_err(|e| e.to_string())
}

impl NetworkArg {
    /// The canonical name, used as the config file's section key and in
    /// diagnostics. For a custom target this is the caller-supplied label, which
    /// [`validate_label`] has already vetted as a storage key.
    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet => "testnet",
            Self::Local => "local",
            Self::Custom(label) => label,
        }
    }

    /// Parse a **built-in** network name. Returns `None` for anything else,
    /// including a custom label — resolving one needs the config file, so it
    /// cannot happen here.
    pub(crate) fn builtin(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mainnet" => Some(Self::Mainnet),
            "testnet" => Some(Self::Testnet),
            "local" => Some(Self::Local),
            _ => None,
        }
    }

    /// clap's value parser for `--network`/`NEXUS_NETWORK`.
    ///
    /// Rejects a retired release-channel name here, at parse time, so no
    /// invocation can keep using a name whose meaning was wrong — and rejects a
    /// label that could not be a credential key, so a bad one fails before it is
    /// ever used to select anything. What is *not* checked here is whether the
    /// label names a declared stage: that needs the config file, which the parser
    /// has no access to, and [`Cli::target`] reports it.
    fn from_flag(s: &str) -> Result<Self, String> {
        if let Some(builtin) = Self::builtin(s) {
            return Ok(builtin);
        }
        if let Some(replacement) = Self::retired_replacement(s) {
            return Err(format!(
                "`{s}` was a release channel, not a network, and no longer exists; it named a \
                 play-funds host, which is now `{replacement}`. Valid networks: {NETWORK_VALUES}"
            ));
        }
        let label = validate_label(s).map_err(|why| {
            format!("`{s}` is not a usable network. Valid networks: {NETWORK_VALUES}. {why}")
        })?;
        Ok(Self::Custom(label))
    }

    /// The current name for a retired release-channel value, if `s` is one.
    /// Used only to turn a stale config into an actionable warning.
    pub(crate) fn retired_replacement(s: &str) -> Option<&'static str> {
        let name = s.trim().to_ascii_lowercase();
        RETIRED_NETWORKS
            .iter()
            .find(|(retired, _)| *retired == name)
            .map(|(_, replacement)| *replacement)
    }
}

/// How command results are rendered to stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable, aligned tables (the default).
    Human,
    /// Pretty-printed JSON.
    Json,
}

/// Order side. Maps onto the SDK's [`Side`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SideArg {
    Buy,
    Sell,
}

impl From<SideArg> for Side {
    fn from(s: SideArg) -> Self {
        match s {
            SideArg::Buy => Side::Buy,
            SideArg::Sell => Side::Sell,
        }
    }
}

/// Order type. Maps onto the SDK's [`OrderType`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderTypeArg {
    Limit,
    Market,
}

impl From<OrderTypeArg> for OrderType {
    fn from(t: OrderTypeArg) -> Self {
        match t {
            OrderTypeArg::Limit => OrderType::Limit,
            OrderTypeArg::Market => OrderType::Market,
        }
    }
}

/// Time in force. Maps onto the SDK's [`TimeInForce`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TifArg {
    /// Good-til-cancelled.
    Gtc,
    /// Immediate-or-cancel.
    Ioc,
    /// Fill-or-kill.
    Fok,
    /// Post-only: rest or be rejected, never cross the spread.
    ///
    /// Spelled `post-only` on the command line (clap kebab-cases the variant).
    /// Post-only is a time-in-force on this API rather than a boolean flag, so
    /// it belongs here rather than as `--post-only` — which is what made it
    /// look absent: the capability is in the spec and the engine, and only the
    /// CLI could not reach it.
    PostOnly,
}

impl From<TifArg> for TimeInForce {
    fn from(t: TifArg) -> Self {
        match t {
            TifArg::Gtc => TimeInForce::Gtc,
            TifArg::Ioc => TimeInForce::Ioc,
            TifArg::Fok => TimeInForce::Fok,
            TifArg::PostOnly => TimeInForce::PostOnly,
        }
    }
}

/// The `nexus-exchange` release whose `Network` axis the mapping below was read
/// against.
///
/// This constant is the signal, because the `match` cannot be. `Network` is
/// `#[non_exhaustive]`, so a variant added upstream compiles straight through a
/// mapping that only matches on the CLI's own enum — and `#[non_exhaustive]` also
/// forbids a downstream crate from matching `Network` exhaustively, so there is
/// no compile-time guard to be had. Pinning the version instead means an SDK bump
/// goes red until someone re-reads the axis.
///
/// Transcribing an upstream fact with nothing to notice when upstream moves is
/// what pointed the real-funds guard at the wrong network to begin with
/// (ENG-6452), so the transcription is pinned here rather than trusted.
///
/// Checked by `the_network_axis_was_checked_against_the_pinned_sdk`, hence
/// `#[cfg(test)]`: it is a claim about the mapping below, not a value the binary
/// has any use for at runtime.
#[cfg(test)]
const NETWORK_AXIS_VERIFIED_AGAINST: &str = "0.9.0";

impl NetworkArg {
    /// The SDK network for a built-in variant, or `None` for a custom label,
    /// whose bundle lives in the config file (see
    /// [`CustomNetworkConfig::to_network`]).
    ///
    /// Exhaustive over `NetworkArg` — the CLI's own enum — so adding a value
    /// there without mapping it is a compile error. It is deliberately NOT a
    /// claim about the SDK's axis growing: see [`NETWORK_AXIS_VERIFIED_AGAINST`],
    /// which is what notices that.
    pub(crate) fn builtin_network(&self) -> Option<Network> {
        match self {
            NetworkArg::Mainnet => Some(Network::Mainnet),
            NetworkArg::Testnet => Some(Network::Testnet),
            NetworkArg::Local => Some(Network::Local),
            NetworkArg::Custom(_) => None,
        }
    }
}

/// A stage declared under `custom_networks` in the config file: the CLI's
/// transcription of [`CustomNetwork`], which is the whole safety bundle rather
/// than an address (ENG-9827).
///
/// # Every field is optional *here*, and required *there*
///
/// Not because any of them may be omitted, but because a malformed entry must
/// not stop the config file from parsing: the file also holds credentials, and
/// failing the whole load would break every command over a mistake in one stage
/// nobody selected. Each field is therefore checked in [`to_network`], when the
/// stage this describes is actually the one being used.
///
/// [`to_network`]: Self::to_network
#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomNetworkConfig {
    /// REST base URL. Required; validated by the SDK (scheme, host, no
    /// userinfo/query/fragment).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Base for the direct `/api/v1` surface, when the deployment splits it from
    /// the REST base. Defaults to `base_url`, which is where every deployment
    /// that exists today mounts it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct_base_url: Option<String>,
    /// `"real"`, `"play"` or `"unknown"`. Required, with no default: see
    /// [`parse_funds`] for why neither boolean answer is safe.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub funds: Option<String>,
    /// WebSocket origin (`ws://` or `wss://`). Absent until declared: it is a
    /// separate host from the REST base and is never derived from it, so `nexus
    /// ws` refuses rather than guessing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_url: Option<String>,
    /// Whether the synthetic faucet exists here. Assumed **absent** until
    /// declared, so `account credit` cannot route to a faucet that is not there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faucet: Option<bool>,
    /// EIP-712 domain chain id, read from this host's `GET /metadata`. Absent
    /// until declared — a signature made under the wrong domain may be *valid on
    /// a different network*, so it is never guessed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<u64>,
}

/// Read a config-file `funds` value, together with the diagnostic for one that
/// cannot be read.
///
/// Fails **closed**: an absent or unrecognized value becomes [`Funds::Unknown`],
/// which refuses the real-funds-guarded commands rather than assuming play
/// funds. Both spellings of the mistake land there — `"funds": "reel"` is as
/// likely a typo for `real` as for `play`, and only one of those guesses costs
/// money.
///
/// A warning rather than a hard error, for the same reason
/// [`stale_network_warning`] is one: reads against the stage still work, so
/// refusing the whole invocation would be a bigger hammer than the mistake
/// warrants. What it must not do is stay silent — an undeclared classification
/// that quietly behaved like play funds is precisely the bug ENG-9823 exists to
/// remove.
fn parse_funds(label: &str, declared: Option<&str>) -> (Funds, Option<String>) {
    // `{declared:?}`/`{label:?}` throughout: both come from a file and are
    // echoed to a terminal, and `Debug` escapes control bytes.
    match declared.map(str::trim) {
        Some("real") => (Funds::Real, None),
        Some("play") => (Funds::Play, None),
        Some("unknown") => (Funds::Unknown, None),
        None | Some("") => (
            Funds::Unknown,
            Some(format!(
                "warning: custom network {label:?} does not declare \"funds\"; treating it as \
                 unknown, so commands that move or mint funds are refused. Set \"funds\" to \
                 \"real\", \"play\" or \"unknown\" in the config."
            )),
        ),
        Some(other) => (
            Funds::Unknown,
            Some(format!(
                "warning: custom network {label:?} declares \"funds\": {other:?}, which is not \
                 \"real\", \"play\" or \"unknown\"; treating it as unknown, so commands that move \
                 or mint funds are refused."
            )),
        ),
    }
}

impl CustomNetworkConfig {
    /// Build the SDK target this describes, with the diagnostic for a `funds`
    /// value that could not be read.
    ///
    /// Everything the SDK validates is left to the SDK: the label as a credential
    /// key, and each URL's scheme, host, userinfo, query and fragment. This adds
    /// only what the SDK cannot see — that the entry declared a base URL at all.
    ///
    /// Note what is deliberately absent: no hostname is inspected, defaulted or
    /// interpolated anywhere on this path. The URL is the caller's, verbatim.
    pub fn to_network(&self, label: &str) -> Result<(Network, Option<String>)> {
        let base_url = self
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "custom network {label:?} declares no \"base_url\"; a custom network is a \
                     caller-supplied deployment, so there is no host to fall back to"
                )
            })?;

        let (funds, warning) = parse_funds(label, self.funds.as_deref());
        let mut custom = CustomNetwork::new(label, base_url, funds)
            .map_err(|e| anyhow::anyhow!("custom network {label:?}: {e}"))?;
        if let Some(direct) = self.direct_base_url.as_deref() {
            custom = custom
                .with_direct_base_url(direct)
                .map_err(|e| anyhow::anyhow!("custom network {label:?}: {e}"))?;
        }
        if let Some(ws) = self.ws_url.as_deref() {
            custom = custom
                .with_ws_url(ws)
                .map_err(|e| anyhow::anyhow!("custom network {label:?}: {e}"))?;
        }
        // Absent means absent, not false-by-omission — but the SDK's default is
        // already "no faucet", so declaring it explicitly here changes nothing
        // and keeps the mapping one-to-one with the file.
        custom = custom.with_faucet(self.faucet.unwrap_or(false));
        if let Some(chain_id) = self.chain_id {
            custom = custom.with_signing_domain(SigningDomain::new(chain_id));
        }
        Ok((Network::Custom(custom), warning))
    }
}

/// Where one invocation is pointed, and what that place moves.
///
/// Resolved once, in [`Cli::target`], and then read by everything that needs to
/// know: the SDK [`Config`], the credential namespace, and every guardrail. One
/// resolution rather than three means the network the request goes to, the key it
/// presents, and the guard that vets it cannot disagree — they did not use to be
/// derived from the same place, and `--base-url` is exactly where they diverged.
#[derive(Debug, Clone)]
pub struct Target {
    /// The **declared** target: a built-in network, or a stage from
    /// `custom_networks`. Not necessarily where the request goes — see
    /// [`base_url_override`](Self::base_url_override).
    network: Network,
    /// A `--base-url`/`NEXUS_BASE_URL`/config-file `base_url` redirect, if one is
    /// in effect.
    base_url_override: Option<String>,
}

impl Target {
    /// The declared target.
    pub fn network(&self) -> &Network {
        &self.network
    }

    /// The key this invocation's stored credentials are namespaced under.
    ///
    /// Deliberately **not** affected by `--base-url`. A base-URL override
    /// changes where the request goes, not who you are — pointing at a proxy, a
    /// tunnel or a staging host in front of a network should still present that
    /// network's key, and an override carries no label to namespace by anyway.
    /// So the namespace is always well-defined, and `--base-url` callers keep the
    /// credentials they had before namespacing existed.
    ///
    /// For a custom stage this is its **label**, not its URL, which is the point:
    /// two stages sharing a host still get separate credential slots, and one
    /// stage keeps its slot across a host move.
    pub fn namespace(&self) -> &str {
        self.network.label()
    }

    /// The base-URL override in effect, if any.
    pub fn base_url_override(&self) -> Option<&str> {
        self.base_url_override.as_deref()
    }

    /// What the **destination** moves.
    ///
    /// `Unknown` whenever a base-URL override is in effect, matching what the SDK
    /// reports for the same config. Answering with the named network's funds here
    /// would be the exact lie the bundle exists to stop: an override changes the
    /// destination, and it is the destination whose funds are at stake. A bare
    /// URL says nothing about what its host moves, so the honest answer is that
    /// nobody declared it — and `Unknown` fails closed.
    pub fn funds(&self) -> Funds {
        match self.base_url_override {
            Some(_) => Funds::Unknown,
            None => self.network.funds(),
        }
    }

    /// What the **named** target moves, ignoring any override. This is the
    /// question to ask about the credentials, since they are namespaced by the
    /// named target rather than by the destination.
    pub fn credential_funds(&self) -> Funds {
        self.network.funds()
    }

    /// Whether anything here touches real money — either the destination is a
    /// declared real-funds target, or the key about to be presented belongs to
    /// one.
    ///
    /// The union is deliberate: an override that redirects a *real-funds
    /// credential* somewhere unclassified is not made safe by the destination
    /// being unclassified, and neither half alone would have caught both.
    pub fn touches_real_funds(&self) -> bool {
        matches!(self.funds(), Funds::Real) || matches!(self.credential_funds(), Funds::Real)
    }

    /// Whether the synthetic faucet exists here. Always `false` under an
    /// override: the flag belongs to the declared target, and the request would
    /// go somewhere else.
    pub fn has_faucet(&self) -> bool {
        self.base_url_override.is_none() && self.network.has_faucet()
    }

    /// The EIP-712 domain chain id to sign an agent registration under.
    ///
    /// A custom network's declared `chain_id` when it has one, and
    /// [`DEFAULT_CHAIN_ID`] otherwise — which is what every built-in network
    /// falls back to, since the SDK publishes no `chain_id` for them (it is
    /// server-authoritative, read from `/metadata`).
    ///
    /// Not applied under a base-URL override: the domain belongs to the declared
    /// target and the registration would be signed for a different host.
    pub fn signing_chain_id(&self) -> u64 {
        if self.base_url_override.is_some() {
            return DEFAULT_CHAIN_ID;
        }
        self.network
            .signing_domain()
            .and_then(|domain| domain.chain_id)
            .unwrap_or(DEFAULT_CHAIN_ID)
    }
}

/// EIP-712 domain chain id used when the selected target declares none — the
/// exchange's own chain, and the value `agents register` has always defaulted to.
///
/// Kept as a fallback rather than a refusal because every built-in network
/// reaches it: the SDK deliberately publishes no `chain_id` for them, so
/// refusing here would break `agents register` on testnet. A custom network can
/// say better, and [`Target::signing_chain_id`] prefers what it says.
pub(crate) const DEFAULT_CHAIN_ID: u64 = 393;

/// Whether `name` selects a network against `file`: a built-in name, or a label
/// the file declares under `custom_networks`. `None` for anything else — a
/// retired channel, a typo, or a label with no declaration.
///
/// The declaration check is what makes a label different from a typo. `mainet`
/// and `dev` are the same shape, and only the file can say which of them names
/// something.
pub(crate) fn selectable_network(name: &str, file: &FileConfig) -> Option<NetworkArg> {
    let name = name.trim();
    if let Some(builtin) = NetworkArg::builtin(name) {
        return Some(builtin);
    }
    // Declared *and* usable as a credential key. A declaration under a label the
    // SDK would refuse is not a network this build can select, so it must not
    // become one here either — `Cli::target` reports it rather than silently
    // resolving it.
    let label = validate_label(name).ok()?;
    file.custom_networks
        .contains_key(&label)
        .then_some(NetworkArg::Custom(label))
}

/// Which network a config file names, as this build resolves it.
///
/// Shared by [`Cli::target`] and the legacy-credential migration so the two
/// cannot disagree about which network a file belongs to; they answer different
/// questions about the same string, and a file whose credentials migrate to one
/// network while its requests go to another is the failure that would follow.
pub(crate) fn declared_network(file: &FileConfig) -> Option<NetworkArg> {
    selectable_network(file.network.as_deref()?, file)
}

/// The diagnostic for a `custom_networks` entry that claims a built-in
/// network's name, or `None` when there is nothing to report.
///
/// The built-in wins, which is the safe resolution — `mainnet` keeps meaning
/// mainnet, and the declaration cannot capture its credential slot. But it must
/// not win *silently*: the user wrote a stage and would otherwise be pointed at
/// a different host than the one they described, with the entry they can see in
/// their own config file offering no hint as to why.
///
/// The SDK refuses these labels for the same reason, one level down — see
/// [`validate_label`] — so this reports a declaration that could never have been
/// selected rather than adding a rule of its own.
fn shadowed_declaration_warning(name: &str, file: &FileConfig) -> Option<String> {
    file.custom_networks.contains_key(name).then(|| {
        format!(
            "warning: the config file declares a custom network named {name:?}, which is a \
             built-in network's own name and is therefore ignored — per-network credentials are \
             stored under that name, so a stage may not claim it. Using the built-in {name:?}. \
             Rename the entry under \"custom_networks\" to select it."
        )
    })
}

/// What a selected network moves, without building a [`Config`] for it.
///
/// For a custom stage this is the declared classification, defaulting closed to
/// [`Funds::Unknown`] when the declaration is missing or unreadable — the same
/// answer [`CustomNetworkConfig::to_network`] reaches, arrived at without the
/// URL validation this caller has no use for.
pub(crate) fn declared_funds(selected: &NetworkArg, file: &FileConfig) -> Funds {
    match selected.builtin_network() {
        Some(builtin) => builtin.funds(),
        None => file
            .custom_networks
            .get(selected.name())
            .map(|declared| parse_funds(selected.name(), declared.funds.as_deref()).0)
            .unwrap_or(Funds::Unknown),
    }
}

/// Turn a selected [`NetworkArg`] into an SDK network, reporting a `funds` value
/// that could not be read.
///
/// A free function rather than a method: it reads only the selection and the
/// file, so nothing else about the invocation can influence where it lands.
fn resolve_selection(selected: &NetworkArg, file: &FileConfig) -> Result<Network> {
    if let Some(builtin) = selected.builtin_network() {
        if let Some(warning) = shadowed_declaration_warning(selected.name(), file) {
            eprintln!("{warning}");
        }
        return Ok(builtin);
    }
    let label = selected.name();
    let Some(declared) = file.custom_networks.get(label) else {
        // `{key:?}` on every declared name: they come from a file and are
        // echoed to a terminal, and `Debug` escapes control bytes, so a
        // config cannot smuggle ESC sequences into this list.
        let mut known: Vec<String> = file
            .custom_networks
            .keys()
            .map(|key| format!("{key:?}"))
            .collect();
        known.sort();
        let declared = if known.is_empty() {
            "the config file declares none".to_string()
        } else {
            format!("declared: {}", known.join(", "))
        };
        bail!(
            "no custom network named {label:?} is declared ({declared}). Add it under \
             \"custom_networks\" in the config file with a \"base_url\" and a \"funds\" value \
             of \"real\", \"play\" or \"unknown\"."
        );
    };
    let (network, warning) = declared.to_network(label)?;
    if let Some(warning) = warning {
        eprintln!("{warning}");
    }
    Ok(network)
}

impl Cli {
    /// Resolve where this invocation is pointed, layering: `--network`/env >
    /// config-file `network` > the SDK default (testnet — play funds; the default
    /// must never be a real-funds network). A base-URL override from either
    /// `--base-url`/env or the config file is recorded on top, redirecting the
    /// request without changing which network's credentials it presents.
    ///
    /// # What errors, and what falls back
    ///
    /// A config-file `network` this build cannot **resolve at all** — a retired
    /// channel name, a typo, a label nothing declares — warns and falls back to
    /// the default. The default is play funds, so the request is safe to make,
    /// and it is the stale name that needs fixing rather than this invocation.
    ///
    /// Everything else errors. A `--network` typed on this command line is
    /// honoured or refused, never quietly redirected. And a label that *is*
    /// declared but declared badly — no base URL, a URL the SDK refuses — errors
    /// from either source: the stage exists, so falling back would send the
    /// request to testnet while the user believes they are on their own
    /// deployment. That is worse than the stale-name case, where nothing was
    /// described at all.
    ///
    /// What it never does is fall back *silently* — that is the failure class
    /// ENG-6455 removed, and the typo case is the durable one (`stable`/`beta`
    /// age out of configs; misspellings never will).
    pub fn target(&self, file: &FileConfig) -> Result<Target> {
        let network = match &self.network {
            // Typed on this command line: honour it or fail. Falling back here
            // would send the request somewhere the user did not ask for.
            Some(selected) => resolve_selection(selected, file)?,
            None => match declared_network(file) {
                Some(selected) => resolve_selection(&selected, file)?,
                None => {
                    let fallback = Config::default().network().clone();
                    if let Some(name) = file.network.as_deref() {
                        if let Some(warning) = stale_network_warning(name, fallback.label()) {
                            eprintln!("{warning}");
                        }
                    }
                    fallback
                }
            },
        };
        Ok(Target {
            network,
            base_url_override: self.base_url.clone().or_else(|| file.base_url.clone()),
        })
    }

    /// Resolve the SDK [`Config`] for a [`Target`]. Every resolved config carries
    /// the CLI's [`USER_AGENT`].
    ///
    /// A base-URL override still goes through [`Config::with_base_url`], which is
    /// now itself sugar for a custom network with undeclared funds — so both
    /// paths build the same shape and there is no second code path to drift.
    pub fn config(&self, target: &Target) -> Config {
        let config = match target.base_url_override() {
            Some(url) => Config::with_base_url(url),
            None => Config::new(target.network().clone()),
        };
        config.with_user_agent(USER_AGENT)
    }
    /// Resolve an API key/secret pair, layering flags/env over the config file's
    /// section for the resolved target. Returns `None` when no usable pair is
    /// configured. Warns (and still returns `None`) when only one half is
    /// present, since that is almost always a mistake.
    ///
    /// Flags and env are **not** namespaced: they are a per-invocation override
    /// the user has just typed for the network they just selected, so scoping
    /// them would mean inventing `NEXUS_TESTNET_API_KEY`-style variables for no
    /// gain. Only the persisted layer — the one that outlives the invocation and
    /// can hold several networks at once — is sectioned.
    ///
    /// The pair is handed to [`Config::api_key`] so the SDK signs authenticated
    /// requests; the CLI never touches the secret beyond passing it through.
    pub fn credentials(&self, file: &FileConfig, target: &Target) -> Option<(String, String)> {
        let stored = file.credentials_for(target.namespace());
        let key = self
            .credentials
            .api_key
            .clone()
            .or_else(|| stored.and_then(|c| c.api_key.clone()));
        let secret = self
            .credentials
            .api_secret
            .clone()
            .or_else(|| stored.and_then(|c| c.api_secret.clone()));

        match (key, secret) {
            (Some(k), Some(s)) => Some((k, s)),
            (Some(_), None) => {
                eprintln!(
                    "warning: API key set without a matching API secret; requests will be unsigned"
                );
                None
            }
            (None, Some(_)) => {
                eprintln!(
                    "warning: API secret set without a matching API key; requests will be unsigned"
                );
                None
            }
            (None, None) => None,
        }
    }

    /// Resolve a wallet session token, layering flag/env over the config file's
    /// section for the resolved target (the same precedence, and the same
    /// namespacing, as the HMAC pair). Returns `None` when none is configured.
    /// Handed to [`Config::session_token`] only when no HMAC key pair is
    /// present, so the HMAC pair takes precedence as the request signer.
    pub fn session_token(&self, file: &FileConfig, target: &Target) -> Option<String> {
        self.credentials.session_token.clone().or_else(|| {
            file.credentials_for(target.namespace())
                .and_then(|c| c.session_token.clone())
        })
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List all tradable markets and their trading rules.
    Markets,

    /// Per-market data: summaries, lifecycle status, mark price.
    Market {
        #[command(subcommand)]
        action: MarketCommand,
    },

    /// Fetch the ticker for a single market, e.g. `BTC-USDX-PERP`.
    Ticker {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
    },

    /// List tickers for every market.
    Tickers,

    /// List per-market 24h summaries (mark price, volume, status).
    Summaries,

    /// Show the current mark price for a market.
    MarkPrice {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
    },

    /// Show the lifecycle/halt status for a market.
    MarketStatus {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
    },

    /// Show the funding-rate history for a market.
    FundingRates {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
        /// Maximum number of samples to return.
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },

    /// Show the order book (bids/asks) for a market.
    Orderbook {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
    },

    /// Show recent trades for a market.
    Trades {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
        /// Maximum number of trades to return.
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },

    /// Show OHLCV candles for a market.
    Candles {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
        /// Candle interval.
        #[arg(long, default_value = "1m")]
        timeframe: String,
        /// Maximum number of candles to return.
        #[arg(long, default_value_t = 200)]
        limit: u32,
    },

    /// Show the aggregate service health snapshot (`GET /status`).
    Health,

    /// Show your account summary (balance, collateral, equity, margin).
    Balance,

    /// List your open positions.
    Positions,

    /// List your recent fills (executions).
    Fills {
        /// Maximum number of fills to return (the server's page size, 1..=1000).
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=MAX_FILLS_LIMIT as i64))]
        limit: u32,
    },

    /// List your withdrawal history.
    Withdrawals {
        /// Maximum number of withdrawals to return.
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },

    /// List your open orders.
    Orders,

    /// Your funding payments (perp funding booked against the account).
    FundingPayments {
        /// Maximum number of payments to return.
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },

    /// Place, amend, cancel, or fetch orders.
    Order {
        #[command(subcommand)]
        action: OrderCommand,
    },

    /// Manage account settings (deposit, credit, leverage, rate-limit).
    Account {
        #[command(subcommand)]
        action: AccountCommand,
    },

    /// Wallet-signed authentication (EIP-191 sign-in for a session token).
    Auth {
        #[command(subcommand)]
        action: AuthCommand,
    },

    /// Manage HMAC API keys (list/create/delete).
    Keys {
        #[command(subcommand)]
        action: KeysCommand,
    },

    /// Manage registered agent keys (list/revoke).
    Agents {
        #[command(subcommand)]
        action: AgentsCommand,
    },

    /// Manage collateral transfers (list/create).
    Transfers {
        #[command(subcommand)]
        action: TransfersCommand,
    },

    /// Manage sub-accounts (list/create).
    SubAccounts {
        #[command(subcommand)]
        action: SubAccountsCommand,
    },

    /// Stream live data over WebSocket. Public channels (`trades`, `book`,
    /// `candles`) need `--market`; account channels (`orders`, `fills`,
    /// `positions`, `balances`) are scoped to your key.
    Ws {
        /// One or more channels to subscribe to.
        #[arg(required = true, num_args = 1..)]
        channels: Vec<String>,
        /// Market for public channels, e.g. `BTC-USDX-PERP`.
        #[arg(long)]
        market: Option<String>,
        /// Resume from this sequence number (per channel).
        #[arg(long)]
        since: Option<i64>,
    },

    /// Interactively configure network and credentials.
    Setup,

    /// Print shell-completion script to stdout.
    Completions {
        /// Target shell.
        shell: Shell,
    },
}

#[derive(Debug, Subcommand)]
pub enum MarketCommand {
    /// Per-market summaries with 24h volume and halt state.
    Summary,

    /// Show the lifecycle/halt status for a single market.
    Status {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
    },

    /// Show the current mark price for a single market.
    MarkPrice {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum OrderCommand {
    /// Submit a new order.
    Place {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        #[arg(long)]
        market: String,
        /// Order side.
        #[arg(long, value_enum)]
        side: SideArg,
        /// Order type.
        #[arg(long = "type", value_enum)]
        order_type: OrderTypeArg,
        /// Limit price (required for `--type limit`).
        #[arg(long)]
        price: Option<String>,
        /// Order quantity (base units).
        #[arg(long)]
        quantity: String,
        /// Time in force.
        #[arg(long, value_enum, default_value_t = TifArg::Gtc)]
        tif: TifArg,
        /// Only reduce an existing position; never open or flip one.
        #[arg(long)]
        reduce_only: bool,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },

    /// Cancel a single order by id (requires `--market`), or all open orders
    /// with `--all`.
    Cancel {
        /// Order id to cancel. Requires `--market` (by-id cancels are routed
        /// per market).
        #[arg(requires = "market")]
        order_id: Option<String>,
        /// Market the order is on, e.g. `BTC-USDX-PERP`. Required when
        /// cancelling a single order: the engine routes by-id cancels per
        /// market. Not used with `--all`.
        #[arg(long, conflicts_with = "all")]
        market: Option<String>,
        /// Cancel all open orders.
        #[arg(long, conflicts_with = "order_id")]
        all: bool,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },

    /// Fetch a single order by id.
    Get {
        /// Order id.
        order_id: String,
        /// Market the order is on, e.g. `BTC-USDX-PERP`. Required: the engine
        /// routes by-id lookups per market.
        #[arg(long)]
        market: String,
    },

    /// Amend an open order in place (atomic cancel-replace). Set only the
    /// fields you want to change.
    Amend {
        /// Order id to amend.
        order_id: String,
        /// Market the order is on, e.g. `BTC-USDX-PERP`. Required: the engine
        /// routes by-id amends per market.
        #[arg(long)]
        market: String,
        /// New limit price.
        #[arg(long)]
        price: Option<String>,
        /// New order quantity (base units).
        #[arg(long)]
        quantity: Option<String>,
        /// New time in force.
        #[arg(long, value_enum)]
        tif: Option<TifArg>,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },

    /// Submit a batch of orders from a JSON file (an array of order objects),
    /// or `-` to read the array from stdin.
    Batch {
        /// Path to a JSON file containing an array of order requests, or `-`
        /// for stdin.
        file: String,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum AccountCommand {
    /// Portfolio summary: equity, PnL, 24h volume, open counts, margin, and the
    /// withdrawable balance.
    ///
    /// The server reports every field optionally; an absent one renders `-`
    /// (JSON `null`) and never `0` — "not reported" is not "zero".
    Summary,

    /// Consolidated account snapshot: the portfolio summary *and* every open
    /// position, from one coherent server-side read.
    ///
    /// Prefer this over running `account summary` and `positions` separately:
    /// those are two independent requests, so a fill landing between them
    /// returns an aggregate that disagrees with the position list.
    State,

    /// Effective fee schedule for the account (maker/taker bps, tier, 30d
    /// volume). A negative maker fee is a rebate paid to you.
    Fees,

    /// Portfolio time series: equity, cumulative PnL, and cumulative traded
    /// volume, oldest first.
    PortfolioHistory {
        /// Window to report over. Also fixes the server-side sample cadence and
        /// point capacity (day 5m/288, week 1h/168, month 6h/120, all 1d/366).
        /// Omit for the server's `day` default.
        #[arg(long, value_enum)]
        window: Option<PortfolioWindowArg>,
        /// Maximum number of points to return. The API schema allows 1..=366
        /// (the widest window's capacity); the server additionally clamps to the
        /// selected window's own capacity, so fewer points may come back.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=MAX_PORTFOLIO_HISTORY_LIMIT as i64))]
        limit: Option<u32>,
    },

    /// Deposit collateral into the account.
    Deposit {
        /// Amount to deposit (quote asset).
        amount: String,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },

    /// Claim synthetic (testnet) USDX credit from the faucet. Omit `--amount`
    /// to claim the full remaining daily allowance.
    Credit {
        /// Amount to claim; defaults to the remaining daily allowance.
        #[arg(long)]
        amount: Option<String>,
    },

    /// Show the caller's rate-limit status.
    RateLimit,

    /// Set the leverage for a market.
    Leverage {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
        /// Leverage multiplier (e.g. 10 for 10x). Must be at least 1.
        leverage: u32,
    },
    // There is deliberately no `margin-mode` subcommand: no endpoint backs one.
    // `margin_mode` appears nowhere in the pinned spec, and the only isolated-
    // margin route (`POST /account/margin`) requires a position that is already
    // isolated. It was withdrawn in ENG-7740; ENG-7614 tracks the engine work
    // that has to land before it can come back. Don't re-add it against a guessed
    // request shape — add it when the spec defines one.
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Sign in with an EVM wallet (EIP-191) and store the session token.
    ///
    /// The raw private key is read from `--private-key`, the
    /// `NEXUS_PRIVATE_KEY` environment variable, or — when neither is set and
    /// stdin is a terminal — a hidden interactive prompt. It is used only to
    /// produce the sign-in signature and is never written to disk or echoed.
    Login {
        /// Raw EVM private key (`0x`-prefix optional). Prefer the env var or the
        /// hidden prompt over the flag, which is visible in your shell history
        /// and process list.
        #[arg(long, env = "NEXUS_PRIVATE_KEY", hide_env_values = true)]
        private_key: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum KeysCommand {
    /// List the API keys on the authenticated session.
    List,
    /// Create a new API key. The secret is shown once — store it immediately.
    Create {
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },
    /// Delete an API key by id.
    Delete {
        /// Key id to delete.
        key_id: String,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum AgentsCommand {
    /// List registered agent keys for the authenticated wallet.
    List,
    /// Register an agent key, authorized by an EIP-712 signature from the
    /// owning wallet. The wallet's raw private key produces the signature and
    /// is never written to disk or echoed; the request itself is unauthenticated
    /// (the signature is the authorization), so no API key or session token is
    /// required.
    Register {
        /// Agent address to authorize (`0x`-prefixed, 20 bytes).
        #[arg(long)]
        agent: String,
        /// Owning wallet's raw EVM private key (`0x`-prefix optional). Prefer the
        /// env var or the hidden prompt over the flag, which is visible in your
        /// shell history and process list.
        #[arg(long, env = "NEXUS_PRIVATE_KEY", hide_env_values = true)]
        private_key: Option<String>,
        /// Authorization expiry, Unix milliseconds. The spec expects expiry in
        /// `[now+1d, now+90d]`; defaults to 30 days from now when omitted.
        #[arg(long)]
        expires_at: Option<u64>,
        /// Monotonic nonce; defaults to the current Unix-ms timestamp (a safe
        /// starting value, per the spec).
        #[arg(long)]
        nonce: Option<u64>,
        /// EIP-712 domain chain id (the exchange's chain id). Part of the signed
        /// payload, so it must match what the server verifies against.
        ///
        /// Defaults to the selected network's declared signing domain — a custom
        /// network's `chain_id` — and to [`DEFAULT_CHAIN_ID`] when it declares
        /// none. Read it off the target's `GET /metadata` rather than assuming:
        /// a signature made under the wrong domain either fails verification or,
        /// worse, is *valid on a different network*.
        #[arg(long)]
        chain_id: Option<u64>,
        /// Optional human-readable label for the agent.
        #[arg(long)]
        label: Option<String>,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },
    /// Revoke a registered agent by address.
    Revoke {
        /// Agent address (0x-prefixed).
        address: String,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum TransfersCommand {
    /// List collateral transfers.
    List,
    /// Create a transfer between accounts (e.g. to/from a sub-account).
    Create {
        /// Source account id to debit.
        #[arg(long)]
        from: String,
        /// Destination account id to credit.
        #[arg(long)]
        to: String,
        /// Amount of collateral to move; must be positive.
        #[arg(long)]
        amount: String,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum SubAccountsCommand {
    /// List sub-accounts of the authenticated master account.
    List,
    /// Create a new sub-account with a label.
    Create {
        /// Human-readable label for the sub-account.
        label: String,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },
}

/// Window for the portfolio time series. Maps onto the SDK's
/// [`PortfolioWindow`], which is the closed set of values the API accepts — so
/// the `window` query parameter can only ever carry one of these four, never
/// caller-shaped text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PortfolioWindowArg {
    /// Trailing 24 hours, sampled every 5 minutes (the server's default).
    Day,
    /// Trailing 7 days, sampled hourly.
    Week,
    /// Trailing 30 days, sampled every 6 hours.
    Month,
    /// Full retained history (~1 year), sampled daily.
    All,
}

impl From<PortfolioWindowArg> for PortfolioWindow {
    fn from(w: PortfolioWindowArg) -> Self {
        match w {
            PortfolioWindowArg::Day => PortfolioWindow::Day,
            PortfolioWindowArg::Week => PortfolioWindow::Week,
            PortfolioWindowArg::Month => PortfolioWindow::Month,
            PortfolioWindowArg::All => PortfolioWindow::All,
        }
    }
}

// `MarginModeArg` and its `From<MarginModeArg> for MarginMode` impl stood
// here and are removed with the command they existed for (ENG-7740): the
// API has no margin-mode endpoint, so the arg had nothing to map onto.
// `PortfolioWindowArg` above is unrelated and stays.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::NetworkCredentials;
    use clap::CommandFactory;
    use nexus_exchange::Client;

    /// Resolve a target against an empty config file. Panics on a selection the
    /// file cannot honour — the tests that exercise those assert on the error.
    fn target(cli: &Cli) -> Target {
        cli.target(&FileConfig::default())
            .expect("a resolvable target")
    }

    fn base_url_with(cli: &Cli, file: &FileConfig) -> String {
        let target = cli.target(file).expect("a resolvable target");
        Client::new(cli.config(&target)).base_url().to_string()
    }

    fn base_url(cli: &Cli) -> String {
        base_url_with(cli, &FileConfig::default())
    }

    /// A play-funds stage with a faucet, declared the way a config file would.
    /// `example.com` is RFC 2606 reserved, so no real deployment is named here.
    fn declared_stage(funds: &str) -> CustomNetworkConfig {
        CustomNetworkConfig {
            base_url: Some("https://exchange.example.com/api/exchange".into()),
            funds: Some(funds.into()),
            faucet: Some(true),
            ..Default::default()
        }
    }

    /// A config file declaring one custom stage under `label`.
    fn file_declaring(label: &str, declared: CustomNetworkConfig) -> FileConfig {
        let mut file = FileConfig::default();
        file.custom_networks.insert(label.to_string(), declared);
        file
    }

    /// Catches conflicting flags, bad arg specs, etc. at test time.
    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults_to_testnet_network() {
        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        assert_eq!(cli.network, None);
        assert_eq!(base_url(&cli), Network::Testnet.base_url());
    }

    /// The invariant behind the default, not just its current value: omitting
    /// `--network` must never reach a network that moves real money.
    #[test]
    fn the_default_network_is_not_real_funds() {
        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        let funds = target(&cli).funds();
        assert_eq!(
            funds,
            Funds::Play,
            "the default network must be *known* play funds, got {funds:?}"
        );
    }

    #[test]
    fn base_url_overrides_network() {
        let cli = Cli::try_parse_from([
            "nexus",
            "--network",
            "testnet",
            "--base-url",
            "http://x:1",
            "health",
        ])
        .unwrap();
        assert_eq!(base_url(&cli), "http://x:1");
    }

    #[test]
    fn config_file_is_a_fallback_below_flags() {
        let file = FileConfig {
            network: Some("local".into()),
            ..Default::default()
        };
        // No flag → file network wins.
        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        assert_eq!(base_url_with(&cli, &file), Network::Local.base_url());
        // Flag beats the file.
        let cli = Cli::try_parse_from(["nexus", "--network", "testnet", "markets"]).unwrap();
        assert_eq!(base_url_with(&cli, &file), Network::Testnet.base_url());
    }

    /// ENG-6455. `stable`/`beta` were release channels, not networks; the axis is
    /// now the spec's. Rejected at parse time so no invocation can keep using a
    /// name whose meaning was wrong.
    #[test]
    fn retired_release_channel_names_are_rejected_by_the_flag() {
        for retired in ["stable", "beta"] {
            let parsed = Cli::try_parse_from(["nexus", "--network", retired, "markets"]);
            assert!(
                parsed.is_err(),
                "--network {retired} must not parse; it is not a network"
            );
            // clap names the values that *are* valid, which is the migration hint.
            let rendered = parsed.unwrap_err().to_string();
            for valid in ["mainnet", "testnet", "local"] {
                assert!(
                    rendered.contains(valid),
                    "the error for `{retired}` should list `{valid}`; got: {rendered}"
                );
            }
        }
    }

    /// A config file written before the rename must not silently change which
    /// network is used, and must not be fatal either: the default *is* the host
    /// `stable` named, so the invocation is unaffected and only the stale name
    /// needs fixing.
    #[test]
    fn retired_network_in_the_config_file_falls_back_to_the_default() {
        for retired in ["stable", "beta", "STABLE", "  beta  "] {
            let file = FileConfig {
                network: Some(retired.into()),
                ..Default::default()
            };
            let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
            assert_eq!(
                base_url_with(&cli, &file),
                Network::Testnet.base_url(),
                "a config naming {retired:?} should land on the default network"
            );
        }
    }

    /// What the table claims is what the retired names *were*: `stable`/`beta`
    /// pointed at a play-funds host. That stays true no matter what the SDK
    /// default later becomes, so it is asserted on its own terms — every
    /// replacement must be a real network, and must not be real funds.
    ///
    /// Deliberately *not* asserted against `Config::default()`. The two facts
    /// coincide today, and an earlier version of this test conflated them, which
    /// would have made a future default change look like a table error. Where a
    /// stale config actually lands is covered by
    /// `the_stale_network_warning_names_where_it_actually_lands` and
    /// `retired_network_in_the_config_file_falls_back_to_the_default`.
    #[test]
    fn every_retired_name_maps_to_a_play_funds_network() {
        for (retired, replacement) in RETIRED_NETWORKS {
            let mapped = NetworkArg::builtin(replacement)
                .unwrap_or_else(|| panic!("{retired}'s replacement {replacement:?} must parse"));
            let funds = mapped
                .builtin_network()
                .expect("a built-in replacement")
                .funds();
            assert_eq!(
                funds,
                Funds::Play,
                "{retired:?} named a play-funds host, so its replacement must be known play \
                 funds, got {replacement:?} ({funds:?})"
            );
        }
    }

    /// Pins the transcription. `From<NetworkArg> for Network` mirrors an axis the
    /// SDK owns, and because `Network` is `#[non_exhaustive]` no match here can
    /// fail when a variant is added upstream — so the pinned version is the only
    /// thing that can notice.
    ///
    /// When an SDK bump trips this: re-read `Network`'s variants, map any new one
    /// (or decide the CLI should not expose it), then move the constant. Expect
    /// `sdk-autobump.yml`'s PR to land on this deliberately.
    #[test]
    fn the_network_axis_was_checked_against_the_pinned_sdk() {
        // `NEXUS_SDK_VERSION` is injected by `build.rs` from `Cargo.lock`, so this
        // is the version actually linked rather than the caret requirement in
        // `Cargo.toml` — a patch release picked up by `cargo update` trips it too,
        // which is right: a `#[non_exhaustive]` enum can gain a variant in one.
        let linked = env!("NEXUS_SDK_VERSION");
        assert_eq!(
            linked, NETWORK_AXIS_VERIFIED_AGAINST,
            "this build links nexus-exchange {linked}, but the {{mainnet, testnet, local}} \
             mapping was last read against {NETWORK_AXIS_VERIFIED_AGAINST}. `Network` is \
             #[non_exhaustive], so the compiler cannot flag a new variant: re-read it, map any \
             new network (or decide not to expose it), then update \
             NETWORK_AXIS_VERIFIED_AGAINST."
        );
    }

    /// The fix for the case the review caught: a config-file network that is
    /// neither valid nor retired used to fall through in complete silence, so the
    /// user believed they had selected a network while the CLI used the default.
    /// Typos are the durable version of this — `stable`/`beta` age out of configs,
    /// misspellings never do.
    #[test]
    fn an_unparseable_config_network_is_always_reported() {
        for name in ["mainet", "prod", "Testnet2", "main net"] {
            let warning = stale_network_warning(name, "testnet")
                .unwrap_or_else(|| panic!("{name:?} must produce a warning, not silence"));
            assert!(
                warning.contains(name),
                "the warning should quote the offending value; got: {warning}"
            );
            assert!(
                warning.contains("not a known network"),
                "the warning should say the value is unknown; got: {warning}"
            );
            for valid in ["mainnet", "testnet", "local"] {
                assert!(
                    warning.contains(valid),
                    "the warning should list `{valid}` as valid; got: {warning}"
                );
            }
            // No retired-channel explanation for a value that never was one.
            assert!(
                !warning.contains("release channel"),
                "unexpected retired-name story for {name:?}: {warning}"
            );
        }
    }

    /// A retired name keeps its own sentence — the migration story is the whole
    /// reason the table exists — and still points at `testnet`, never `mainnet`.
    #[test]
    fn a_retired_config_network_keeps_its_migration_hint() {
        for retired in ["stable", "beta", "STABLE", "  beta  "] {
            let warning = stale_network_warning(retired, "testnet")
                .unwrap_or_else(|| panic!("{retired:?} must produce a warning"));
            assert!(
                warning.contains("release channel"),
                "the warning for {retired:?} should explain what it was; got: {warning}"
            );
            assert!(
                warning.contains("testnet"),
                "the warning for {retired:?} must point at testnet; got: {warning}"
            );
        }
    }

    /// The "where you actually land" clause is read from the resolved default, not
    /// restated from `RETIRED_NETWORKS`, so the warning cannot outlive a change to
    /// the SDK default. Asserted for a retired *and* an unknown name, since the
    /// two take different branches.
    #[test]
    fn the_stale_network_warning_names_where_it_actually_lands() {
        let landing = Config::default().network().label().to_string();
        for name in ["stable", "mainet"] {
            let warning = stale_network_warning(name, &landing).expect("a warning");
            assert!(
                warning.contains(&format!("the default network ({landing})")),
                "the warning should name the resolved default {landing:?}; got: {warning}"
            );
        }
    }

    /// A blank value selects nothing, which is not a stale name — `nexus setup`
    /// normalizes an empty answer to "no preference" and a hand-edited
    /// `"network": ""` means the same. Warning there would be noise.
    #[test]
    fn a_blank_config_network_is_not_reported_as_stale() {
        for blank in ["", "   ", "\t"] {
            assert!(
                stale_network_warning(blank, "testnet").is_none(),
                "a blank network ({blank:?}) should be silent, not a warning"
            );
        }
    }

    /// `network_name` is what the warning interpolates, so it has to agree with
    /// the vocabulary `--network` accepts — otherwise the CLI would suggest a
    /// value it cannot parse.
    #[test]
    fn the_sdk_label_round_trips_through_the_flag_vocabulary() {
        for arg in [NetworkArg::Mainnet, NetworkArg::Testnet, NetworkArg::Local] {
            let label = arg.builtin_network().expect("a built-in network");
            let label = label.label();
            assert_eq!(
                NetworkArg::from_flag(label).as_ref(),
                Ok(&arg),
                "the SDK labels this {label:?}, which --network does not accept"
            );
            // And the CLI's own name for it agrees, since that is the credential
            // key the SDK documents as safe.
            assert_eq!(arg.name(), label);
        }
    }

    #[test]
    fn network_args_map_one_to_one_onto_the_sdk_axis() {
        assert_eq!(
            NetworkArg::Mainnet.builtin_network(),
            Some(Network::Mainnet)
        );
        assert_eq!(
            NetworkArg::Testnet.builtin_network(),
            Some(Network::Testnet)
        );
        assert_eq!(NetworkArg::Local.builtin_network(), Some(Network::Local));
        // A custom label has no built-in mapping: its bundle comes from the file.
        assert_eq!(NetworkArg::Custom("dev".into()).builtin_network(), None);
        // Only mainnet is real funds — the classification the SDK guards
        // `fund()` with, matched positively so `Unknown` can never pass for safe.
        assert_eq!(Network::Mainnet.funds(), Funds::Real);
        assert_eq!(Network::Testnet.funds(), Funds::Play);
        assert_eq!(Network::Local.funds(), Funds::Play);
    }

    /// `--network mainnet` is accepted here and refused by the SDK at request
    /// time, with a reason. Asserting the CLI does *not* pre-empt it keeps one
    /// gate instead of two that can disagree — the SDK owns the refusal.
    #[test]
    fn mainnet_is_selectable_and_left_to_the_sdk_to_refuse() {
        let cli = Cli::try_parse_from(["nexus", "--network", "mainnet", "markets"]).unwrap();
        assert_eq!(cli.network, Some(NetworkArg::Mainnet));
        assert_eq!(base_url(&cli), Network::Mainnet.base_url());
    }

    #[test]
    fn defaults_to_human_output() {
        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Human);
    }

    #[test]
    fn parses_output_json() {
        let cli = Cli::try_parse_from(["nexus", "--output", "json", "markets"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Json);
    }

    #[test]
    fn rejects_unknown_output() {
        assert!(Cli::try_parse_from(["nexus", "--output", "yaml", "markets"]).is_err());
    }

    #[test]
    fn credentials_require_both_halves() {
        let empty = FileConfig::default();
        let cli = Cli::try_parse_from(["nexus", "--api-key", "k", "markets"]).unwrap();
        assert!(cli.credentials(&empty, &target(&cli)).is_none());

        let cli = Cli::try_parse_from(["nexus", "--api-key", "k", "--api-secret", "s", "markets"])
            .unwrap();
        assert!(cli.credentials(&empty, &target(&cli)).is_some());
    }

    #[test]
    fn debug_redacts_api_secret() {
        let cli = Cli::try_parse_from([
            "nexus",
            "--api-key",
            "nx_visible",
            "--api-secret",
            "topsecret",
            "markets",
        ])
        .unwrap();
        let dbg = format!("{cli:?}");
        assert!(!dbg.contains("topsecret"), "secret leaked via Debug: {dbg}");
        assert!(dbg.contains("nx_visible"));
        assert!(dbg.contains("<redacted>"));
    }

    #[test]
    fn credentials_fall_back_to_file() {
        let mut file = FileConfig::default();
        *file.section_mut(DEFAULT_NETWORK.name()) = NetworkCredentials {
            api_key: Some("k".into()),
            api_secret: Some("s".into()),
            session_token: None,
        };
        let cli = Cli::try_parse_from(["nexus", "balance"]).unwrap();
        let target = cli.target(&file).unwrap();
        assert_eq!(
            cli.credentials(&file, &target),
            Some(("k".into(), "s".into()))
        );
    }

    #[test]
    fn flag_overrides_file_credentials() {
        let mut file = FileConfig::default();
        *file.section_mut(DEFAULT_NETWORK.name()) = NetworkCredentials {
            api_key: Some("file-key".into()),
            api_secret: Some("file-secret".into()),
            session_token: None,
        };
        // Flag key layers over the file secret, per-field.
        let cli = Cli::try_parse_from(["nexus", "--api-key", "flag-key", "balance"]).unwrap();
        let target = cli.target(&file).unwrap();
        assert_eq!(
            cli.credentials(&file, &target),
            Some(("flag-key".into(), "file-secret".into()))
        );
    }

    #[test]
    fn sets_descriptive_user_agent() {
        let expected = format!("nexus-cli/{}", env!("CARGO_PKG_VERSION"));

        // Network path.
        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        assert_eq!(cli.config(&target(&cli)).user_agent(), expected);

        // Explicit base-url path also carries the UA.
        let cli = Cli::try_parse_from(["nexus", "--base-url", "http://x:1", "markets"]).unwrap();
        assert_eq!(cli.config(&target(&cli)).user_agent(), expected);
    }

    #[test]
    fn completions_parses_bash() {
        let cli = Cli::try_parse_from(["nexus", "completions", "bash"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Completions { shell: Shell::Bash }
        ));
    }

    #[test]
    fn order_place_parses() {
        let cli = Cli::try_parse_from([
            "nexus",
            "order",
            "place",
            "--market",
            "BTC-USDX-PERP",
            "--side",
            "buy",
            "--type",
            "limit",
            "--price",
            "84000",
            "--quantity",
            "0.01",
        ])
        .unwrap();
        match cli.command {
            Command::Order {
                action:
                    OrderCommand::Place {
                        market,
                        side,
                        order_type,
                        tif,
                        ..
                    },
            } => {
                assert_eq!(market, "BTC-USDX-PERP");
                assert_eq!(side, SideArg::Buy);
                assert_eq!(order_type, OrderTypeArg::Limit);
                assert_eq!(tif, TifArg::Gtc);
            }
            _ => panic!("expected order place"),
        }
    }

    #[test]
    fn market_summary_parses() {
        let cli = Cli::try_parse_from(["nexus", "market", "summary"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Market {
                action: MarketCommand::Summary
            }
        ));
    }

    #[test]
    fn account_rate_limit_parses() {
        let cli = Cli::try_parse_from(["nexus", "account", "rate-limit"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Account {
                action: AccountCommand::RateLimit
            }
        ));
    }

    #[test]
    fn market_status_takes_a_market_id() {
        let cli = Cli::try_parse_from(["nexus", "market", "status", "BTC-USDX-PERP"]).unwrap();
        match cli.command {
            Command::Market {
                action: MarketCommand::Status { market_id },
            } => assert_eq!(market_id, "BTC-USDX-PERP"),
            _ => panic!("expected market status"),
        }
        // The market id is required.
        assert!(Cli::try_parse_from(["nexus", "market", "status"]).is_err());
    }

    #[test]
    fn market_mark_price_takes_a_market_id() {
        let cli = Cli::try_parse_from(["nexus", "market", "mark-price", "BTC-USDX-PERP"]).unwrap();
        match cli.command {
            Command::Market {
                action: MarketCommand::MarkPrice { market_id },
            } => assert_eq!(market_id, "BTC-USDX-PERP"),
            _ => panic!("expected market mark-price"),
        }
    }

    #[test]
    fn order_cancel_all_conflicts_with_id() {
        // `--all` and a positional id are mutually exclusive.
        assert!(Cli::try_parse_from(["nexus", "order", "cancel", "abc", "--all"]).is_err());
    }

    #[test]
    fn ws_requires_at_least_one_channel() {
        assert!(Cli::try_parse_from(["nexus", "ws"]).is_err());
        let cli =
            Cli::try_parse_from(["nexus", "ws", "trades", "--market", "BTC-USDX-PERP"]).unwrap();
        assert!(matches!(cli.command, Command::Ws { .. }));
    }

    /// `--help` renders, names the binary, and lists the full command surface.
    /// Guards against a command silently dropping out of the top-level help.
    #[test]
    fn top_level_help_lists_every_command() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("nexus"), "help should name the binary");
        for cmd in [
            "markets",
            "market",
            "ticker",
            "tickers",
            "summaries",
            "mark-price",
            "market-status",
            "funding-rates",
            "orderbook",
            "trades",
            "candles",
            "health",
            "balance",
            "account",
            "positions",
            "fills",
            "withdrawals",
            "orders",
            "order",
            "funding-payments",
            "withdrawals",
            "account",
            "auth",
            "keys",
            "agents",
            "transfers",
            "sub-accounts",
            "ws",
            "setup",
            "completions",
        ] {
            assert!(help.contains(cmd), "top-level help should list `{cmd}`");
        }
    }

    /// Every subcommand (and nested subcommand) renders `--help` without
    /// panicking and produces a usage line — exercises the whole help path.
    #[test]
    fn every_subcommand_renders_help() {
        fn check(cmd: &mut clap::Command) {
            let help = cmd.render_long_help().to_string();
            assert!(
                help.contains("Usage:"),
                "`{}` help should have a usage line",
                cmd.get_name()
            );
            for sub in cmd.get_subcommands_mut() {
                check(sub);
            }
        }
        check(&mut Cli::command());
    }

    #[test]
    fn order_get_parses() {
        // `--market` is required (by-id lookups are routed per market).
        assert!(Cli::try_parse_from(["nexus", "order", "get", "o123"]).is_err());
        let cli =
            Cli::try_parse_from(["nexus", "order", "get", "o123", "--market", "BTC-USDX-PERP"])
                .unwrap();
        match cli.command {
            Command::Order {
                action: OrderCommand::Get { order_id, market },
            } => {
                assert_eq!(order_id, "o123");
                assert_eq!(market, "BTC-USDX-PERP");
            }
            other => panic!("expected order get, got {other:?}"),
        }
    }

    #[test]
    fn order_cancel_by_id_requires_market() {
        // A single by-id cancel needs `--market`; `--all` does not (and the two
        // are mutually exclusive).
        assert!(Cli::try_parse_from(["nexus", "order", "cancel", "o123"]).is_err());
        assert!(Cli::try_parse_from([
            "nexus",
            "order",
            "cancel",
            "--all",
            "--market",
            "BTC-USDX-PERP"
        ])
        .is_err());
        let cli = Cli::try_parse_from([
            "nexus",
            "order",
            "cancel",
            "o123",
            "--market",
            "BTC-USDX-PERP",
        ])
        .unwrap();
        match cli.command {
            Command::Order {
                action:
                    OrderCommand::Cancel {
                        order_id, market, ..
                    },
            } => {
                assert_eq!(order_id.as_deref(), Some("o123"));
                assert_eq!(market.as_deref(), Some("BTC-USDX-PERP"));
            }
            other => panic!("expected order cancel, got {other:?}"),
        }
    }

    #[test]
    fn order_amend_requires_market() {
        // Amends are routed per market, so `--market` is required.
        assert!(
            Cli::try_parse_from(["nexus", "order", "amend", "o123", "--price", "100"]).is_err()
        );
        let cli = Cli::try_parse_from([
            "nexus",
            "order",
            "amend",
            "o123",
            "--market",
            "BTC-USDX-PERP",
            "--price",
            "100",
        ])
        .unwrap();
        match cli.command {
            Command::Order {
                action: OrderCommand::Amend { market, .. },
            } => assert_eq!(market, "BTC-USDX-PERP"),
            other => panic!("expected order amend, got {other:?}"),
        }
    }

    #[test]
    fn account_leverage_parses() {
        let cli =
            Cli::try_parse_from(["nexus", "account", "leverage", "BTC-USDX-PERP", "10"]).unwrap();
        match cli.command {
            Command::Account {
                action:
                    AccountCommand::Leverage {
                        market_id,
                        leverage,
                    },
            } => {
                assert_eq!(market_id, "BTC-USDX-PERP");
                assert_eq!(leverage, 10);
            }
            _ => panic!("expected account leverage"),
        }
    }

    /// `account margin-mode` is withdrawn (ENG-7740): no endpoint backs it, so
    /// offering it was a correctness claim the CLI could not honour. Parsing must
    /// fail rather than accept the arguments and dispatch a request that cannot
    /// succeed. Re-adding the subcommand fails this test on purpose — it may only
    /// come back once ENG-7614 lands and the spec defines the request shape.
    #[test]
    fn account_margin_mode_is_withdrawn() {
        for args in [
            vec![
                "nexus",
                "account",
                "margin-mode",
                "BTC-USDX-PERP",
                "isolated",
            ],
            vec!["nexus", "account", "margin-mode", "BTC-USDX-PERP", "cross"],
            vec!["nexus", "account", "margin-mode"],
        ] {
            let err = Cli::try_parse_from(&args)
                .expect_err(&format!("{args:?} must be rejected, not parsed"));
            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::InvalidSubcommand,
                "{args:?} should be an unrecognized-subcommand error, got: {err}"
            );
        }
    }

    /// The withdrawal must also be invisible in help, so `--help` can't keep
    /// advertising a command that cannot work.
    #[test]
    fn account_help_does_not_offer_margin_mode() {
        let mut cli = Cli::command();
        let account = cli
            .get_subcommands_mut()
            .find(|c| c.get_name() == "account")
            .expect("account subcommand");
        let help = account.render_long_help().to_string();
        assert!(
            help.contains("leverage"),
            "sanity: account help should still list leverage: {help}"
        );
        assert!(
            !help.contains("margin-mode"),
            "account help must not offer the withdrawn margin-mode command: {help}"
        );
    }

    /// The top-level `account` summary line must not advertise a margin surface
    /// either. It read "(deposit, credit, leverage, margin, rate-limit)" while the
    /// subcommand existed; leaving "margin" there would keep promising a
    /// capability the CLI does not have, which is the same claim in shorter form.
    #[test]
    fn account_summary_does_not_advertise_margin() {
        let help = Cli::command().render_long_help().to_string();
        let line = help
            .lines()
            .find(|l| l.contains("Manage account settings"))
            .expect("top-level help should summarize `account`");
        assert!(
            !line.contains("margin"),
            "the account summary must not advertise a margin surface: {line:?}"
        );
        assert!(
            line.contains("leverage") && line.contains("rate-limit"),
            "sanity: the summary should still name the settings that do work: {line:?}"
        );
    }

    /// The portfolio-parity reads (ENG-6460) parse as `account` subcommands.
    #[test]
    fn account_portfolio_subcommands_parse() {
        assert!(matches!(
            Cli::try_parse_from(["nexus", "account", "summary"])
                .unwrap()
                .command,
            Command::Account {
                action: AccountCommand::Summary
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["nexus", "account", "state"])
                .unwrap()
                .command,
            Command::Account {
                action: AccountCommand::State
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["nexus", "account", "fees"])
                .unwrap()
                .command,
            Command::Account {
                action: AccountCommand::Fees
            }
        ));
    }

    #[test]
    fn portfolio_history_defaults_window_and_limit_to_the_server() {
        // Both are optional: omitting them lets the server pick its `day`
        // default and full window rather than the CLI inventing one.
        let cli = Cli::try_parse_from(["nexus", "account", "portfolio-history"]).unwrap();
        match cli.command {
            Command::Account {
                action: AccountCommand::PortfolioHistory { window, limit },
            } => {
                assert_eq!(window, None);
                assert_eq!(limit, None);
            }
            other => panic!("expected account portfolio-history, got {other:?}"),
        }

        let cli = Cli::try_parse_from([
            "nexus",
            "account",
            "portfolio-history",
            "--window",
            "month",
            "--limit",
            "50",
        ])
        .unwrap();
        match cli.command {
            Command::Account {
                action: AccountCommand::PortfolioHistory { window, limit },
            } => {
                assert_eq!(window, Some(PortfolioWindowArg::Month));
                assert_eq!(limit, Some(50));
            }
            other => panic!("expected account portfolio-history, got {other:?}"),
        }
    }

    /// `--limit` is bounded by the API's request schema (1..=366) at parse time,
    /// so an out-of-range value is refused before anything is signed or sent.
    #[test]
    fn portfolio_history_limit_is_bounded_by_the_spec() {
        for bad in ["0", "367", "-1", "4294967296"] {
            assert!(
                Cli::try_parse_from(["nexus", "account", "portfolio-history", "--limit", bad])
                    .is_err(),
                "--limit {bad} should be rejected"
            );
        }
        for ok in ["1", "366"] {
            assert!(
                Cli::try_parse_from(["nexus", "account", "portfolio-history", "--limit", ok])
                    .is_ok(),
                "--limit {ok} should be accepted"
            );
        }
        // The bound tracks the SDK constant rather than a copy of the number.
        assert_eq!(MAX_PORTFOLIO_HISTORY_LIMIT, 366);
    }

    /// `--window` only accepts the spec's closed enum, so no caller-shaped text
    /// can reach the signed query string.
    #[test]
    fn portfolio_history_window_is_a_closed_enum() {
        for bad in ["quarter", "DAY", "", "day; rm -rf /"] {
            assert!(
                Cli::try_parse_from(["nexus", "account", "portfolio-history", "--window", bad])
                    .is_err(),
                "--window {bad:?} should be rejected"
            );
        }
        assert_eq!(
            PortfolioWindow::from(PortfolioWindowArg::All).as_str(),
            "all"
        );
    }

    /// `fills --limit` is now the server-side page size, bounded by the API's
    /// 1..=1000 (nexus-exchange 0.7.0 forwards it instead of the CLI truncating).
    #[test]
    fn fills_limit_is_bounded_by_the_page_size() {
        for bad in ["0", "1001"] {
            assert!(
                Cli::try_parse_from(["nexus", "fills", "--limit", bad]).is_err(),
                "--limit {bad} should be rejected"
            );
        }
        assert!(Cli::try_parse_from(["nexus", "fills", "--limit", "1000"]).is_ok());
        assert_eq!(MAX_FILLS_LIMIT, 1000);
        // Default stays the server's own default page size.
        let cli = Cli::try_parse_from(["nexus", "fills"]).unwrap();
        assert!(matches!(cli.command, Command::Fills { limit: 100 }));
    }

    #[test]
    fn auth_login_parses_and_takes_private_key_flag() {
        let cli =
            Cli::try_parse_from(["nexus", "auth", "login", "--private-key", "0xabc"]).unwrap();
        match cli.command {
            Command::Auth {
                action: AuthCommand::Login { private_key },
            } => assert_eq!(private_key.as_deref(), Some("0xabc")),
            _ => panic!("expected auth login"),
        }
        // The private key is optional (env var / prompt fallback).
        let cli = Cli::try_parse_from(["nexus", "auth", "login"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Auth {
                action: AuthCommand::Login { private_key: None }
            }
        ));
    }

    #[test]
    fn agents_register_parses_with_defaults() {
        let cli = Cli::try_parse_from([
            "nexus",
            "agents",
            "register",
            "--agent",
            "0x1234567890abcdef1234567890abcdef12345678",
            "--private-key",
            "0xkey",
        ])
        .unwrap();
        match cli.command {
            Command::Agents {
                action:
                    AgentsCommand::Register {
                        agent,
                        chain_id,
                        nonce,
                        expires_at,
                        label,
                        ..
                    },
            } => {
                assert_eq!(agent, "0x1234567890abcdef1234567890abcdef12345678");
                assert_eq!(
                    chain_id, None,
                    "the chain id defaults from the target at call time, not at parse time"
                );
                assert_eq!(nonce, None, "nonce defaults at call time, not parse time");
                assert_eq!(expires_at, None);
                assert_eq!(label, None);
            }
            _ => panic!("expected agents register"),
        }
    }

    #[test]
    fn session_token_resolves_flag_over_file() {
        let mut file = FileConfig::default();
        file.section_mut(DEFAULT_NETWORK.name()).session_token = Some("file-token".into());
        // No flag -> file token.
        let cli = Cli::try_parse_from(["nexus", "balance"]).unwrap();
        let on_default = cli.target(&file).unwrap();
        assert_eq!(
            cli.session_token(&file, &on_default).as_deref(),
            Some("file-token")
        );
        // Flag wins.
        let cli =
            Cli::try_parse_from(["nexus", "--session-token", "flag-token", "balance"]).unwrap();
        assert_eq!(
            cli.session_token(&file, &on_default).as_deref(),
            Some("flag-token")
        );
        // Neither set -> None.
        let cli = Cli::try_parse_from(["nexus", "balance"]).unwrap();
        assert_eq!(
            cli.session_token(&FileConfig::default(), &target(&cli)),
            None
        );
    }

    #[test]
    fn debug_redacts_session_token() {
        let cli =
            Cli::try_parse_from(["nexus", "--session-token", "topsecrettoken", "balance"]).unwrap();
        let dbg = format!("{cli:?}");
        assert!(
            !dbg.contains("topsecrettoken"),
            "session token leaked via Debug: {dbg}"
        );
        assert!(dbg.contains("<redacted>"));
    }

    #[test]
    fn keys_and_agents_subcommands_parse() {
        assert!(matches!(
            Cli::try_parse_from(["nexus", "keys", "list"])
                .unwrap()
                .command,
            Command::Keys {
                action: KeysCommand::List
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["nexus", "agents", "revoke", "0xabc"])
                .unwrap()
                .command,
            Command::Agents {
                action: AgentsCommand::Revoke { .. }
            }
        ));
    }

    #[test]
    fn transfers_create_requires_flags() {
        // Missing --to/--amount is an error.
        assert!(Cli::try_parse_from(["nexus", "transfers", "create", "--from", "a"]).is_err());
        let cli = Cli::try_parse_from([
            "nexus",
            "transfers",
            "create",
            "--from",
            "a",
            "--to",
            "b",
            "--amount",
            "5",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Transfers {
                action: TransfersCommand::Create { .. }
            }
        ));
    }

    /// `order place`/`cancel` help spells out their flags, so the trading surface
    /// stays documented.
    #[test]
    fn order_subcommand_help_documents_flags() {
        let mut cli = Cli::command();
        let order = cli
            .get_subcommands_mut()
            .find(|c| c.get_name() == "order")
            .expect("order subcommand");
        let help = order.render_long_help().to_string();
        assert!(help.contains("place"));
        assert!(help.contains("cancel"));
    }

    // ─────────────────── custom networks (ENG-9827) ───────────────────

    /// The headline: a declared stage is selectable by flag, drives the base
    /// URL, and namespaces its credentials under its **label**.
    #[test]
    fn a_declared_stage_is_selectable_by_flag() {
        let file = file_declaring("dev", declared_stage("play"));
        let cli = Cli::try_parse_from(["nexus", "--network", "dev", "markets"]).unwrap();
        let target = cli.target(&file).expect("a declared stage resolves");

        assert_eq!(target.namespace(), "dev");
        assert_eq!(target.funds(), Funds::Play);
        assert!(target.has_faucet());
        assert_eq!(
            base_url_with(&cli, &file),
            "https://exchange.example.com/api/exchange"
        );
    }

    /// ...and by config file, on exactly the same terms. The two selection
    /// routes resolve through one function, so this pins that they agree rather
    /// than that a second code path happens to match.
    #[test]
    fn a_declared_stage_is_selectable_by_config_file() {
        let mut file = file_declaring("dev", declared_stage("play"));
        file.network = Some("dev".into());
        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        let target = cli.target(&file).expect("a declared stage resolves");

        assert_eq!(target.namespace(), "dev");
        assert_eq!(target.funds(), Funds::Play);
        // The flag still wins over the file, as it does for a built-in.
        let cli = Cli::try_parse_from(["nexus", "--network", "testnet", "markets"]).unwrap();
        assert_eq!(cli.target(&file).unwrap().namespace(), "testnet");
    }

    /// The whole reason the label is the namespace key rather than the URL
    /// (ENG-9827): two stages that share a host must not share credentials, and
    /// with named stages that collision would be between environments with
    /// different funds semantics.
    #[test]
    fn two_custom_labels_do_not_share_credentials() {
        let mut file = file_declaring("one", declared_stage("play"));
        // Deliberately the *same* base URL: if the namespace were derived from
        // the URL rather than from the label, these two would collide.
        file.custom_networks
            .insert("two".into(), declared_stage("real"));
        *file.section_mut("one") = NetworkCredentials {
            api_key: Some("nx_one".into()),
            api_secret: Some("one-secret".into()),
            session_token: Some("one-token".into()),
        };

        let on_one = Cli::try_parse_from(["nexus", "--network", "one", "balance"]).unwrap();
        let target = on_one.target(&file).unwrap();
        assert_eq!(
            on_one.credentials(&file, &target),
            Some(("nx_one".into(), "one-secret".into()))
        );
        assert_eq!(
            on_one.session_token(&file, &target).as_deref(),
            Some("one-token")
        );

        let on_two = Cli::try_parse_from(["nexus", "--network", "two", "balance"]).unwrap();
        let target = on_two.target(&file).unwrap();
        assert_eq!(
            on_two.credentials(&file, &target),
            None,
            "one stage's key must not authenticate another, even on the same host"
        );
        assert_eq!(on_two.session_token(&file, &target), None);
        assert_eq!(target.namespace(), "two");
    }

    /// A label that could address another target's credentials is refused, and
    /// refused at *parse* time, before it can select anything. The rules are the
    /// SDK's — see `validate_label` — so this asserts the CLI actually applies
    /// them rather than restating what they are.
    #[test]
    fn an_unsafe_label_is_rejected_by_the_flag() {
        for bad in [
            "../other", // traversal
            "one/two",  // separator
            "one:two",  // keyring separator
            "one two",  // whitespace
            ".",        // a directory, not a network
            "..",
            "d\u{e9}v", // non-ASCII: normalization makes keys ambiguous
            "one\ntwo", // control character
            "custom",   // the label `--base-url` targets carry
            "",
            &"x".repeat(65), // longer than a key has any reason to be
        ] {
            assert!(
                NetworkArg::from_flag(bad).is_err(),
                "--network {bad:?} must be rejected: it is used as a credential-storage key"
            );
        }
        // A built-in's own name is not rejected — it selects the built-in, which
        // is checked first, so it can never be read as a custom label and can
        // never reach that network's credentials while pointing elsewhere.
        for reserved in ["mainnet", "MAINNET", "  local  "] {
            let parsed = NetworkArg::from_flag(reserved).expect("a built-in name still selects");
            assert!(
                parsed.builtin_network().is_some(),
                "{reserved:?} must select the built-in, not become a label: {parsed:?}"
            );
        }
        // A plausible stage name is still accepted, trimmed.
        assert_eq!(
            NetworkArg::from_flag("  dev-1_2.3  "),
            Ok(NetworkArg::Custom("dev-1_2.3".into()))
        );
    }

    /// The same refusal on the config-file side, where a label arrives as a map
    /// key rather than through the flag parser. A declaration under a label the
    /// SDK would refuse must not become selectable by the back door.
    #[test]
    fn an_unsafe_declared_label_is_not_selectable() {
        for bad in ["../other", "one two", "d\u{e9}v"] {
            let mut file = file_declaring(bad, declared_stage("play"));
            file.network = Some(bad.into());
            assert_eq!(
                selectable_network(bad, &file),
                None,
                "a declaration under {bad:?} must not be selectable"
            );
            // The file naming it lands on the default with a diagnostic rather
            // than silently.
            let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
            assert_eq!(
                cli.target(&file).unwrap().namespace(),
                DEFAULT_NETWORK.name()
            );
            assert!(stale_network_warning(bad, "testnet").is_some());
        }
    }

    /// A declaration that claims a built-in network's name loses to the built-in
    /// — which is the safe resolution, since that name owns real credentials —
    /// but it must not lose in silence, or the user is pointed at a different
    /// host than the entry in front of them describes.
    #[test]
    fn a_declaration_may_not_claim_a_built_in_name() {
        for reserved in ["mainnet", "testnet", "local"] {
            let file = file_declaring(reserved, declared_stage("play"));
            let cli = Cli::try_parse_from(["nexus", "--network", reserved, "markets"]).unwrap();
            let target = cli.target(&file).expect("the built-in still resolves");

            assert_eq!(target.namespace(), reserved);
            assert_eq!(
                target.network(),
                &NetworkArg::builtin(reserved)
                    .unwrap()
                    .builtin_network()
                    .unwrap(),
                "the built-in must win over a declaration claiming its name"
            );
            let warning = shadowed_declaration_warning(reserved, &file)
                .unwrap_or_else(|| panic!("{reserved:?} must be reported, not ignored silently"));
            assert!(warning.contains(reserved));
            assert!(
                warning.contains("ignored"),
                "the warning must say the entry does nothing: {warning}"
            );
        }
        // Nothing to report when no declaration claims the name.
        assert!(shadowed_declaration_warning("mainnet", &FileConfig::default()).is_none());
    }

    /// Selecting a stage the file does not describe is an error, not a fallback:
    /// the user named it on this command line, and quietly sending the request
    /// somewhere else is the failure class ENG-6455 removed.
    #[test]
    fn an_undeclared_label_is_a_hard_error() {
        let file = file_declaring("dev", declared_stage("play"));
        let cli = Cli::try_parse_from(["nexus", "--network", "other", "markets"]).unwrap();
        let err = cli
            .target(&file)
            .expect_err("an undeclared label must not resolve")
            .to_string();
        assert!(
            err.contains("other"),
            "the error must quote the label: {err}"
        );
        // It names what *is* declared, so the fix is one line away.
        assert!(
            err.contains(r#"declared: "dev""#),
            "the error should list the declared labels: {err}"
        );
        assert!(
            err.contains("custom_networks"),
            "the error should say where to declare it: {err}"
        );
    }

    /// Config-file text reaches a terminal through these diagnostics, so it is
    /// escaped rather than interpolated raw: a label carrying an ESC sequence
    /// must not be able to rewrite the line reporting it. Every interpolation of
    /// a file-supplied value uses `{:?}`, and this is what notices if one stops.
    #[test]
    fn a_config_supplied_label_cannot_smuggle_control_bytes() {
        let hostile = "dev\u{1b}[2K\u{7}";
        let mut file = file_declaring(hostile, declared_stage("play"));
        file.network = Some(hostile.into());

        // It is not selectable, so the file naming it warns...
        assert_eq!(selectable_network(hostile, &file), None);
        let warning = stale_network_warning(hostile, "testnet").expect("a warning");
        assert!(!warning.contains('\u{1b}'), "raw ESC in: {warning:?}");
        assert!(!warning.contains('\u{7}'), "raw BEL in: {warning:?}");

        // ...and naming a *different* label lists what is declared, which must
        // escape it too.
        let cli = Cli::try_parse_from(["nexus", "--network", "other", "markets"]).unwrap();
        let err = cli.target(&file).expect_err("undeclared").to_string();
        assert!(!err.contains('\u{1b}'), "raw ESC in: {err:?}");
        assert!(!err.contains('\u{7}'), "raw BEL in: {err:?}");

        // The same for an unreadable `funds` value, the other file-supplied
        // string that is echoed back.
        let (funds, warning) = parse_funds("dev", Some("re\u{1b}[2Kal"));
        assert_eq!(funds, Funds::Unknown);
        assert!(!warning.expect("a warning").contains('\u{1b}'));
    }

    /// A stage with no base URL cannot be pointed anywhere, and there is
    /// deliberately no host to fall back to — that is the point of the variant.
    #[test]
    fn a_stage_without_a_base_url_is_refused() {
        let file = file_declaring(
            "dev",
            CustomNetworkConfig {
                funds: Some("play".into()),
                ..Default::default()
            },
        );
        let cli = Cli::try_parse_from(["nexus", "--network", "dev", "markets"]).unwrap();
        let err = cli.target(&file).expect_err("no base URL").to_string();
        assert!(err.contains("base_url"), "got: {err}");
    }

    /// URL validation is the SDK's, and this asserts the CLI routes through it:
    /// each of these would build a *wrong* request rather than merely fail.
    #[test]
    fn a_dangerous_url_is_refused() {
        for bad in [
            "https://user:pass@exchange.example.com", // credentials leak into logs
            "https://exchange.example.com/?x=1",      // a query swallows the path
            "https://exchange.example.com/#frag",
            "file:///etc/passwd", // not a thing this client can talk to
            "https://",           // no host
            "exchange.example.com",
        ] {
            let file = file_declaring(
                "dev",
                CustomNetworkConfig {
                    base_url: Some(bad.into()),
                    funds: Some("play".into()),
                    ..Default::default()
                },
            );
            let cli = Cli::try_parse_from(["nexus", "--network", "dev", "markets"]).unwrap();
            assert!(
                cli.target(&file).is_err(),
                "base_url {bad:?} must be refused"
            );
        }
    }

    /// Funds fail **closed**. An absent or unreadable classification is
    /// `Unknown`, never `Play` — the guardrails match `Play` positively, so this
    /// is what makes an unclassified stage refuse rather than pass.
    #[test]
    fn an_unreadable_funds_value_falls_closed_to_unknown() {
        for declared in [
            None,
            Some(""),
            Some("  "),
            Some("reel"),
            Some("Real"),
            Some("true"),
        ] {
            let (funds, warning) = parse_funds("dev", declared);
            assert_eq!(
                funds,
                Funds::Unknown,
                "funds {declared:?} must not resolve to anything but Unknown"
            );
            let warning =
                warning.unwrap_or_else(|| panic!("{declared:?} must warn, not be silent"));
            assert!(
                warning.contains("dev"),
                "the warning must name the stage: {warning}"
            );
        }
        // The three real values read cleanly and say nothing.
        for (declared, expected) in [
            ("real", Funds::Real),
            ("play", Funds::Play),
            ("unknown", Funds::Unknown),
            ("  play  ", Funds::Play),
        ] {
            assert_eq!(parse_funds("dev", Some(declared)), (expected, None));
        }
    }

    /// The rest of the bundle reaches the SDK: the WS origin, the split direct
    /// base, and the signing domain. Each is absent until declared — the CLI
    /// never derives one — so this pins that a declared one is not dropped.
    #[test]
    fn the_declared_bundle_reaches_the_sdk() {
        let file = file_declaring(
            "dev",
            CustomNetworkConfig {
                base_url: Some("https://exchange.example.com/api/exchange".into()),
                direct_base_url: Some("https://direct.example.com".into()),
                ws_url: Some("wss://stream.example.com/ws".into()),
                funds: Some("play".into()),
                faucet: Some(true),
                chain_id: Some(393),
            },
        );
        let cli = Cli::try_parse_from(["nexus", "--network", "dev", "markets"]).unwrap();
        let target = cli.target(&file).unwrap();
        let network = target.network();

        assert_eq!(
            network.base_url(),
            "https://exchange.example.com/api/exchange"
        );
        assert_eq!(network.direct_base_url(), "https://direct.example.com");
        assert_eq!(network.ws_base(), Some("wss://stream.example.com/ws"));
        assert_eq!(network.signing_domain().and_then(|d| d.chain_id), Some(393));
        // ...and the resolved `Config` carries them too, since that is what the
        // client actually reads.
        let config = cli.config(&target);
        assert_eq!(
            config.base_url(),
            "https://exchange.example.com/api/exchange"
        );
        assert_eq!(config.ws_url(), Some("wss://stream.example.com/ws"));
    }

    /// A registration is signed under the target's own domain when it declares
    /// one. Falling back to the constant for a stage on another chain would
    /// produce a signature that is valid somewhere else — the failure mode the
    /// never-guess rule exists for.
    #[test]
    fn the_signing_chain_id_follows_the_declared_domain() {
        let mut declared = declared_stage("play");
        declared.chain_id = Some(31337);
        let file = file_declaring("dev", declared);

        let cli = Cli::try_parse_from(["nexus", "--network", "dev", "markets"]).unwrap();
        assert_eq!(cli.target(&file).unwrap().signing_chain_id(), 31337);

        // A stage that declares none, and every built-in, fall back to the
        // exchange's own chain — the SDK publishes no chain id for them.
        let file = file_declaring("dev", declared_stage("play"));
        let cli = Cli::try_parse_from(["nexus", "--network", "dev", "markets"]).unwrap();
        assert_eq!(
            cli.target(&file).unwrap().signing_chain_id(),
            DEFAULT_CHAIN_ID
        );
        let cli = Cli::try_parse_from(["nexus", "--network", "testnet", "markets"]).unwrap();
        assert_eq!(target(&cli).signing_chain_id(), DEFAULT_CHAIN_ID);
    }

    /// An undeclared WS origin stays `None` rather than being derived from the
    /// REST base: it is a separate host, so `nexus ws` must refuse instead of
    /// connecting to a guessed one.
    #[test]
    fn an_undeclared_ws_origin_is_not_derived() {
        let file = file_declaring("dev", declared_stage("play"));
        let cli = Cli::try_parse_from(["nexus", "--network", "dev", "markets"]).unwrap();
        let target = cli.target(&file).unwrap();
        assert_eq!(target.network().ws_base(), None);
        assert_eq!(cli.config(&target).ws_url(), None);
    }

    /// An undeclared signing domain stays `None`, which means **refuse to sign**
    /// rather than fall back to a constant: a signature made under the wrong
    /// domain may be valid on a *different* network.
    #[test]
    fn an_undeclared_signing_domain_is_not_guessed() {
        let file = file_declaring("dev", declared_stage("play"));
        let cli = Cli::try_parse_from(["nexus", "--network", "dev", "markets"]).unwrap();
        assert_eq!(cli.target(&file).unwrap().network().signing_domain(), None);
    }

    /// No hostname is hardcoded for a custom target. The variant exists so this
    /// public artifact ships none, and the one URL literal on the path is the
    /// RFC 2606 placeholder the label validator probes with.
    #[test]
    fn no_hostname_is_shipped_for_a_custom_target() {
        let file = file_declaring("dev", declared_stage("play"));
        let cli = Cli::try_parse_from(["nexus", "--network", "dev", "markets"]).unwrap();
        let target = cli.target(&file).unwrap();
        // Verbatim from the file: nothing appended, rewritten or inferred.
        assert_eq!(
            target.network().base_url(),
            file.custom_networks["dev"].base_url.as_deref().unwrap()
        );
        assert!(
            LABEL_PROBE_URL.ends_with(".invalid"),
            "the label probe must use a name that can never resolve, got {LABEL_PROBE_URL:?}"
        );
    }

    /// `--base-url` keeps working, unchanged, for anyone upgrading: same
    /// precedence over `--network`, same credential namespace.
    #[test]
    fn the_base_url_override_still_wins_over_a_custom_network() {
        let file = file_declaring("dev", declared_stage("play"));
        let cli = Cli::try_parse_from([
            "nexus",
            "--network",
            "dev",
            "--base-url",
            "http://127.0.0.1:9090",
            "markets",
        ])
        .unwrap();
        let target = cli.target(&file).unwrap();
        assert_eq!(base_url_with(&cli, &file), "http://127.0.0.1:9090");
        assert_eq!(target.namespace(), "dev", "the label still owns the key");
        assert_eq!(target.base_url_override(), Some("http://127.0.0.1:9090"));
    }

    // ───────────────── per-network credentials (ENG-6462) ─────────────────

    /// [`DEFAULT_NETWORK`] transcribes the SDK's default so credential sections
    /// can be named before a `Config` exists. If the SDK ever moves its default,
    /// the two must move together — otherwise the CLI would read one network's
    /// stored key while sending the request to another.
    #[test]
    fn the_default_network_matches_the_sdk() {
        let sdk_default = Config::default().network().clone();
        assert_eq!(
            DEFAULT_NETWORK.builtin_network(),
            Some(sdk_default.clone()),
            "DEFAULT_NETWORK is {:?} but the SDK now defaults to {sdk_default:?}; credentials \
             would be read from one network's section and sent to another",
            DEFAULT_NETWORK
        );
    }

    /// The default must never move real money.
    ///
    /// `legacy_credential_owner` falls back to [`DEFAULT_NETWORK`] when a
    /// pre-namespacing config names no network, or names one this build cannot
    /// parse. So this constant decides who inherits an orphaned flat key, and
    /// while it is play funds that fallback is harmless. Were it ever a
    /// real-funds network, the migration would silently start attributing
    /// unattributable keys to mainnet — and nothing else in the tree would
    /// object, because `the_default_network_matches_the_sdk` only pins the two
    /// defaults *together*: an upstream default moving to mainnet would drag
    /// this one along and read as a passing test.
    ///
    /// This is the assertion that refuses. It is deliberately not derived from
    /// the SDK — the point is to fail rather than to follow.
    #[test]
    fn the_default_network_is_play_funds() {
        let funds = declared_funds(&DEFAULT_NETWORK, &FileConfig::default());
        assert_eq!(
            funds,
            Funds::Play,
            "DEFAULT_NETWORK is {:?}, whose funds are {funds:?}; a legacy config naming no \
             network would have its credentials migrated onto that section, and an unnamed \
             invocation would send requests there. Anything but known play funds needs a \
             deliberate decision, not a constant change",
            DEFAULT_NETWORK
        );
    }

    /// The CLI no longer keeps its own real-funds predicate: every guardrail
    /// reads [`Funds`] off the resolved target, which is the SDK's own answer.
    /// This pins that there is nothing left to drift — a built-in's funds come
    /// from the SDK and from nowhere else.
    #[test]
    fn built_in_funds_come_from_the_sdk() {
        let file = FileConfig::default();
        for arg in [NetworkArg::Mainnet, NetworkArg::Testnet, NetworkArg::Local] {
            assert_eq!(
                declared_funds(&arg, &file),
                arg.builtin_network().expect("a built-in network").funds(),
                "{} disagrees with the SDK about what it moves",
                arg.name()
            );
        }
    }

    /// A second real-funds target is now expressible — that is the point of
    /// ENG-9827 — so the one-time acknowledgement is keyed per network rather
    /// than held as the single `mainnet_acknowledged` flag it used to be. This
    /// is the assertion the old `exactly_one_network_moves_real_funds` deferred
    /// to: it demanded the schema change before a second real-funds network
    /// existed, and here it is.
    #[test]
    fn a_real_funds_custom_stage_is_expressible() {
        let file = file_declaring("dev", declared_stage("real"));
        let cli = Cli::try_parse_from(["nexus", "--network", "dev", "markets"]).unwrap();
        let target = cli.target(&file).expect("a declared stage resolves");
        assert_eq!(target.funds(), Funds::Real);
        assert_eq!(target.namespace(), "dev");
        assert!(target.touches_real_funds());
    }

    /// The section key is the name users type and hand-edit into the config
    /// file, so it has to survive a round trip through the flag parser.
    #[test]
    fn section_keys_round_trip_through_the_flag() {
        for arg in [NetworkArg::Mainnet, NetworkArg::Testnet, NetworkArg::Local] {
            assert_eq!(NetworkArg::from_flag(arg.name()).as_ref(), Ok(&arg));
        }
    }

    #[test]
    fn the_credential_namespace_follows_flag_then_file_then_default() {
        let file = FileConfig {
            network: Some("local".into()),
            ..Default::default()
        };

        let cli = Cli::try_parse_from(["nexus", "--network", "mainnet", "markets"]).unwrap();
        assert_eq!(cli.target(&file).unwrap().namespace(), "mainnet");

        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        assert_eq!(cli.target(&file).unwrap().namespace(), "local");
        assert_eq!(target(&cli).namespace(), DEFAULT_NETWORK.name());
    }

    /// A base-URL override redirects the request but must not change *whose*
    /// key is presented — there is no network to derive from an arbitrary URL,
    /// and users pointing at a proxy in front of a network still want that
    /// network's credentials.
    #[test]
    fn a_base_url_override_does_not_change_the_credential_namespace() {
        let file = FileConfig {
            network: Some("mainnet".into()),
            ..Default::default()
        };
        let cli = Cli::try_parse_from(["nexus", "--base-url", "http://127.0.0.1:9090", "markets"])
            .unwrap();
        assert_eq!(cli.target(&file).unwrap().namespace(), "mainnet");
    }

    /// ...but it *does* change what the target is known to move. The named
    /// network's classification describes the named network, and the request is
    /// no longer going there. Reporting `Play` for a host nobody classified is
    /// the exact failure ENG-9823 exists to remove.
    #[test]
    fn a_base_url_override_has_undeclared_funds() {
        let cli = Cli::try_parse_from([
            "nexus",
            "--network",
            "local",
            "--base-url",
            "http://x:1",
            "markets",
        ])
        .unwrap();
        let target = target(&cli);
        assert_eq!(target.funds(), Funds::Unknown);
        // The credential side still belongs to the named network, so the two
        // questions have two answers rather than one that is wrong for both.
        assert_eq!(target.credential_funds(), Funds::Play);
        assert_eq!(target.namespace(), "local");
        // And it matches what the SDK reports for the same config, so the CLI's
        // guards and the SDK's cannot disagree about the same invocation.
        assert_eq!(cli.config(&target).network().funds(), Funds::Unknown);
    }

    /// The core guarantee of ENG-6462: a key stored for one network is never
    /// offered to another. Server-side enforcement (ENG-6443) would reject it
    /// anyway, but only after the request has left with a real-funds host as its
    /// destination — the refusal belongs on this side too.
    #[test]
    fn a_stored_key_is_never_offered_to_another_network() {
        let mut file = FileConfig::default();
        *file.section_mut("testnet") = NetworkCredentials {
            api_key: Some("nx_testnet".into()),
            api_secret: Some("testnet-secret".into()),
            session_token: Some("testnet-token".into()),
        };

        let on_testnet = Cli::try_parse_from(["nexus", "--network", "testnet", "balance"]).unwrap();
        let target = on_testnet.target(&file).unwrap();
        assert_eq!(
            on_testnet.credentials(&file, &target),
            Some(("nx_testnet".into(), "testnet-secret".into()))
        );
        assert_eq!(
            on_testnet.session_token(&file, &target).as_deref(),
            Some("testnet-token")
        );

        let on_mainnet = Cli::try_parse_from(["nexus", "--network", "mainnet", "balance"]).unwrap();
        let target = on_mainnet.target(&file).unwrap();
        assert_eq!(
            on_mainnet.credentials(&file, &target),
            None,
            "a testnet key must not authenticate a mainnet invocation"
        );
        assert_eq!(
            on_mainnet.session_token(&file, &target),
            None,
            "a testnet session token must not authenticate a mainnet invocation"
        );
    }

    /// Flags and env stay global: they are a per-invocation override for the
    /// network just selected, so they apply whichever section is active — and
    /// they still win over a stored value for that network.
    #[test]
    fn flags_override_the_selected_networks_section() {
        let mut file = FileConfig::default();
        *file.section_mut("mainnet") = NetworkCredentials {
            api_key: Some("stored".into()),
            api_secret: Some("stored-secret".into()),
            session_token: None,
        };
        let cli = Cli::try_parse_from([
            "nexus",
            "--network",
            "mainnet",
            "--api-key",
            "flag",
            "--api-secret",
            "flag-secret",
            "balance",
        ])
        .unwrap();
        let target = cli.target(&file).unwrap();
        assert_eq!(
            cli.credentials(&file, &target),
            Some(("flag".into(), "flag-secret".into()))
        );
    }

    /// Two networks configured at once is the case a flat config could not
    /// express, and the reason for the map.
    #[test]
    fn two_networks_coexist() {
        let mut file = FileConfig::default();
        *file.section_mut("testnet") = NetworkCredentials {
            api_key: Some("nx_testnet".into()),
            api_secret: Some("s1".into()),
            session_token: None,
        };
        *file.section_mut("mainnet") = NetworkCredentials {
            api_key: Some("nx_mainnet".into()),
            api_secret: Some("s2".into()),
            session_token: None,
        };
        for (flag, expected) in [("testnet", "nx_testnet"), ("mainnet", "nx_mainnet")] {
            let cli = Cli::try_parse_from(["nexus", "--network", flag, "balance"]).unwrap();
            let target = cli.target(&file).unwrap();
            assert_eq!(cli.credentials(&file, &target).unwrap().0, expected);
        }
    }
}
