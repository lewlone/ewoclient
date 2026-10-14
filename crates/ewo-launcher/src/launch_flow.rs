//! Launch orchestration: the `App` methods that start a game, wait on its
//! preparation (token refresh, JRE fetch, background `prepare`), and finish
//! the launch, plus the pending-launch records they pass between frames.
//! Moved out of `main.rs` unchanged.

use super::*;

/// A launch click whose JRE wasn't available — we kicked off a runtime
/// fetch and will retry once it lands.
#[derive(Debug, Clone)]
pub(crate) struct PendingRelaunch {
    pub(crate) instance_idx: usize,
    pub(crate) instance_name: String,
    pub(crate) instance_meta: String,
    /// Major version the missing JRE is for. The retry only fires when
    /// the runtime service emits `Done { major }` matching this value.
    pub(crate) waiting_for_major: u32,
}

/// A launch held back until the active account's Minecraft token has been
/// refreshed (it was missing or close to expiry). `App::poll_auth_launch`
/// starts the refresh, waits for it off the UI thread, then re-enters the
/// launch, or fails it visibly; a signed-in account never drops to
/// offline.
#[derive(Debug, Clone)]
pub(crate) struct PendingAuthLaunch {
    pub(crate) idx: usize,
    pub(crate) inst_name: String,
    pub(crate) inst_meta: String,
    /// The account the launch is for. A switch of the active account while
    /// waiting cancels the launch rather than launching as someone else.
    pub(crate) uuid: String,
    /// Whether this launch has started its refresh yet (one attempt).
    pub(crate) refresh_started: bool,
    /// The auth stage label last mirrored into the launch log.
    pub(crate) last_stage: &'static str,
}

/// What to do if a launch's background preparation fails.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PrepareFail {
    /// Fall back to the synthetic launch animation (plain Launch click).
    Synthetic,
    /// Show this line + an error exit on the launching screen (retries).
    Error(&'static str),
}

/// The launching screen's meta line: the instance's "<LOADER> · <version>",
/// then, for a JVM instance, the Java runtime once one has been picked and
/// the heap. A native (Rewo) instance has neither.
pub(crate) fn launch_meta(
    inst: &ewo_render::screens::instances::Instance,
    java_major: Option<u32>,
    ram_gb: i32,
) -> String {
    if matches!(
        inst.loader,
        ewo_render::screens::instances::InstanceLoader::Native
    ) {
        return inst.version.clone();
    }
    match java_major {
        Some(major) => format!("{} · JAVA {} · {} GB", inst.version, major, ram_gb),
        None => format!("{} · {} GB", inst.version, ram_gb),
    }
}

/// A launch whose blocking preparation runs on a worker thread; the result
/// arrives on `rx` and the UI thread finishes the launch (JRE pick, plan,
/// spawn) — see `App::poll_prepare`.
pub(crate) struct PendingPrepare {
    pub(crate) rx: std::sync::mpsc::Receiver<Result<launch::prepare::Prepared, String>>,
    pub(crate) idx: usize,
    pub(crate) inst_name: String,
    pub(crate) inst_meta: String,
    pub(crate) fail: PrepareFail,
}

impl App {
    /// Attempt a real launch for the instance at `idx`. Returns `true` if a
    /// real launch is under way (Native spawned, or a JVM launch handed to
    /// the background preparer); `false` if we should fall back right away
    /// (instance not Ready, manifest missing, …). A JVM launch finishes in
    /// [`Self::poll_prepare`] once its preparation arrives; a failure there
    /// is handled per `fail`.
    pub(crate) fn try_real_launch(
        &mut self,
        idx: usize,
        inst_name: &str,
        inst_meta: &str,
        time: f32,
        fail: PrepareFail,
    ) -> bool {
        // E6: apply any bundled-mod toggles made in the in-game overlay last
        // session, before we read the instance's mod state for this launch.
        if overlay_mods::apply_overrides(&mut self.instances, idx) {
            persistence::save_instances(&self.instances);
        }
        let inst = match self.instances.get(idx) {
            Some(i) => i.clone(),
            None => return false,
        };
        if inst.status != ewo_render::screens::instances::InstanceStatus::Ready {
            log::warn!(
                "launch: \"{}\" is Pending (download not done) — falling back",
                inst.name
            );
            return false;
        }
        // A signed-in account launches with a live Minecraft token or not
        // at all: refresh first when it is missing or near expiry, and
        // fail visibly when none can be had. Only a launcher with no
        // account launches offline.
        match self.auth.launch_auth() {
            auth::LaunchAuth::Ready | auth::LaunchAuth::Offline => {}
            auth::LaunchAuth::NeedsRefresh => {
                let Some(uuid) = self.auth.active().map(|a| a.uuid.clone()) else {
                    return false;
                };
                self.launching.enter_real(time, inst_name, inst_meta);
                self.launching.push_real_line(
                    screens::RealSeverity::Info,
                    "[ewo] refreshing your Microsoft session…".into(),
                    time,
                );
                self.pending_auth = Some(PendingAuthLaunch {
                    idx,
                    inst_name: inst_name.to_string(),
                    inst_meta: inst_meta.to_string(),
                    uuid,
                    refresh_started: false,
                    // The line above already says this; don't repeat it.
                    last_stage: auth::chain::ChainStage::RefreshingMicrosoft.label(),
                });
                self.poll_auth_launch(time);
                return true;
            }
            auth::LaunchAuth::Unavailable(msg) => {
                self.launching.enter_real(time, inst_name, inst_meta);
                self.fail_launch_visibly(msg, time);
                return true;
            }
        }
        // E6: refresh the in-game MODS view's snapshot of the bundled mods.
        overlay_mods::write_catalog(&inst);
        // F5c: resolve the active profile's keybinds for the in-game mod.
        overlay_mods::write_keybinds(&inst.id);
        // The version *string* comes from the meta, formatted as
        // "<LOADER> · <version>". Strip the loader prefix.
        let version_id = inst.version.rsplit(" · ").next().unwrap_or(&inst.version);
        // Native (Rewo) instances skip the entire JVM pipeline — no
        // manifest, no libraries, no natives, no JRE. Spawn the rewo
        // binary with the REWO_* env contract (REWO_PLAN.md §9.1). With a
        // server join active this is `rewo live` — the real playable
        // client (M3+); without one there's nothing to connect to, so the
        // bare M0 window opens as a placeholder.
        if matches!(
            inst.loader,
            ewo_render::screens::instances::InstanceLoader::Native
        ) {
            let Some(program) = launch::find_rewo_binary() else {
                log::warn!("launch: rewo binary not found next to the launcher — falling back");
                return false;
            };
            let mut envs = vec![("REWO_VERSION".to_string(), version_id.to_string())];
            // The instance's game folder: rewo's options.txt lives there,
            // and `spawn_native` makes it the child's working directory.
            if let Some(dir) = downloads::paths::instance_dir(&inst.id) {
                envs.push(("REWO_GAME_DIR".into(), dir.display().to_string()));
            }
            if let Some(account) = self.auth.active() {
                if !account.minecraft_token.is_empty() {
                    envs.push(("REWO_USERNAME".into(), account.name.clone()));
                    envs.push(("REWO_UUID".into(), account.uuid.clone()));
                    envs.push(("REWO_ACCESS_TOKEN".into(), account.minecraft_token.clone()));
                }
            }
            let mut args: Vec<String> = Vec::new();
            if let Some(addr) = self.active_server.clone() {
                let (host, port) = match addr.rsplit_once(':') {
                    Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(25565)),
                    None => (addr.clone(), 25565),
                };
                envs.push(("REWO_SERVER".into(), addr.clone()));
                args = launch::native_client_args(&host, port, version_id);
                log::info!("launch: rewo live → {}", addr);
            }
            let (tx, rx) = std::sync::mpsc::channel::<launch::LaunchEvent>();
            let _ = launch::spawn_native(
                launch::NativePlan {
                    program,
                    args,
                    envs,
                },
                tx,
            );
            self.launch_rx = Some(rx);
            self.active_launch_instance_id = Some(inst.id.clone());
            self.launching.enter_real(time, inst_name, inst_meta);
            log::info!(
                "launch: rewo spawned for \"{}\" ({})",
                inst.name,
                version_id
            );
            return true;
        }
        let manifest = match self.versions.manifest() {
            Some(m) => m,
            None => {
                log::warn!("launch: master manifest not loaded — falling back");
                return false;
            }
        };
        let entry = match manifest.entry(version_id) {
            Some(e) => e.clone(),
            None => {
                log::warn!(
                    "launch: {} not in master manifest — falling back",
                    version_id
                );
                return false;
            }
        };
        // The network + disk work (manifests, library downloads, natives
        // extraction, zombie reaping) runs on a worker so the UI never
        // blocks; `poll_prepare` finishes the launch.
        let rx = launch::prepare::spawn(launch::prepare::PrepareJob {
            entry,
            version_id: version_id.to_string(),
            instance_id: inst.id.clone(),
            loader: inst.loader.clone(),
            mods: inst.mods.clone(),
        });
        self.launching.enter_real(time, inst_name, inst_meta);
        self.launching.push_real_line(
            screens::RealSeverity::Info,
            "[ewo] preparing game files…".into(),
            time,
        );
        self.pending_prepare = Some(PendingPrepare {
            rx,
            idx,
            inst_name: inst_name.to_string(),
            inst_meta: inst_meta.to_string(),
            fail,
        });
        true
    }

    /// Drain the background launch preparation, if any, and finish the
    /// launch on success. Called once per frame.
    pub(crate) fn poll_prepare(&mut self, time: f32) {
        let result = match self.pending_prepare.as_ref().map(|p| p.rx.try_recv()) {
            None | Some(Err(std::sync::mpsc::TryRecvError::Empty)) => return,
            Some(Ok(r)) => r,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                Err("launch preparation thread died".to_string())
            }
        };
        let Some(pending) = self.pending_prepare.take() else {
            return;
        };
        let prepared = match result {
            Ok(p) => p,
            Err(msg) => {
                log::warn!("launch: {} — falling back", msg);
                self.fail_launch(&pending, &msg, time);
                return;
            }
        };
        if !self.finish_launch(&pending, prepared, time) {
            self.fail_launch(&pending, "could not start the game — see log", time);
        }
    }

    /// Replace the Java-runtime dropdown's options with the detected
    /// runtimes' labels (Auto is prepended here).
    pub(crate) fn set_jre_labels(&mut self, detected: Vec<String>) {
        self.jre_labels = screens::instances::java_runtime_options(&detected);
    }

    /// Deliver the warm-up scan's dropdown labels once, then mirror into
    /// the renderer's prefs (guarded so a small Vec isn't cloned every
    /// frame). Called once per frame.
    pub(crate) fn poll_jre_labels(&mut self) {
        if let Some(rx) = &self.jre_labels_rx {
            if let Ok(detected) = rx.try_recv() {
                log::info!("jre labels: {} runtime(s) listed", detected.len());
                self.jre_labels_rx = None;
                self.set_jre_labels(detected);
            }
        }
        if self.instance_prefs.jre_runtime_labels != self.jre_labels {
            self.instance_prefs.jre_runtime_labels = self.jre_labels.clone();
        }
    }

    /// After a bundled JRE landed: once the background JRE re-scan reports
    /// back, retry the launch that was waiting for that Java major.
    /// Called once per frame.
    pub(crate) fn poll_jre_rescan(&mut self, time: f32) {
        let major = match self.jre_rescan_rx.as_ref().map(|rx| rx.try_recv()) {
            None | Some(Err(std::sync::mpsc::TryRecvError::Empty)) => return,
            Some(Ok(major)) => Some(major),
            // The rescan thread couldn't start; the cache was invalidated
            // instead, so the retry below rescans lazily.
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => None,
        };
        self.jre_rescan_rx = None;
        if major.is_some() {
            // The worker already swapped the fresh scan into the cache,
            // so this is a cheap read. (A failed spawn invalidated the
            // cache instead; skip, rather than scan on the UI thread.)
            self.set_jre_labels(launch::jre::labels());
        }
        let Some(p) = self.pending_relaunch.clone() else {
            return;
        };
        if major.is_some_and(|m| m != p.waiting_for_major) {
            return;
        }
        self.pending_relaunch = None;
        let ok = self.try_real_launch(
            p.instance_idx,
            &p.instance_name,
            &p.instance_meta,
            time,
            PrepareFail::Error("retry failed after JRE install"),
        );
        if !ok {
            self.fail_launch_visibly("retry failed after JRE install", time);
        }
    }

    /// End a real launch with an error line and the error exit, whatever
    /// the launch's `PrepareFail` mode: an auth failure must never be
    /// papered over by the synthetic animation.
    pub(crate) fn fail_launch_visibly(&mut self, msg: &str, time: f32) {
        log::warn!("launch: {}", msg);
        self.launching
            .push_real_line(screens::RealSeverity::Warn, format!("[ewo] {}", msg), time);
        self.launching.set_real_exit(Some(127), time);
    }

    /// Drive a launch waiting on a Minecraft-token refresh (see
    /// [`PendingAuthLaunch`]). Called once per frame; the refresh itself
    /// runs on the auth service's worker thread.
    pub(crate) fn poll_auth_launch(&mut self, time: f32) {
        let Some(pending) = self.pending_auth.as_ref() else {
            return;
        };
        let refresh_started = pending.refresh_started;
        let same_account =
            self.auth.active().map(|a| a.uuid.as_str()) == Some(pending.uuid.as_str());
        if !same_account {
            self.pending_auth = None;
            self.fail_launch_visibly("the active account changed; launch again", time);
            return;
        }
        // Any chain still running (ours, or an earlier one) — wait for it,
        // mirroring its stage labels into the launch log.
        if self.auth.is_busy() {
            if let AuthOp::Working(stage) = self.auth.op() {
                let stage: &'static str = stage;
                let changed = self
                    .pending_auth
                    .as_mut()
                    .is_some_and(|p| std::mem::replace(&mut p.last_stage, stage) != stage);
                if changed {
                    self.launching.push_real_line(
                        screens::RealSeverity::Info,
                        format!("[ewo] {}", stage),
                        time,
                    );
                }
            }
            return;
        }
        match self.auth.launch_auth() {
            auth::LaunchAuth::Ready | auth::LaunchAuth::Offline => {
                let Some(p) = self.pending_auth.take() else {
                    return;
                };
                log::info!("launch: Microsoft session ready, continuing");
                // Re-enter the launch; the gate now passes. A failure
                // past this point is a real launch failure, shown as one.
                if !self.try_real_launch(
                    p.idx,
                    &p.inst_name,
                    &p.inst_meta,
                    time,
                    PrepareFail::Error("launch failed"),
                ) {
                    self.fail_launch_visibly("could not start the game — see log", time);
                }
            }
            auth::LaunchAuth::NeedsRefresh if !refresh_started => {
                if let Some(p) = self.pending_auth.as_mut() {
                    p.refresh_started = true;
                }
                if !self.auth.start_launch_refresh() {
                    self.pending_auth = None;
                    self.fail_launch_visibly("could not refresh your Microsoft session", time);
                }
            }
            auth::LaunchAuth::NeedsRefresh => {
                // Our refresh ran and the token is still unusable.
                self.pending_auth = None;
                let why = match self.auth.op() {
                    AuthOp::Failed(e) => format!("{:?}", e),
                    _ => "no token returned".to_string(),
                };
                log::warn!("launch: Microsoft session refresh failed: {}", why);
                self.fail_launch_visibly(
                    "could not refresh your Microsoft session; check your connection or sign in again in Settings → Account",
                    time,
                );
            }
            auth::LaunchAuth::Unavailable(msg) => {
                self.pending_auth = None;
                self.fail_launch_visibly(msg, time);
            }
        }
    }

    pub(crate) fn fail_launch(&mut self, pending: &PendingPrepare, msg: &str, time: f32) {
        // A missing/unreadable EwoLoader manifest is always a visible launch
        // error, never the synthetic animation or a vanilla launch.
        if launch::prepare::is_loader_error(msg) {
            self.launching.push_real_line(
                screens::RealSeverity::Warn,
                format!(
                    "[ewo] the EwoLoader could not be loaded, so the game was not started ({})",
                    msg
                ),
                time,
            );
            self.launching.set_real_exit(Some(127), time);
            return;
        }
        match pending.fail {
            PrepareFail::Synthetic => {
                log::info!(
                    "launch: falling back to synthetic for \"{}\"",
                    pending.inst_name
                );
                self.launching
                    .enter(time, &pending.inst_name, &pending.inst_meta);
            }
            PrepareFail::Error(line) => {
                self.launching.push_real_line(
                    screens::RealSeverity::Warn,
                    format!("[ewo] {} ({})", line, msg),
                    time,
                );
                self.launching.set_real_exit(Some(127), time);
            }
        }
    }

    /// UI-thread half of a JVM launch: pick a JRE (or start fetching one),
    /// build the plan and spawn. Returns `false` on a failure the caller
    /// should surface.
    pub(crate) fn finish_launch(
        &mut self,
        pending: &PendingPrepare,
        prepared: launch::prepare::Prepared,
        time: f32,
    ) -> bool {
        let Some(inst) = self.instances.get(pending.idx).cloned() else {
            return false;
        };
        let (inst_name, inst_meta) = (pending.inst_name.as_str(), pending.inst_meta.as_str());
        let version_id = inst.version.rsplit(" · ").next().unwrap_or(&inst.version);
        let launch::prepare::Prepared {
            pv,
            disabled_mod_ids,
        } = prepared;
        // Pick a JRE matching the per-version manifest's
        // `javaVersion.majorVersion`. Falls back to whatever's first in
        // the detected list if the manifest doesn't specify (legacy
        // 1.8.9-era).
        let required_major = pv
            .java_version
            .as_ref()
            .map(|j| j.major_version)
            .unwrap_or(8);
        // A per-instance pin (dropdown index >= 1) selects that exact
        // runtime, but only while it still exists AND still satisfies the
        // manifest's major requirement; a stale pin (JRE uninstalled, list
        // reshuffled) falls back to auto with a warning rather than
        // launching something wrong. `detect_jres` is the warmed cache.
        let pinned =
            launch::jre::resolve_pin(&launch::detect_jres(), inst.java_runtime, required_major);
        let jvm_path = match pinned.or_else(|| launch::pick_jre(required_major)) {
            Some(j) => {
                log::info!(
                    "launch: picked Java {} at {} (required ≥ {})",
                    j.major,
                    j.path.display(),
                    required_major
                );
                // Name the runtime actually chosen, now that it is known.
                self.launching.instance_meta =
                    launch_meta(&inst, Some(j.major), self.instance_prefs.ram.value as i32);
                j.path.clone()
            }
            None => {
                let installed: Vec<u32> = launch::detect_jres().iter().map(|j| j.major).collect();
                log::info!(
                    "launch: no Java {} installed (have: {:?}) — fetching from Adoptium",
                    required_major,
                    installed
                );
                // Kick off the bundled-JRE download and record this launch
                // as pending. The per-frame runtime poll retries once the
                // fetch completes.
                self.launching.push_real_line(
                    screens::RealSeverity::Info,
                    format!(
                        "[ewo] Java {} not installed — fetching Eclipse Temurin from Adoptium…",
                        required_major
                    ),
                    time,
                );
                self.runtime_log_bucket = 0;
                self.runtime.start_fetch(required_major);
                self.pending_relaunch = Some(PendingRelaunch {
                    instance_idx: pending.idx,
                    instance_name: inst_name.to_string(),
                    instance_meta: inst_meta.to_string(),
                    waiting_for_major: required_major,
                });
                return true;
            }
        };
        // Use the signed-in Microsoft account's profile; offline mode
        // (placeholder UUID + token) only when no account is signed in.
        // Online mode unlocks multiplayer + skin sync; offline mode is
        // singleplayer/LAN only. `try_real_launch` already refreshed a
        // stale token, so anything but Ready/Offline here means the token
        // went stale during preparation (or a JRE download): fail visibly
        // rather than launch a signed-in user offline.
        let profile = match (self.auth.launch_auth(), self.auth.active()) {
            (auth::LaunchAuth::Ready, Some(account)) => {
                log::info!(
                    "launch: using signed-in profile {} (token live)",
                    account.name
                );
                launch::LaunchProfile {
                    username: account.name.clone(),
                    uuid: account.uuid.clone(),
                    access_token: account.minecraft_token.clone(),
                    user_type: "msa".to_string(),
                }
            }
            (auth::LaunchAuth::Offline, _) => {
                log::info!("launch: offline profile (no account signed in)");
                launch::LaunchProfile::offline(&inst.name)
            }
            (decision, _) => {
                let msg = match decision {
                    auth::LaunchAuth::Unavailable(m) => m,
                    _ => "your Microsoft session expired while the game was being prepared; launch again",
                };
                log::warn!("launch: no usable Minecraft token ({decision:?})");
                self.fail_launch_visibly(msg, time);
                return true;
            }
        };
        let mut plan = match launch::build(&pv, &inst.id, inst.ram, &profile, jvm_path) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("launch: plan build failed: {}", e);
                return false;
            }
        };
        // Append the disabled-mods JVM arg so the loader's BundledMods
        // verification subtracts them from the expected set and the
        // discovery filter skips them. -D args go before main-class +
        // game args, which is exactly where jvm_args ends up.
        if !disabled_mod_ids.is_empty() {
            plan.jvm_args.push(format!(
                "-Dfabric.debug.disableModIds={}",
                disabled_mod_ids.join(",")
            ));
        }
        // Tell ewo-hud's mixin plugin which MC version this is so it can
        // select the right version-specific mixin set (26.2 renamed the
        // frame hook + crosshair/fire-overlay targets). Authoritative over
        // the plugin's Fabric-loader fallback probe.
        plan.jvm_args
            .push(format!("-Dewo.mc.version={}", version_id));
        // A bundled HUD native next to the launcher (see package.ps1) wins
        // over the mod's dev fallback (the repo's cargo target dir).
        if let Some(dll) = launch::find_hud_native() {
            plan.jvm_args
                .push(format!("-Dewo.hud.nativePath={}", dll.display()));
        }
        // H6: if this launch was initiated as a server-join (main-menu
        // server widget or a friend's "Join"), auto-connect on boot via the
        // modern quick-play arg. The address is `host:port`; the client
        // defaults the port to 25565 when omitted. `--quickPlayMultiplayer`
        // is the 1.20+ replacement for the removed `--server`/`--port` pair,
        // which is right for the 26.x line this launcher targets.
        if let Some(addr) = self.active_server.clone() {
            plan.game_args.push("--quickPlayMultiplayer".to_string());
            plan.game_args.push(addr.clone());
            log::info!("launch: quick-play join → {}", addr);
        }
        let (tx, rx) = std::sync::mpsc::channel::<launch::LaunchEvent>();
        let _ = launch::spawn_jvm(plan, tx);
        self.launch_rx = Some(rx);
        self.active_launch_instance_id = Some(inst.id.clone());
        self.launching.push_real_line(
            screens::RealSeverity::Info,
            "[ewo] starting jvm…".into(),
            time,
        );
        log::info!(
            "launch: real JVM spawned for \"{}\" ({})",
            inst.name,
            version_id
        );
        true
    }

    /// Whether a launch is already running or being prepared. When the
    /// running game is a zombie (its window closed but the process lingers)
    /// it's reaped here and the launch slot is freed; a live game is never
    /// killed — the new launch is refused instead.
    pub(crate) fn launch_slot_busy(&mut self, time: f32) -> bool {
        use launch_slot::{decide, SlotDecision, SlotInput};
        // Gather the input — the only place OS process state is read.
        let tracked = self.active_launch;
        let input = if self.pending_prepare.is_some()
            || self.pending_relaunch.is_some()
            || self.pending_auth.is_some()
        {
            SlotInput::Preparing
        } else if self.launch_rx.is_none() {
            SlotInput::NeverLaunched
        } else {
            match tracked {
                // Spawned but not yet reported `Started`.
                None => SlotInput::Starting,
                Some(t) => match launch::reaper::state(&t) {
                    launch::reaper::GameState::Gone => SlotInput::Gone,
                    launch::reaper::GameState::Running { visible_window } => SlotInput::Running {
                        zombie: launch::reaper::is_zombie(
                            visible_window,
                            self.active_launch_window_seen,
                            time - self.active_launch_started_at,
                        ),
                    },
                },
            }
        };
        match decide(input) {
            SlotDecision::Busy(reason) => {
                // The running-game line names the pid, which the pure
                // decision can't carry; it is appended here.
                match (input, tracked) {
                    (SlotInput::Running { .. }, Some(t)) => {
                        log::warn!("launch: {reason} (pid {}) — ignoring launch request", t.pid)
                    }
                    _ => log::warn!("launch: {reason} — ignoring launch request"),
                }
                return true;
            }
            // Nothing was ever launched: nothing to clean up either.
            SlotDecision::Free => return false,
            SlotDecision::ReapThenFree => {
                // Reap first; forgetting the record comes below.
                if let Some(t) = tracked {
                    launch::reaper::reap(&t);
                }
            }
            SlotDecision::ForgetThenFree => {}
        }
        // The game is gone (or was a zombie): drop its OS-side record and
        // clear the launch fields so the next click gets a fresh slot.
        if let Some(t) = tracked {
            launch::reaper::forget(&t);
        }
        self.launch_rx = None;
        self.active_launch = None;
        self.active_launch_instance_id = None;
        false
    }

    /// Shared launch entry point for the Launch button and the H6 server-join
    /// paths (main-menu server widget + friend "Join"). `server` is the
    /// `host:port` to auto-connect to, or `None` for a plain launch.
    ///
    /// Resolves the instance's display name + meta, gates out a `Pending`
    /// (mid-download) or missing instance, stamps `last_played`, then runs
    /// the real launch (falling back to the synthetic animation if anything
    /// upstream is missing) and switches to the Launching screen with the
    /// celebrate burst.
    pub(crate) fn start_launch(&mut self, idx: usize, server: Option<String>, time: f32) {
        use ewo_render::screens::instances::InstanceStatus;
        // A still-downloading instance can't launch; clicking Launch on one
        // whose download failed (or never started this session) retries it.
        if matches!(self.instances.get(idx), Some(i) if i.status == InstanceStatus::Pending) {
            self.retry_download(idx);
        }
        // Resolve name + meta and gate Pending/missing instances. The
        // immutable borrow ends here so the rest can mutate `self`.
        let (inst_name, inst_meta) = match self.instances.get(idx) {
            Some(i) if i.status != InstanceStatus::Pending => (
                i.name.clone(),
                launch_meta(i, None, self.instance_prefs.ram.value as i32),
            ),
            _ => {
                log::warn!(
                    "launch: instance #{} not launchable (missing or still downloading)",
                    idx
                );
                return;
            }
        };
        // One game at a time: a live game is never killed by a second
        // launch click — the click is ignored. Only a zombie is reaped.
        if self.launch_slot_busy(time) {
            return;
        }
        // Dev affordance: `EWO_DEV_SERVER=host:port` points plain Launch
        // clicks at a server without going through a join flow — e.g. the
        // local offline Rewo test server (127.0.0.1:25599) for eyeballing
        // the Native → `rewo live` path. Join flows (Some) always win.
        let server = server.or_else(|| {
            std::env::var("EWO_DEV_SERVER")
                .ok()
                .filter(|s| !s.is_empty())
        });
        if let Some(addr) = &server {
            log::info!(
                "launch: server-join → {} (instance \"{}\")",
                addr,
                inst_name
            );
        } else {
            log::info!("vbtn: Launch clicked → launching \"{}\"", inst_name);
        }
        self.active_server = server;

        // Stamp last_played + persist so the timestamp survives a restart.
        if let Some(inst) = self.instances.get_mut(idx) {
            inst.last_played = "just now".to_string();
            inst.last_played_at = screens::instances::current_unix_seconds();
        }
        persistence::save_instances(&self.instances);

        // A previous launcher session may have left a zombie game process
        // recorded on disk; reap it (only if it's ours, still that exact
        // process, and windowless — a live game is left alone).
        let reaped = launch::reaper::reap_recorded_zombies();
        if reaped > 0 {
            log::info!(
                "launch: reaped {} zombie game process(es) from an earlier run",
                reaped
            );
        }

        // Real launch fires only when the instance is Ready + the manifest
        // resolves; otherwise fall back to the synthetic animation so the
        // user still gets feedback.
        let real_launched =
            self.try_real_launch(idx, &inst_name, &inst_meta, time, PrepareFail::Synthetic);
        if !real_launched {
            log::info!("launch: falling back to synthetic for \"{}\"", inst_name);
            self.launching.enter(time, &inst_name, &inst_meta);
        }
        self.screen = Screen::Launching;
        self.launch_button = VbtnState::default();
        self.prefs.close_dropdowns();
        self.instance_prefs.close_dropdowns();
        if let Some(bd) = self.backdrop.as_mut() {
            bd.celebrate(true);
        }
        self.celebrate_until = Some(time + 4.5);
    }

    /// The name and meta line the launching screen shows for a synthetic
    /// (demo or dev-simulated) launch: the selected instance, or a fixed
    /// placeholder when the list is empty.
    pub(crate) fn demo_launch_label(&self) -> (String, String) {
        self.instances
            .get(self.instance_prefs.selected)
            .map(|i| {
                (
                    i.name.clone(),
                    launch_meta(i, None, self.instance_prefs.ram.value as i32),
                )
            })
            .unwrap_or_else(|| {
                (
                    "Velvet Hours".to_string(),
                    "VANILLA · 1.21 · ADOPTIUM 21".to_string(),
                )
            })
    }

    /// (Re)start the download job for a Pending instance whose job failed or
    /// never ran this session. No-op while a job for it is in flight.
    pub(crate) fn retry_download(&mut self, idx: usize) {
        let Some(inst) = self.instances.get(idx) else {
            return;
        };
        let version_id = inst.version.rsplit(" · ").next().unwrap_or(&inst.version);
        let loader_spec = loader_spec_for(&inst.loader, version_id);
        let Some(manifest) = self.versions.manifest() else {
            log::warn!(
                "downloads: master manifest not loaded — can't retry {}",
                version_id
            );
            return;
        };
        let Some(entry) = manifest.entry(version_id).cloned() else {
            log::warn!(
                "downloads: {} not in master manifest — can't retry",
                version_id
            );
            return;
        };
        self.downloads.start(entry, loader_spec);
    }
}

/// Dump the launching screen's in-memory log to
/// `<config>/EwoClient/instances/<id>/logs/<timestamp>.log`.
/// Best-effort — failures log a warning but don't surface. Each line is
/// prefixed with its severity tag so stderr lines stay distinguishable
/// from stdout when grepping.
pub(crate) fn persist_launch_log(
    instance_id: &str,
    instance_name: &str,
    lines: &[ewo_render::screens::launching::RealLogLine],
    exit_code: Option<i32>,
) {
    use std::io::Write;
    if lines.is_empty() {
        return;
    }
    let Some(mut path) = downloads::paths::instance_dir(instance_id) else {
        log::warn!("logs: instance dir unresolvable for {}", instance_name);
        return;
    };
    path.push("logs");
    if let Err(e) = std::fs::create_dir_all(&path) {
        log::warn!("logs: mkdir {} failed: {}", path.display(), e);
        return;
    }
    // Filename uses a sortable timestamp so newest logs sort to the
    // bottom alphabetically.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    path.push(format!("launch_{}.log", ts));
    let file = match std::fs::File::create(&path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("logs: create {} failed: {}", path.display(), e);
            return;
        }
    };
    let mut w = std::io::BufWriter::new(file);
    let _ = writeln!(
        w,
        "# EwoClient launch log — instance \"{}\" — exit code {:?}",
        instance_name, exit_code
    );
    let _ = writeln!(w, "# {} lines\n", lines.len());
    for line in lines {
        let tag = match line.severity {
            ewo_render::screens::RealSeverity::Info => "OUT",
            ewo_render::screens::RealSeverity::Warn => "ERR",
        };
        let _ = writeln!(w, "[{}] {:.2}s {}", tag, line.at, line.text);
    }
    log::info!("logs: wrote {} lines to {}", lines.len(), path.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ewo_render::screens::instances::{Instance, InstanceLoader};

    fn inst(loader: InstanceLoader) -> Instance {
        let mut i = Instance::new(
            "n".into(),
            "FABRIC · 1.21".into(),
            "never".into(),
            Vec::new(),
        );
        i.loader = loader;
        i
    }

    #[test]
    fn jvm_launch_meta_adds_runtime_once_picked_then_heap() {
        let i = inst(InstanceLoader::Vanilla);
        assert_eq!(launch_meta(&i, None, 4), "FABRIC · 1.21 · 4 GB");
        assert_eq!(
            launch_meta(&i, Some(21), 4),
            "FABRIC · 1.21 · JAVA 21 · 4 GB"
        );
    }

    #[test]
    fn native_launch_meta_is_just_the_version() {
        let i = inst(InstanceLoader::Native);
        assert_eq!(launch_meta(&i, None, 4), "FABRIC · 1.21");
        assert_eq!(launch_meta(&i, Some(21), 4), "FABRIC · 1.21");
    }
}
