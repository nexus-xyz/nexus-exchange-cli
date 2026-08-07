//! Real-funds guardrails (ENG-6462).
//!
//! Three checks stand between an invocation and real money: an active-network
//! banner, a hard refusal of the testnet faucet on mainnet, and a one-time
//! acknowledgement before the first mainnet trade. They are collected here
//! rather than spread through `main` so the rules read as a set — the CLI's
//! answer to "what stops me losing money by accident" is one file.
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
//! the cutover deletes. [`refuse_faucet_on_real_funds`] is therefore a real
//! guard rather than a duplicated one.

use anyhow::Result;

use crate::cli::NetworkArg;
use crate::credentials::{self, real_funds_notice, FileConfig};

/// Announce the active network on stderr when it moves real money.
///
/// Only real-funds networks say anything. A banner on every `nexus markets`
/// would be noise on the default (testnet) and noise is how a warning stops
/// being read — the signal worth spending is "this one is different".
///
/// stderr, not stdout, so `--output json` stays machine-parseable; the same
/// split the stale-network warning already uses.
pub fn announce_network(network: NetworkArg, base_url_override: Option<&str>) {
    if !network.is_real_funds() {
        return;
    }
    // A base-URL override changes the destination but not the credential
    // namespace, so "you are on mainnet" would be an overstatement — say what is
    // actually true, which is that mainnet's key is being presented to a host the
    // user chose. `{url:?}` because the value can come from the config file.
    match base_url_override {
        Some(url) => eprintln!(
            "{}",
            real_funds_notice(&format!(
                "Using mainnet credentials against the overridden base URL {url:?}."
            ))
        ),
        None => eprintln!(
            "{}",
            real_funds_notice("Every order, deposit and transfer settles for real.")
        ),
    }
}

/// Refuse the testnet faucet on a real-funds network.
///
/// The faucet mints synthetic collateral; on mainnet the equivalent is a real
/// deposit of real money, and the two must never be one command away from each
/// other. Naming `account deposit` in the error is the point — a refusal that
/// leaves the user guessing gets worked around.
pub fn refuse_faucet_on_real_funds(network: NetworkArg) -> Result<()> {
    if network.is_real_funds() {
        anyhow::bail!(
            "`account credit` claims synthetic USDX from the testnet faucet and is refused on \
             {}: there is no faucet for real funds. To add real collateral, deposit it \
             explicitly with `nexus account deposit <amount>`.",
            network.as_str()
        );
    }
    Ok(())
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
pub fn acknowledge_real_funds(network: NetworkArg, file: &FileConfig, yes: bool) -> Result<bool> {
    if !network.is_real_funds() || file.mainnet_acknowledged {
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
    credentials::save_mainnet_acknowledged()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_faucet_is_refused_only_on_real_funds() {
        for play in [NetworkArg::Testnet, NetworkArg::Local] {
            assert!(
                refuse_faucet_on_real_funds(play).is_ok(),
                "{} is play funds; the faucet must work",
                play.as_str()
            );
        }
        let err = refuse_faucet_on_real_funds(NetworkArg::Mainnet)
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

    /// The prompt must never fire on play funds — not even non-interactively,
    /// where `confirm` would turn it into a hard error and break testnet
    /// scripting for everyone.
    #[test]
    fn play_funds_never_prompt() {
        let file = FileConfig::default();
        for play in [NetworkArg::Testnet, NetworkArg::Local] {
            assert!(
                acknowledge_real_funds(play, &file, false).unwrap(),
                "{} must not require an acknowledgement",
                play.as_str()
            );
        }
    }

    /// An already-acknowledged config short-circuits before any I/O, so the
    /// prompt is genuinely one-time rather than once-per-session.
    #[test]
    fn an_acknowledged_config_does_not_prompt_again() {
        let file = FileConfig {
            mainnet_acknowledged: true,
            ..Default::default()
        };
        assert!(acknowledge_real_funds(NetworkArg::Mainnet, &file, false).unwrap());
    }

    /// `--yes` proceeds without touching the config file. If it recorded the
    /// acknowledgement, a single scripted run would silently disarm the prompt
    /// for every later interactive trade.
    #[test]
    fn yes_proceeds_without_recording() {
        let file = FileConfig::default();
        assert!(acknowledge_real_funds(NetworkArg::Mainnet, &file, true).unwrap());
        assert!(
            !file.mainnet_acknowledged,
            "--yes must not record the acknowledgement"
        );
    }
}
