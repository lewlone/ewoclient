//! Launch orchestration: the `App` methods that start a game, wait on its
//! preparation (token refresh, JRE fetch, background `prepare`), and finish
//! the launch, plus the pending-launch records they pass between frames.
//! Moved out of `main.rs` unchanged.

use super::*;

/// A launch click whose JRE wasn't available — we kicked off a runtime
/// fetch and will retry once it lands.
#[derive(Debug, Clone)]
pub(crate) struct PendingRelaunch {
    /// The instance's stable folder id, never its list index: the list can
    /// shift (delete, create, reorder) while the launch waits.
    pub(crate) instance_id: String,
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
    /// Stable instance id (see [`PendingRelaunch::instance_id`]).
    pub(crate) instance_id: String,
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

/// Shown when the instance a waiting launch belongs to left the list.
pub(crate) const INSTANCE_GONE_MSG: &str =
    "the instance was removed while the launch was waiting; launch cancelled";

/// The list index of the instance with folder id `id`. Launches that wait
/// (token refresh, file preparation, a JRE download) hold the id and
/// resolve it here at every step, because the list shifts when any other
/// instance is deleted, created or reordered.
pub(crate) fn index_of_instance(
    instances: &[ewo_render::screens::instances::Instance],
    id: &str,
) -> Option<usize> {
    instances.iter().position(|i| i.id == id)
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

/// Shown on the launching screen when a Native (Rewo) launch goes ahead
/// without a Microsoft session.
pub(crate) const NATIVE_UNSIGNED_WARNING: &str =
    "not signed in to Microsoft \u{2014} online servers will reject you";

/// What the sign-in gate decides for a launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthGate {
    /// Launch with the active account's live token, or offline (no account).
    Proceed,
    /// The token is missing or near expiry: refresh it first.
    RefreshFirst,
    /// Do not launch; show this error.
    Fail(&'static str),
    /// Launch a Native (Rewo) game without a token, with a visible warning.
    ProceedUnsigned,
}

/// The message for a refresh that ran and left no usable token.
pub(crate) const REFRESH_FAILED_MSG: &str = "could not refresh your Microsoft session; check your connection or sign in again in Settings \u{2192} Account";

/// The sign-in gate. A signed-in account never launches a **Java** game
/// offline: it launches with a live token, refreshes first, or fails
/// visibly. A **Native** (Rewo) game targets offline servers (CLAUDE.md,
/// "offline first"), so when no token can be had (`refresh_tried`: the
/// refresh already ran and failed, or there is nothing to refresh with) it
/// launches without one and warns instead of being blocked.
pub(crate) fn auth_gate(
    decision: &auth::LaunchAuth,
    native: bool,
    refresh_tried: bool,
) -> AuthGate {
    match decision {
        auth::LaunchAuth::Ready | auth::LaunchAuth::Offline => AuthGate::Proceed,
        auth::LaunchAuth::NeedsRefresh if !refresh_tried => AuthGate::RefreshFirst,
        auth::LaunchAuth::NeedsRefresh if native => AuthGate::ProceedUnsigned,
        auth::LaunchAuth::NeedsRefresh => AuthGate::Fail(REFRESH_FAILED_MSG),
        auth::LaunchAuth::Unavailable(_) if native => AuthGate::ProceedUnsigned,
        auth::LaunchAuth::Unavailable(msg) => AuthGate::Fail(msg),
    }
}

/// The account handoff environment for a Native launch. A live token passes
/// the profile (name, UUID, token); an unsigned launch passes only the
/// display name (Rewo's offline handoff), never a stale token.
fn native_account_envs(
    account: Option<&auth::MinecraftAccount>,
    unsigned: bool,
) -> Vec<(String, String)> {
    match account {
        None => Vec::new(),
        Some(a) if unsigned => vec![("REWO_USERNAME".into(), a.name.clone())],
        Some(a) if a.minecraft_token.is_empty() => Vec::new(),
        Some(a) => vec![
            ("REWO_USERNAME".into(), a.name.clone()),
            ("REWO_UUID".into(), a.uuid.clone()),
            ("REWO_ACCESS_TOKEN".into(), a.minecraft_token.clone()),
        ],
    }
}

/// A launch whose blocking preparation runs on a worker thread; the result
/// arrives on `rx` and the UI thread finishes the launch (JRE pick, plan,
/// spawn) — see `App::poll_prepare`.
pub(crate) struct PendingPrepare {
    pub(crate) rx: std::sync::mpsc::Receiver<Result<launch::prepare::Prepared, String>>,
    /// Stable instance id (see [`PendingRelaunch::instance_id`]).
    pub(crate) instance_id: String,
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
        let native = matches!(
            inst.loader,
            ewo_render::screens::instances::InstanceLoader::Native
        );
        // (A Native game is not blocked by a missing session: see `auth_gate`.)
        let mut unsigned = false;
        match auth_gate(&self.auth.launch_auth(), native, false) {
            AuthGate::Proceed => {}
            AuthGate::ProceedUnsigned => unsigned = true,
            AuthGate::RefreshFirst => {
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
                    instance_id: inst.id.clone(),
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
            AuthGate::Fail(msg) => {
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
        if native {
            return self.launch_native(&inst, version_id, inst_name, inst_meta, unsigned, time);
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
            instance_id: inst.id.clone(),
            inst_name: inst_name.to_string(),
            inst_meta: inst_meta.to_string(),
            fail,
        });
        true
    }

    /// Spawn the Rewo binary for a Native instance. `unsigned`: the launch
    /// goes ahead without a Microsoft token (see [`auth_gate`]); the
    /// launching screen then says so, since an online server will reject
    /// the player.
    fn launch_native(
        &mut self,
        inst: &ewo_render::screens::instances::Instance,
        version_id: &str,
        inst_name: &str,
        inst_meta: &str,
        unsigned: bool,
        time: f32,
    ) -> bool {
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
        envs.extend(native_account_envs(self.auth.active(), unsigned));
        let mut args: Vec<String> = Vec::new();
        if let Some(addr) = self.active_server.clone() {
            // No port typed stays `None`: the client then follows the
            // `_minecraft._tcp` SRV record, as vanilla's resolver does.
            let (host, port) = launch::split_server_address(&addr);
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
        if unsigned {
            log::warn!("launch: rewo without a Microsoft session (unsigned)");
            self.launching.push_real_line(
                screens::RealSeverity::Warn,
                format!("[ewo] {NATIVE_UNSIGNED_WARNING}"),
                time,
            );
        }
        log::info!(
            "launch: rewo spawned for \"{}\" ({})",
            inst.name,
            version_id
        );
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
        // The instance was removed while its files were being prepared:
        // cancel visibly (never the synthetic fallback, never another
        // instance's launch).
        if self.instance_index(&pending.instance_id).is_none() {
            self.fail_launch_visibly(INSTANCE_GONE_MSG, time);
            return;
        }
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
    /// runtimes (Auto is prepended here), and keep every instance's pin on
    /// its runtime however the list re-sorted.
    pub(crate) fn set_jre_labels(&mut self, detected: Vec<(String, screens::instances::JavaPin)>) {
        let (labels, pins): (Vec<String>, Vec<_>) = detected.into_iter().unzip();
        self.jre_labels = screens::instances::java_runtime_options(&labels);
        self.jre_pins = pins;
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
        if self.instance_prefs.jre_runtime_labels != self.jre_labels
            || self.instance_prefs.jre_runtime_pins != self.jre_pins
        {
            self.instance_prefs.jre_runtime_labels = self.jre_labels.clone();
            self.instance_prefs.jre_runtime_pins = self.jre_pins.clone();
            self.instance_prefs.reselect_java(&self.instances);
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
            self.set_jre_labels(launch::jre::listing());
        }
        let Some(p) = self.pending_relaunch.clone() else {
            return;
        };
        if major.is_some_and(|m| m != p.waiting_for_major) {
            return;
        }
        self.pending_relaunch = None;
        let Some(idx) = self.instance_index(&p.instance_id) else {
            self.fail_launch_visibly(INSTANCE_GONE_MSG, time);
            return;
        };
        let ok = self.try_real_launch(
            idx,
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
        let decision = self.auth.launch_auth();
        let native = self
            .pending_auth
            .as_ref()
            .is_some_and(|p| self.is_native_instance(&p.instance_id));
        match decision {
            auth::LaunchAuth::Ready | auth::LaunchAuth::Offline => {
                let Some(p) = self.pending_auth.take() else {
                    return;
                };
                log::info!("launch: Microsoft session ready, continuing");
                // Re-enter the launch; the gate now passes. A failure
                // past this point is a real launch failure, shown as one.
                let Some(idx) = self.instance_index(&p.instance_id) else {
                    self.fail_launch_visibly(INSTANCE_GONE_MSG, time);
                    return;
                };
                if !self.try_real_launch(
                    idx,
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
                let pending = self.pending_auth.take();
                let why = match self.auth.op() {
                    AuthOp::Failed(e) => format!("{:?}", e),
                    _ => "no token returned".to_string(),
                };
                log::warn!("launch: Microsoft session refresh failed: {}", why);
                self.after_failed_session(pending, &decision, native, time);
            }
            auth::LaunchAuth::Unavailable(_) => {
                let pending = self.pending_auth.take();
                self.after_failed_session(pending, &decision, native, time);
            }
        }
    }

    /// A launch that waited on a session ended without a usable token: a
    /// Java game fails visibly, a Native one launches unsigned with a
    /// warning (see [`auth_gate`]).
    fn after_failed_session(
        &mut self,
        pending: Option<PendingAuthLaunch>,
        decision: &auth::LaunchAuth,
        native: bool,
        time: f32,
    ) {
        match (auth_gate(decision, native, true), pending) {
            (AuthGate::ProceedUnsigned, Some(p)) => {
                let Some(inst) = self.instance_by_id(&p.instance_id).cloned() else {
                    self.fail_launch_visibly(INSTANCE_GONE_MSG, time);
                    return;
                };
                let version_id = inst.version.rsplit(" · ").next().unwrap_or(&inst.version);
                if !self.launch_native(&inst, version_id, &p.inst_name, &p.inst_meta, true, time) {
                    self.fail_launch_visibly("could not start the game — see log", time);
                }
            }
            (AuthGate::Fail(msg), _) => self.fail_launch_visibly(msg, time),
            (_, _) => self.fail_launch_visibly("could not start the game — see log", time),
        }
    }

    /// The list index of the instance with folder id `id`, if it is still
    /// in the list.
    pub(crate) fn instance_index(&self, id: &str) -> Option<usize> {
        index_of_instance(&self.instances, id)
    }

    fn instance_by_id(&self, id: &str) -> Option<&ewo_render::screens::instances::Instance> {
        self.instance_index(id).and_then(|i| self.instances.get(i))
    }

    /// Whether the instance with folder id `id` is a Native (Rewo) one.
    fn is_native_instance(&self, id: &str) -> bool {
        self.instance_by_id(id).is_some_and(|i| {
            matches!(
                i.loader,
                ewo_render::screens::instances::InstanceLoader::Native
            )
        })
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
        let Some(inst) = self.instance_by_id(&pending.instance_id).cloned() else {
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
        // A per-instance pin (the runtime's identity) selects that runtime,
        // but only while it still exists AND still satisfies the manifest's
        // major requirement; a stale pin (JRE uninstalled) falls back to
        // auto with a warning rather than launching something wrong.
        // `detect_jres` is the warmed cache.
        let pinned = launch::jre::resolve_pin(
            &launch::detect_jres(),
            inst.java_pin.as_ref(),
            required_major,
        );
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
                    instance_id: pending.instance_id.clone(),
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
                    launch::reaper::GameState::Running { visible_window } => {
                        // A window seen right now counts as observed too.
                        self.active_launch_window_seen |= visible_window;
                        SlotInput::Running {
                            zombie: launch::reaper::is_zombie(
                                visible_window,
                                self.active_launch_window_seen,
                                self.active_launch_marker_seen,
                                time - self.active_launch_started_at,
                            ),
                        }
                    }
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

    /// Observe the tracked game for a visible window (about once a second,
    /// from `about_to_wait`, whether or not any frame is being drawn): the
    /// real "its window was seen" signal for the zombie decision, which the
    /// log markers only hint at. Covers a Native (Rewo) game too: it prints
    /// no markers, but its window belongs to the tracked PID like a JVM's.
    pub(crate) fn poll_game_window(&mut self) {
        let now = Instant::now();
        let tracked = self.active_launch;
        if !launch_slot::window_poll_due(
            tracked.is_some(),
            self.active_launch_window_seen,
            self.active_launch_polled_at,
            now,
        ) {
            return;
        }
        self.active_launch_polled_at = Some(now);
        if let Some(t) = tracked {
            if matches!(
                launch::reaper::state(&t),
                launch::reaper::GameState::Running {
                    visible_window: true
                }
            ) {
                self.active_launch_window_seen = true;
            }
        }
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

    #[test]
    fn a_waiting_launch_follows_its_instance_when_the_list_shifts() {
        let mk = |name: &str| {
            let mut i = Instance::new(
                name.into(),
                "FABRIC · 1.21".into(),
                "never".into(),
                Vec::new(),
            );
            i.id = format!("id-{name}");
            i
        };
        let mut list = vec![mk("a"), mk("b"), mk("c")];
        let launching = list[2].id.clone();
        assert_eq!(index_of_instance(&list, &launching), Some(2));
        // An earlier instance is deleted mid-prepare: the id still finds
        // the same instance (a stored index 2 would now be out of range,
        // and index 1 would have been another instance).
        let removed = list.remove(0);
        assert_eq!(index_of_instance(&list, &launching), Some(1));
        assert_eq!(list[1].name, "c");
        // The launching instance itself gone: nothing resolves.
        assert_eq!(index_of_instance(&list, &removed.id), None);
        list.retain(|i| i.id != launching);
        assert_eq!(index_of_instance(&list, &launching), None);
    }

    use auth::LaunchAuth::{NeedsRefresh, Offline, Ready, Unavailable};

    #[test]
    fn a_session_that_cannot_be_had_blocks_java_but_not_rewo() {
        let gone = Unavailable("your Microsoft session has expired");
        // Java keeps the strict behaviour: refresh first, then fail visibly.
        assert_eq!(auth_gate(&Ready, false, false), AuthGate::Proceed);
        assert_eq!(auth_gate(&Offline, false, false), AuthGate::Proceed);
        assert_eq!(
            auth_gate(&NeedsRefresh, false, false),
            AuthGate::RefreshFirst
        );
        assert_eq!(
            auth_gate(&NeedsRefresh, false, true),
            AuthGate::Fail(REFRESH_FAILED_MSG)
        );
        assert_eq!(
            auth_gate(&gone, false, false),
            AuthGate::Fail("your Microsoft session has expired")
        );
        // Rewo still tries the refresh first...
        assert_eq!(
            auth_gate(&NeedsRefresh, true, false),
            AuthGate::RefreshFirst
        );
        // ...and launches unsigned, with a warning, when it can't be had.
        assert_eq!(
            auth_gate(&NeedsRefresh, true, true),
            AuthGate::ProceedUnsigned
        );
        assert_eq!(auth_gate(&gone, true, false), AuthGate::ProceedUnsigned);
        assert_eq!(auth_gate(&Ready, true, false), AuthGate::Proceed);
        assert_eq!(auth_gate(&Offline, true, false), AuthGate::Proceed);
        assert!(NATIVE_UNSIGNED_WARNING.contains("online servers will reject you"));
    }

    #[test]
    fn an_unsigned_rewo_launch_never_hands_over_a_token() {
        let account = auth::MinecraftAccount {
            name: "Steve".into(),
            uuid: "uuid-1".into(),
            minecraft_token: "stale-token".into(),
            minecraft_token_expires_at: None,
            ms_refresh_token: String::new(),
            social_token: None,
        };
        let keys = |v: Vec<(String, String)>| v.into_iter().map(|(k, _)| k).collect::<Vec<_>>();
        assert_eq!(
            keys(native_account_envs(Some(&account), false)),
            ["REWO_USERNAME", "REWO_UUID", "REWO_ACCESS_TOKEN"]
        );
        assert_eq!(
            keys(native_account_envs(Some(&account), true)),
            ["REWO_USERNAME"],
            "a stale token is not sent"
        );
        let no_token = auth::MinecraftAccount {
            minecraft_token: String::new(),
            ..account
        };
        assert!(native_account_envs(Some(&no_token), false).is_empty());
        assert!(native_account_envs(None, false).is_empty());
    }
}
