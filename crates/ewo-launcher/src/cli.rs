//! Command line: the `--dev` overlay flag and the headless `--mint-rewo-env`
//! token handoff for Rewo's online-mode harness. Moved out of `main.rs` unchanged.

use super::*;

#[derive(Parser, Debug)]
#[command(name = "ewolauncher", about = "EwoClient — Velvet & Pearl")]
pub(crate) struct Args {
    /// Show the developer overlay (state-picker, layout-picker, tweaks panel).
    #[arg(long)]
    pub(crate) dev: bool,
    /// Headless dev tool: refresh the active account's Minecraft token and
    /// print the `REWO_USERNAME` / `REWO_UUID` / `REWO_ACCESS_TOKEN` env
    /// handoff lines, then exit (no window). Used by Rewo's M7 online-mode
    /// verification harness.
    #[arg(long)]
    pub(crate) mint_rewo_env: bool,
}

/// `--mint-rewo-env`: refresh the active account's Minecraft token from
/// the persisted MS refresh token and print the Rewo env handoff, one
/// `KEY=value` per line on stdout (everything else goes to stderr via the
/// logger). Returns a process exit code.
pub(crate) fn mint_rewo_env() -> i32 {
    let store = auth::persistence::load_store();
    let Some(account) = store.active_account() else {
        log::error!(
            "mint-rewo-env: no active account in auth.toml — sign in via the launcher first"
        );
        return 1;
    };
    log::info!(
        "mint-rewo-env: refreshing Minecraft token for {}…",
        account.name
    );
    match auth::chain::run_chain_from_refresh(&account.ms_refresh_token, |stage| {
        log::info!("mint-rewo-env: {}", stage.label());
    }) {
        Ok(fresh) => {
            println!("REWO_USERNAME={}", fresh.name);
            println!("REWO_UUID={}", fresh.uuid);
            println!("REWO_ACCESS_TOKEN={}", fresh.minecraft_token);
            0
        }
        Err(e) => {
            log::error!("mint-rewo-env: auth chain failed: {e:?}");
            1
        }
    }
}
