//! Real-funds guardrails (ENG-6462).
//!
//! Three checks stand between an invocation and real money: an active-network
//! banner, a hard refusal of the faucet anywhere it is not known to mint
//! synthetic funds, and a one-time acknowledgement before the first trade on a
//! real-funds target. They are collected here rather than spread through `main`
//! so the rules read as a set — the CLI's answer to "what stops me losing money
//! by accident" is one file.
//!
//! # They key off a `Funds`, not off a network name
//!
//! Since ENG-9827 a caller-declared stage can be real funds under a label this
//! crate has never heard of, so "is this mainnet" is no longer the question. Each
//! guard reads [`Target::funds`] and matches [`Funds::Play`] **positively**:
//! negating the real case is how an unclassified target quietly becomes a safe
//! one, and unclassified is the common case for a private stage.
//!
//! # These run ahead of the SDK, not instead of it
//!
//! The pinned `nexus-exchange` refuses *every* mainnet request locally, in
//! `Client::base_for`, because `api.nexus.xyz` does not resolve yet. So today
//! nothing here is load-bearing: a mainnet request dies at the SDK gate whatever
//! the CLI does. That gate is exactly what the Phase 6 cutover (ENG-8865)
//! removes, and these guardrails are what remains afterwards, so they are
//! written and tested now rather than retrofitted onto a live real-funds host.
//!
//! One of them is not merely early. `fund()` refuses on mainnet inside the SDK,
//! but the CLI's faucet command calls `claim_credit()`, which carries **no**
//! per-network guard of its own — it is covered only by the blanket gate that
//! the cutover deletes. [`refuse_faucet_without_play_funds`] is therefore a real
//! guard rather than a duplicated one — and it is the only one of the three that
//! also covers a `Custom` target, which the SDK's blanket mainnet gate never
//! did.

use anyhow::Result;
use nexus_exchange::Funds;

use crate::cli::Target;
use crate::credentials::{self, real_funds_notice, FileConfig};

/// Announce the active network on stderr when it moves real money.
///
/// Only real-funds targets say anything. A banner on every `nexus markets`
/// would be noise on the default (testnet) and noise is how a warning stops
/// being read — the signal worth spending is "this one is different". An
/// undeclared-funds target is deliberately quiet here too: it is reported at the
/// point of refusal, where it is actionable, rather than on every read command.
///
/// stderr, not stdout, so `--output json` stays machine-parseable; the same
/// split the stale-network warning already uses.
pub fn announce_network(target: &Target) {
    if !target.touches_real_funds() {
        return;
    }
    match target.base_url_override() {
        // An override changes the destination but not the credential namespace,
        // so "you are on mainnet" would be an overstatement — say what is
        // actually true, which is that a real-funds network's key is being
        // presented to a host the user chose. `{url:?}` because the value can
        // come from the config file, and `Debug` escapes control bytes.
        Some(url) => eprintln!(
            "{}",
            real_funds_notice(
                target.namespace(),
                &format!(
                    "Using {} credentials against the overridden base URL {url:?}, whose funds \
                     are undeclared.",
                    target.namespace()
                )
            )
        ),
        None => eprintln!(
            "{}",
            real_funds_notice(
                target.namespace(),
                "Every order, deposit and transfer settles for real."
            )
        ),
    }
}

/// Refuse the faucet anywhere it is not known to mint *synthetic* funds.
///
/// The faucet mints play collateral; on a real-funds network the equivalent is a
/// real deposit of real money, and the two must never be one command away from
/// each other. Naming the alternative in each error is the point — a refusal
/// that leaves the user guessing gets worked around.
///
/// Three refusals, because there are three ways this can be wrong:
///
///   - **Real funds.** There is no faucet; `account deposit` is the real thing.
///   - **Undeclared funds.** Matched positively on [`Funds::Play`] rather than by
///     negating `Real`, so an unclassified target fails closed. Negating the real
///     case is precisely how "unknown" silently becomes "safe".
///   - **Play funds with no faucet.** "Not real money" does not imply "has a
///     faucet" — a private stage may be seeded by other means, and claiming
///     credit from one that is not there is a confusing 404 rather than a refusal
///     that says why.
pub fn refuse_faucet_without_play_funds(target: &Target) -> Result<()> {
    let network = target.namespace();
    match target.funds() {
        Funds::Play if target.has_faucet() => Ok(()),
        Funds::Play => anyhow::bail!(
            "`account credit` claims synthetic USDX from the faucet, and {network} does not \
             declare one. Set \"faucet\": true for it under \"custom_networks\" if the \
             deployment has a faucet; otherwise fund the account the way that stage is seeded."
        ),
        Funds::Real => anyhow::bail!(
            "`account credit` claims synthetic USDX from the faucet and is refused on \
             {network}: there is no faucet for real funds. To add real collateral, deposit it \
             explicitly with `nexus account deposit <amount>`."
        ),
        // Includes every future classification: `Funds` is `#[non_exhaustive]`,
        // and the wildcard arm is the safe one on purpose.
        _ => anyhow::bail!(
            "`account credit` mints funds, and this target does not declare whether it moves \
             real ones, so it is refused rather than assumed safe. {}",
            match target.base_url_override() {
                Some(url) => format!(
                    "The base URL {url:?} overrides {network}, and a bare URL carries no funds \
                     classification: declare the stage under \"custom_networks\" with a \
                     \"funds\" value and select it with `--network <label>`."
                ),
                None => format!(
                    "Set \"funds\" to \"real\", \"play\" or \"unknown\" for {network} under \
                     \"custom_networks\" in the config file."
                ),
            }
        ),
    }
}

/// One-time acknowledgement before the first trade on a real-funds network.
/// `Ok(false)` means the user declined and the caller must not trade.
///
/// Fires once per config, not once per order: the prompt exists to make the
/// first real trade a deliberate act, and re-asking forever would train the
/// reflex it is trying to interrupt. The acknowledgement is recorded only after
/// an interactive answer — see the `--yes` note below.
///
/// `--yes` skips it, as it skips every other confirmation in this CLI. An
/// unskippable prompt would leave no way to run on mainnet from a script, and
/// the escape hatch users would reach for instead is worse than the one we
/// document. It does **not** record the acknowledgement, so a later interactive
/// trade still gets the prompt once.
///
/// Recorded **per network** (ENG-9827): a custom stage can declare real funds,
/// so acknowledging mainnet must not disarm the prompt for a private real-funds
/// stage the user has never traded on.
pub fn acknowledge_real_funds(target: &Target, file: &FileConfig, yes: bool) -> Result<bool> {
    if !target.touches_real_funds() || file.acknowledged(target.namespace()) {
        return Ok(true);
    }
    // Neither message repeats the real-funds prefix: `announce_network` has
    // already printed it this run, and a warning stated twice in four lines
    // reads as a formatting bug rather than as emphasis.
    if yes {
        eprintln!("note: --yes given, so the first-trade acknowledgement is not being recorded.");
        return Ok(true);
    }

    // `confirm` refuses outright when stdin is not a terminal, so a script that
    // reaches here without --yes stops rather than silently trading.
    let acknowledged = crate::confirm(
        "This is your first trade on a real-funds network from this machine. \
         Funds moved here cannot be recovered by support. Continue",
        false,
    )?;
    if !acknowledged {
        return Ok(false);
    }
    // Persisted only on the way through: a decline leaves no trace, so the next
    // attempt asks again.
    credentials::save_acknowledged(target.namespace())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, CustomNetworkConfig};
    use clap::Parser;

    /// Resolve a target the way `main` does — through the real flag parser and
    /// the real config file — so a guardrail is never tested against a shape the
    /// binary cannot actually produce.
    fn target_for(args: &[&str], file: &FileConfig) -> Target {
        let mut argv = vec!["nexus"];
        argv.extend_from_slice(args);
        argv.push("markets");
        Cli::try_parse_from(argv)
            .expect("the arguments should parse")
            .target(file)
            .expect("the target should resolve")
    }

    /// A config file declaring one stage. `example.com` is RFC 2606 reserved.
    fn file_with(label: &str, funds: &str, faucet: bool) -> FileConfig {
        let mut file = FileConfig::default();
        file.custom_networks.insert(
            label.to_string(),
            CustomNetworkConfig {
                base_url: Some("https://exchange.example.com/api/exchange".into()),
                funds: Some(funds.into()),
                faucet: Some(faucet),
                ..Default::default()
            },
        );
        file
    }

    #[test]
    fn the_faucet_works_only_on_declared_play_funds_with_a_faucet() {
        let empty = FileConfig::default();
        for play in ["testnet", "local"] {
            assert!(
                refuse_faucet_without_play_funds(&target_for(&["--network", play], &empty)).is_ok(),
                "{play} is play funds with a faucet; the faucet must work"
            );
        }
        let err = refuse_faucet_without_play_funds(&target_for(&["--network", "mainnet"], &empty))
            .expect_err("the faucet must be refused on a real-funds network")
            .to_string();
        assert!(
            err.contains("mainnet"),
            "the error must name the network: {err}"
        );
        // The refusal has to point somewhere, or it just gets worked around.
        assert!(
            err.contains("account deposit"),
            "the error must name the real-funds alternative: {err}"
        );
    }

    /// The tri-state rule, on the arm that a boolean could not express: a stage
    /// nobody classified fails **closed**. Matching `Play` positively is what
    /// makes this true — negating `Real` would let `Unknown` through as safe.
    #[test]
    fn undeclared_funds_refuse_the_faucet() {
        for declared in ["unknown", "", "reel"] {
            let file = file_with("dev", declared, true);
            let err = refuse_faucet_without_play_funds(&target_for(&["--network", "dev"], &file))
                .expect_err("undeclared funds must refuse, not assume play funds")
                .to_string();
            assert!(
                err.contains("does not declare whether it moves real ones"),
                "funds {declared:?} should refuse as unclassified; got: {err}"
            );
            assert!(
                err.contains("\"funds\""),
                "the error must say which key to set; got: {err}"
            );
        }
    }

    /// A bare base URL is the same case reached the other way: it carries no
    /// classification, so it cannot mint funds either — and the error says so in
    /// terms of the override rather than of the network whose key it borrows.
    #[test]
    fn a_base_url_override_refuses_the_faucet() {
        let err = refuse_faucet_without_play_funds(&target_for(
            &["--network", "local", "--base-url", "http://127.0.0.1:9090"],
            &FileConfig::default(),
        ))
        .expect_err("an override carries no funds classification")
        .to_string();
        assert!(
            err.contains("--network <label>"),
            "the error must point at the declared-stage path; got: {err}"
        );
    }

    /// "Not real money" does not imply "has a faucet": a play-funds stage seeded
    /// by other means must refuse rather than send a request to a faucet that is
    /// not there.
    #[test]
    fn play_funds_without_a_faucet_still_refuse() {
        let file = file_with("dev", "play", false);
        let err = refuse_faucet_without_play_funds(&target_for(&["--network", "dev"], &file))
            .expect_err("a stage with no declared faucet must refuse")
            .to_string();
        assert!(
            err.contains("does not declare one"),
            "the error should say the faucet is undeclared; got: {err}"
        );
        // ...and declaring one lets it through.
        let file = file_with("dev", "play", true);
        assert!(
            refuse_faucet_without_play_funds(&target_for(&["--network", "dev"], &file)).is_ok()
        );
    }

    /// The prompt must never fire on play funds — not even non-interactively,
    /// where `confirm` would turn it into a hard error and break testnet
    /// scripting for everyone.
    #[test]
    fn play_funds_never_prompt() {
        let file = FileConfig::default();
        for play in ["testnet", "local"] {
            assert!(
                acknowledge_real_funds(&target_for(&["--network", play], &file), &file, false)
                    .unwrap(),
                "{play} must not require an acknowledgement"
            );
        }
    }

    /// An already-acknowledged config short-circuits before any I/O, so the
    /// prompt is genuinely one-time rather than once-per-session.
    #[test]
    fn an_acknowledged_config_does_not_prompt_again() {
        let mut file = FileConfig::default();
        file.acknowledged_networks.insert("mainnet".into());
        let target = target_for(&["--network", "mainnet"], &file);
        assert!(acknowledge_real_funds(&target, &file, false).unwrap());
    }

    /// The reason the acknowledgement is keyed per network (ENG-9827): a custom
    /// stage can declare real funds, and consent given for mainnet says nothing
    /// about a private stage the user has never traded on. With a single flag
    /// this call would short-circuit and the first real trade there would
    /// proceed unremarked.
    #[test]
    fn acknowledging_one_real_funds_network_does_not_cover_another() {
        let mut file = file_with("dev", "real", false);
        file.acknowledged_networks.insert("mainnet".into());
        let target = target_for(&["--network", "dev"], &file);
        assert!(target.touches_real_funds());
        assert!(
            !file.acknowledged(target.namespace()),
            "mainnet's acknowledgement must not cover {:?}",
            target.namespace()
        );
        // `--yes` is the non-interactive path through the prompt, so this
        // exercises the branch without needing a terminal.
        assert!(acknowledge_real_funds(&target, &file, true).unwrap());
    }

    /// `--yes` proceeds without touching the config file. If it recorded the
    /// acknowledgement, a single scripted run would silently disarm the prompt
    /// for every later interactive trade.
    #[test]
    fn yes_proceeds_without_recording() {
        let file = FileConfig::default();
        let target = target_for(&["--network", "mainnet"], &file);
        assert!(acknowledge_real_funds(&target, &file, true).unwrap());
        assert!(
            file.acknowledged_networks.is_empty(),
            "--yes must not record the acknowledgement"
        );
    }
}
