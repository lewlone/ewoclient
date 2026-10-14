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
    let mut store = auth::persistence::load_store();
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
    let refresh_with = account.ms_refresh_token.clone();
    match auth::chain::run_chain_from_refresh(&refresh_with, |stage| {
        log::info!("mint-rewo-env: {}", stage.label());
    }) {
        Ok(fresh) => {
            // Microsoft rolls the refresh token on use and may invalidate the
            // old one, so the new one has to reach auth.toml or the next
            // launcher start finds a dead token and signs the account out.
            if apply_rotation(&mut store, &fresh) {
                log::info!("mint-rewo-env: refresh token rotated — saving it to auth.toml");
                auth::persistence::save_store(&store);
            }
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

/// Fold a freshly refreshed account into `store`: if its refresh token
/// differs from the stored one (Microsoft rotates it on use), store the new
/// one on the matching account. Returns whether the store changed.
fn apply_rotation(
    store: &mut auth::persistence::AccountStore,
    fresh: &auth::MinecraftAccount,
) -> bool {
    if fresh.ms_refresh_token.is_empty() {
        return false;
    }
    let Some(slot) = store.accounts.iter_mut().find(|a| a.uuid == fresh.uuid) else {
        return false;
    };
    if slot.ms_refresh_token == fresh.ms_refresh_token {
        return false;
    }
    slot.ms_refresh_token = fresh.ms_refresh_token.clone();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use auth::persistence::AccountStore;
    use auth::MinecraftAccount;

    fn account(uuid: &str, refresh: &str) -> MinecraftAccount {
        MinecraftAccount {
            name: format!("n-{uuid}"),
            uuid: uuid.into(),
            minecraft_token: String::new(),
            minecraft_token_expires_at: None,
            ms_refresh_token: refresh.into(),
            social_token: Some("social".into()),
        }
    }

    #[test]
    fn a_rotated_refresh_token_replaces_the_stored_one() {
        let mut store = AccountStore::default();
        store.upsert(account("a", "old-a"));
        store.upsert(account("b", "old-b"));
        assert!(apply_rotation(&mut store, &account("a", "new-a")));
        assert_eq!(store.accounts[0].ms_refresh_token, "new-a");
        assert_eq!(store.accounts[0].social_token.as_deref(), Some("social"));
        assert_eq!(
            store.accounts[1].ms_refresh_token, "old-b",
            "others untouched"
        );
    }

    #[test]
    fn an_unchanged_empty_or_unknown_refresh_token_changes_nothing() {
        let mut store = AccountStore::default();
        store.upsert(account("a", "same"));
        assert!(!apply_rotation(&mut store, &account("a", "same")));
        assert!(!apply_rotation(&mut store, &account("a", "")));
        assert!(!apply_rotation(&mut store, &account("zzz", "new")));
        assert_eq!(store.accounts[0].ms_refresh_token, "same");
    }
}
