use super::*;

/// Velvet-tinted linear colors for the capsule set.
pub(super) fn linear_rgb(r: u8, g: u8, b: u8) -> [f32; 3] {
    [
        srgb_to_linear(r as f32 / 255.0),
        srgb_to_linear(g as f32 / 255.0),
        srgb_to_linear(b as f32 / 255.0),
    ]
}

/// Camera basis vectors for nametag billboards, from MC-convention angles.
pub(crate) fn camera_basis(yaw_deg: f32, pitch_deg: f32) -> ([f32; 3], [f32; 3]) {
    let yaw = yaw_deg.to_radians();
    let pitch = pitch_deg.to_radians();
    let dir = Vec3::new(
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    );
    let right = dir.cross(Vec3::Y).normalize_or_zero();
    let up = right.cross(dir).normalize_or_zero();
    (right.to_array(), up.to_array())
}

/// Which of a profile's textures a fetch job is for (M60).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum TexKind {
    Skin,
    Cape,
}

/// One resolved player's textures: the atlas UV offset relocating the
/// default player quads onto the uploaded skin slot, the arm model, and the
/// cape slot's atlas origin.
///
/// The two arrive independently — a profile may carry a cape and no skin —
/// so an entry exists as soon as *either* lands, with the other's field at
/// its no-texture default.
#[derive(Clone, Copy, Default)]
pub(crate) struct PlayerSkin {
    pub(super) uv: [f32; 2],
    pub(super) slim: bool,
    /// Atlas origin of this player's cape slot, if one was uploaded.
    pub(super) cape: Option<(u32, u32)>,
    /// This player's tab-list face slot, if one was uploaded (M155).
    pub(super) face: Option<u8>,
}

pub(crate) type SkinRegistry = std::collections::HashMap<u128, PlayerSkin>;

/// The local player's own textures, staged for the inventory preview's
/// **second** entity pass (M36 skin, M64 cape).
///
/// The raw pixels are kept alongside the addresses because the preview pass
/// is built lazily — the first time the screen opens — so a texture can
/// arrive before there is anywhere to put it. Each address is filled in on
/// the first frame the pass exists and never recomputed.
#[derive(Default)]
pub(crate) struct PreviewTextures {
    /// 64x64 RGBA skin, and whether the profile names the slim model.
    pub skin: Option<(Vec<u8>, bool)>,
    /// 64x32 RGBA cape sheet.
    pub cape: Option<Vec<u8>>,
    /// `EntityDraw::skin_uv` once uploaded into the preview's atlas.
    pub skin_uv: Option<[f32; 2]>,
    /// `CapeDraw::origin` once uploaded into the preview's atlas.
    pub cape_origin: Option<(u32, u32)>,
}

/// How many decoded textures the CPU cache keeps. A skin is 16 KiB and a cape
/// 8 KiB, so the cap is about 2 MiB.
pub(super) const TEX_CACHE_CAP: usize = 128;
/// The soonest a texture whose atlas slot was lost may be uploaded again from
/// the CPU cache. With more players loaded than the atlas has slots, the
/// least-recently-drawn ones are evicted for the ones on screen; without this
/// they would swap back in every frame, evicting each other.
pub(super) const REUPLOAD_COOLDOWN_S: f32 = 5.0;
/// The soonest a texture that fell out of the CPU cache may be DOWNLOADED
/// again. A network round trip, so a much longer leash.
pub(super) const REFETCH_COOLDOWN_S: f32 = 60.0;
/// Atlas re-uploads per frame (each is a region copy).
pub(super) const MAX_REUPLOADS_PER_FRAME: usize = 2;
/// Seconds between sweeps that forget players who left.
pub(super) const PRUNE_INTERVAL_S: f32 = 5.0;

/// Decoded player textures kept on the CPU so a lost atlas slot (evicted for
/// a player on screen, or a claim the atlas refused) is restored by an upload
/// rather than a second download. Bounded: the oldest-used entry goes first.
#[derive(Default)]
pub(super) struct TexCache {
    map: std::collections::HashMap<(u128, TexKind), (Vec<u8>, bool, u64)>,
    seq: u64,
}

impl TexCache {
    pub(super) fn insert(&mut self, key: (u128, TexKind), rgba: Vec<u8>, slim: bool) {
        self.seq += 1;
        if !self.map.contains_key(&key) && self.map.len() >= TEX_CACHE_CAP {
            if let Some(oldest) = self
                .map
                .iter()
                .min_by_key(|(_, (_, _, used))| *used)
                .map(|(k, _)| *k)
            {
                self.map.remove(&oldest);
            }
        }
        self.map.insert(key, (rgba, slim, self.seq));
    }

    /// The pixels and model, marking the entry freshly used.
    pub(super) fn get(&mut self, key: &(u128, TexKind)) -> Option<(Vec<u8>, bool)> {
        self.seq += 1;
        let seq = self.seq;
        let (rgba, slim, used) = self.map.get_mut(key)?;
        *used = seq;
        Some((rgba.clone(), *slim))
    }

    pub(super) fn contains(&self, key: &(u128, TexKind)) -> bool {
        self.map.contains_key(key)
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.map.len()
    }

    fn retain(&mut self, mut keep: impl FnMut(u128) -> bool) {
        self.map.retain(|(u, _), _| keep(*u));
    }
}

/// What to do about a texture that has no atlas slot but is needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Restore {
    /// Upload the cached pixels again (no network).
    Reupload,
    /// The pixels are gone: download again.
    Refetch,
    /// Tried too recently.
    Wait,
}

/// The restore decision as a pure function of "do we hold the pixels", when
/// this texture was last attempted (seconds on the loader's clock) and now.
pub(super) fn plan_restore(cached: bool, last_attempt: Option<f32>, now: f32) -> Restore {
    let cooldown = if cached {
        REUPLOAD_COOLDOWN_S
    } else {
        REFETCH_COOLDOWN_S
    };
    if last_attempt.is_some_and(|t| now - t < cooldown) {
        return Restore::Wait;
    }
    if cached {
        Restore::Reupload
    } else {
        Restore::Refetch
    }
}

/// Async player-texture loader: a worker thread fetches + decodes skin and
/// cape PNGs off the render/tick path; the main loop uploads each result
/// into the entity atlas and records its slot. They arrive rarely (once per
/// player at join); each upload is a region copy with no queue wait.
pub(crate) struct SkinLoader {
    pub(super) req_tx: std::sync::mpsc::Sender<(u128, TexKind, String, bool)>,
    pub(super) res_rx: std::sync::mpsc::Receiver<(u128, TexKind, bool, Vec<u8>)>,
    pub(super) requested: std::collections::HashSet<(u128, TexKind)>,
    /// Each requested texture's URL and model, kept so a texture whose atlas
    /// slot was taken by another player can be fetched again.
    urls: std::collections::HashMap<(u128, TexKind), (String, bool)>,
    /// Textures whose slot was evicted (least recently drawn) and not yet
    /// re-requested. A tab face counts as `Skin`: it is cropped from the skin.
    /// Also holds textures whose atlas claim was REFUSED (every slot drawn
    /// this frame) - same remedy.
    evicted: std::collections::HashSet<(u128, TexKind)>,
    /// Decoded pixels of everything fetched, bounded; see [`TexCache`].
    cache: TexCache,
    /// When each texture was last uploaded-again or re-requested (seconds on
    /// [`Self::now`]), for the cooldowns.
    attempts: std::collections::HashMap<(u128, TexKind), f32>,
    /// The loader's clock.
    born: std::time::Instant,
    /// [`Self::now`] of the last [`Self::prune_due`] that fired.
    last_prune: f32,
    pub(super) registry: SkinRegistry,
    /// The local player's UUID, set by the frame. Its textures are also kept
    /// as pixels, because the inventory's player preview has an atlas of its
    /// own and must upload them itself.
    pub(super) own: Option<u128>,
    /// The local player's skin/cape pixels, handed out by [`Self::take_own_preview`]
    /// whenever one arrives.
    own_preview: PreviewTextures,
    own_changed: bool,
}

/// M308 — the pseudo-URL of a default skin, `rewo-default:<index into
/// DEFAULT_SKIN_PATHS>`: answered by the worker from the jar's sheets, never
/// fetched.
const DEFAULT_SKIN_SCHEME: &str = "rewo-default:";

/// M309 — a skin patch's body texture that names one of the default skins
/// (`minecraft:entity/player/wide/steve`) picks it.
pub(crate) fn default_skin_by_texture(id: &str) -> Option<usize> {
    let path = id.strip_prefix("minecraft:").unwrap_or(id);
    rewo_data::assets::DEFAULT_SKIN_PATHS
        .iter()
        .position(|p| p.strip_suffix(".png") == Some(path))
}

/// M309 — what a mannequin's profile asks of the skin loader:
/// `ClientMannequin.updateSkin`'s lookup, minus every network resolution.
/// A `textures` property is a real skin (fetched only from the allowed host,
/// as a player's is); otherwise the default skin of the profile's id, which
/// for a profile known only by name is its offline UUID, and for the empty
/// profile the nil UUID; a patch's body texture naming a default skin, and a
/// patch's model, override.
pub(crate) enum AvatarSkin {
    Real(rewo_net::skins::SkinInfo),
    Default { index: usize, slim: bool },
}

pub(crate) fn avatar_skin(p: Option<&rewo_world::entities::AvatarProfile>) -> AvatarSkin {
    if let Some(mut info) = p
        .and_then(|p| p.textures.as_deref())
        .and_then(rewo_net::skins::decode_textures_property)
        .filter(|i| i.url.is_some())
    {
        if let Some(s) = p.and_then(|p| p.model_slim) {
            info.slim = s;
        }
        return AvatarSkin::Real(info);
    }
    let id = p
        .and_then(|p| {
            p.id.or_else(|| p.name.as_deref().map(rewo_net::skins::offline_player_uuid))
        })
        .unwrap_or(0);
    let index = p
        .and_then(|p| p.body_texture.as_deref())
        .and_then(default_skin_by_texture)
        .unwrap_or_else(|| default_skin_index(id));
    let slim = p.and_then(|p| p.model_slim).unwrap_or(index < 9);
    AvatarSkin::Default { index, slim }
}

/// M308 — `DefaultPlayerSkin.get(profileId)`'s index:
/// `floorMod(UUID.hashCode(), 18)`, `hashCode` being the two halves XORed and
/// folded to an `int`.
pub(crate) fn default_skin_index(profile_id: u128) -> usize {
    let hilo = ((profile_id >> 64) as u64) ^ (profile_id as u64);
    let hash = ((hilo >> 32) as u32 ^ hilo as u32) as i32;
    hash.rem_euclid(rewo_data::assets::DEFAULT_SKIN_PATHS.len() as i32) as usize
}

impl SkinLoader {
    /// No default skins: the unit tests' loader.
    #[cfg(test)]
    pub(super) fn new() -> Self {
        Self::with_defaults(Vec::new())
    }

    /// M308 — with the jar's default skins (64 x 64 RGBA, in
    /// `DEFAULT_SKIN_PATHS` order), which [`Self::request_default`] serves.
    pub(super) fn with_defaults(defaults: Vec<Option<Vec<u8>>>) -> Self {
        let (req_tx, req_rx) = std::sync::mpsc::channel::<(u128, TexKind, String, bool)>();
        let (res_tx, res_rx) = std::sync::mpsc::channel::<(u128, TexKind, bool, Vec<u8>)>();
        std::thread::Builder::new()
            .name("rewo-skin-fetch".into())
            .spawn(move || {
                while let Ok((uuid, kind, url, slim)) = req_rx.recv() {
                    let got = match (kind, url.strip_prefix(DEFAULT_SKIN_SCHEME)) {
                        // M308 — a default skin, from memory: nothing phones
                        // home for it.
                        (TexKind::Skin, Some(i)) => i
                            .parse::<usize>()
                            .ok()
                            .and_then(|i| defaults.get(i).cloned().flatten())
                            .filter(|rgba| rgba.len() == 64 * 64 * 4)
                            .ok_or_else(|| format!("no default skin {i}")),
                        (TexKind::Skin, None) => crate::skin_fetch::fetch_rgba64(&url),
                        (TexKind::Cape, _) => crate::skin_fetch::fetch_cape_rgba(&url),
                    };
                    match got {
                        Ok(rgba) => {
                            if res_tx.send((uuid, kind, slim, rgba)).is_err() {
                                return;
                            }
                        }
                        Err(e) => log::warn!("skin: fetch {url} failed: {e}"),
                    }
                }
            })
            .ok();
        Self {
            req_tx,
            res_rx,
            requested: std::collections::HashSet::new(),
            urls: std::collections::HashMap::new(),
            evicted: std::collections::HashSet::new(),
            cache: TexCache::default(),
            attempts: std::collections::HashMap::new(),
            born: std::time::Instant::now(),
            last_prune: 0.0,
            registry: SkinRegistry::new(),
            own: None,
            own_preview: PreviewTextures::default(),
            own_changed: false,
        }
    }

    /// Queue this profile's texture fetches (once per UUID per kind).
    pub(super) fn request(&mut self, uuid: u128, info: &rewo_net::skins::SkinInfo) {
        for (kind, url) in [
            (TexKind::Skin, info.url.as_ref()),
            (TexKind::Cape, info.cape.as_ref()),
        ] {
            let Some(url) = url else { continue };
            // M308 — a real skin replaces a default one queued before the
            // profile's textures arrived.
            let over_default = self
                .urls
                .get(&(uuid, kind))
                .is_some_and(|(u, _)| u.starts_with(DEFAULT_SKIN_SCHEME));
            // Bounded like the session's own map (`MAX_TRACKED_SKINS`).
            if self.requested.len() < 2 * rewo_net::skins::MAX_TRACKED_SKINS
                && (self.requested.insert((uuid, kind)) || over_default)
            {
                self.urls.insert((uuid, kind), (url.clone(), info.slim));
                let _ = self.req_tx.send((uuid, kind, url.clone(), info.slim));
            }
        }
    }

    /// M308 — `DefaultPlayerSkin.get(profileId)`: a profile with no textures
    /// (every player on an offline server, a mannequin's empty profile)
    /// wears one of the jar's eighteen default skins, picked by the profile
    /// id's hash. Queued under `key` (the entity's UUID) exactly as a fetched
    /// skin is, so eviction, re-upload and the inventory preview work
    /// unchanged; a key with any request of its own is left alone.
    pub(super) fn request_default(&mut self, key: u128, profile_id: u128) {
        let i = default_skin_index(profile_id);
        // The first nine are the slim models.
        self.request_default_skin(key, i, i < 9);
    }

    /// M309 — default skin `index` on the `slim` (or wide) model for `key`.
    /// A real skin's request is left alone; a different default (a
    /// mannequin's profile changed) replaces the one queued.
    pub(super) fn request_default_skin(&mut self, key: u128, index: usize, slim: bool) {
        let url = format!("{DEFAULT_SKIN_SCHEME}{index}");
        match self.urls.get(&(key, TexKind::Skin)) {
            Some((u, s)) if !u.starts_with(DEFAULT_SKIN_SCHEME) || (*u == url && *s == slim) => {
                return;
            }
            Some(_) => {}
            None if self.requested.contains(&(key, TexKind::Skin))
                || self.requested.len() >= 2 * rewo_net::skins::MAX_TRACKED_SKINS =>
            {
                return;
            }
            None => {}
        }
        self.requested.insert((key, TexKind::Skin));
        self.urls.insert((key, TexKind::Skin), (url.clone(), slim));
        let _ = self.req_tx.send((key, TexKind::Skin, url, slim));
    }

    /// Seconds on the loader's own clock.
    fn now(&self) -> f32 {
        self.born.elapsed().as_secs_f32()
    }

    /// Upload any fetched textures into the atlas + record their slots.
    ///
    /// Every fetched texture is also kept in the bounded CPU [`TexCache`], and
    /// one the atlas refuses (`claim` returns `None` when every slot was drawn
    /// this frame) is queued in `evicted` instead of being lost: the pixels
    /// are still here, so [`Self::restore_evicted`] can try again later with
    /// no second download.
    pub(super) fn poll_uploads(&mut self, gpu: &mut Gpu, wr: &mut WorldRenderer) {
        while let Ok((uuid, kind, slim, rgba)) = self.res_rx.try_recv() {
            if Some(uuid) == self.own {
                match kind {
                    TexKind::Skin => self.own_preview.skin = Some((rgba.clone(), slim)),
                    TexKind::Cape => self.own_preview.cape = Some(rgba.clone()),
                }
                self.own_changed = true;
            }
            let key = (uuid, kind);
            if !self.upload_tex(gpu, wr, key, slim, &rgba) {
                log::debug!("skin: atlas refused {uuid:032x} ({kind:?}); will retry from cache");
                self.evicted.insert(key);
                let now = self.now();
                self.attempts.insert(key, now);
            }
            self.cache.insert(key, rgba, slim);
        }
    }

    /// Put one decoded texture into the atlas and record its address.
    /// `false` = the atlas refused (nothing evictable this frame).
    fn upload_tex(
        &mut self,
        gpu: &mut Gpu,
        wr: &mut WorldRenderer,
        (uuid, kind): (u128, TexKind),
        slim: bool,
        rgba: &[u8],
    ) -> bool {
        match kind {
            TexKind::Skin => {
                // M155 - crop the tab-list face while the full sheet is in
                // hand.
                //
                // 8x8 head at (8,8) beside 8x8 hat at (40,8). **The hat's u
                // is 40, not 32** - 32 is the hat cube's side face.
                let mut face_ok = true;
                if rgba.len() >= 64 * 64 * 4 {
                    let mut strip = vec![0u8; 16 * 8 * 4];
                    for row in 0..8usize {
                        for (col_off, src_x) in [(0usize, 8usize), (8, 40)] {
                            let src = ((8 + row) * 64 + src_x) * 4;
                            let dst = (row * 16 + col_off) * 4;
                            strip[dst..dst + 32].copy_from_slice(&rgba[src..src + 32]);
                        }
                    }
                    match wr.upload_tab_face(gpu, uuid, &strip) {
                        Some(slot) => self.registry.entry(uuid).or_default().face = Some(slot),
                        None => face_ok = false,
                    }
                }
                match wr.upload_player_skin(gpu, uuid, rgba) {
                    Some(uv) => {
                        let e = self.registry.entry(uuid).or_default();
                        e.uv = uv;
                        e.slim = slim;
                        log::debug!(
                            "skin: uploaded for {uuid:032x} ({} model)",
                            if slim { "slim" } else { "wide" }
                        );
                        face_ok
                    }
                    None => false,
                }
            }
            TexKind::Cape => match wr.upload_player_cape(gpu, uuid, rgba) {
                Some(o) => {
                    self.registry.entry(uuid).or_default().cape = Some(o);
                    log::debug!("cape: uploaded for {uuid:032x} at {o:?}");
                    true
                }
                None => false,
            },
        }
    }

    /// Forget the atlas addresses of players whose slot another upload took
    /// (the renderer evicts the least recently drawn, never one on screen this
    /// frame). Their textures render as the default until restored by
    /// [`Self::restore_evicted`].
    pub(super) fn drop_evicted(&mut self, wr: &mut WorldRenderer) {
        use rewo_gpu::world::PlayerTexture;
        for (uuid, tex) in wr.take_evicted_textures() {
            let Some(e) = self.registry.get_mut(&uuid) else {
                continue;
            };
            let kind = match tex {
                PlayerTexture::Skin => {
                    e.uv = [0.0, 0.0];
                    TexKind::Skin
                }
                PlayerTexture::Cape => {
                    e.cape = None;
                    TexKind::Cape
                }
                PlayerTexture::Face => {
                    e.face = None;
                    TexKind::Skin
                }
            };
            log::debug!("skin: atlas slot of {uuid:032x} reused ({tex:?})");
            self.evicted.insert((uuid, kind));
        }
    }

    /// Restore each texture that lost its atlas slot and whose player
    /// `wanted` says is needed now.
    ///
    /// The pixels are in the CPU cache in the normal case, so this is an
    /// upload (at most [`MAX_REUPLOADS_PER_FRAME`] per frame, each texture at
    /// most once per [`REUPLOAD_COOLDOWN_S`]) and never a download. Only a
    /// texture that fell out of the cache is fetched again, and at most once
    /// per [`REFETCH_COOLDOWN_S`]. Without the cooldowns, more loaded players
    /// than atlas slots made eviction -> restore -> eviction cycles at frame
    /// (once at network) speed.
    pub(super) fn restore_evicted(
        &mut self,
        gpu: &mut Gpu,
        wr: &mut WorldRenderer,
        wanted: impl Fn(u128) -> bool,
    ) {
        let now = self.now();
        let mut due: Vec<(u128, TexKind)> = self
            .evicted
            .iter()
            .copied()
            .filter(|(u, _)| wanted(*u))
            .collect();
        // Deterministic order, so the per-frame cap doesn't starve by hash.
        due.sort_by_key(|(u, k)| (*u, *k == TexKind::Cape));
        let mut reuploads = 0;
        for key in due {
            let cached = self.cache.contains(&key);
            match plan_restore(cached, self.attempts.get(&key).copied(), now) {
                Restore::Wait => {}
                Restore::Reupload => {
                    if reuploads >= MAX_REUPLOADS_PER_FRAME {
                        continue;
                    }
                    reuploads += 1;
                    self.attempts.insert(key, now);
                    if let Some((rgba, slim)) = self.cache.get(&key) {
                        if self.upload_tex(gpu, wr, key, slim, &rgba) {
                            self.evicted.remove(&key);
                        }
                    }
                }
                Restore::Refetch => {
                    self.attempts.insert(key, now);
                    self.evicted.remove(&key);
                    if let Some((url, slim)) = self.urls.get(&key) {
                        let _ = self.req_tx.send((key.0, key.1, url.clone(), *slim));
                    }
                }
            }
        }
    }

    /// Whether it is time for another [`Self::retain_players`] sweep.
    pub(super) fn prune_due(&mut self) -> bool {
        let now = self.now();
        if now - self.last_prune < PRUNE_INTERVAL_S {
            return false;
        }
        self.last_prune = now;
        true
    }

    /// Forget every player `keep` rejects: the requested/URL/evicted/cache/
    /// cooldown entries and the atlas addresses. Without it `requested` only
    /// ever grew, so its cap (`2 * MAX_TRACKED_SKINS`) became a lifetime cap
    /// that locked later joiners out after enough player churn. A departed
    /// player's atlas slot is not freed here (the renderer's LRU recycles it);
    /// it just stops being remembered, and a returning player is announced
    /// again by the session and fetched fresh.
    ///
    /// The local player is always kept.
    pub(super) fn retain_players(&mut self, keep: impl Fn(u128) -> bool) {
        let own = self.own;
        let keep = |u: u128| Some(u) == own || keep(u);
        self.requested.retain(|(u, _)| keep(*u));
        self.urls.retain(|(u, _), _| keep(*u));
        self.evicted.retain(|(u, _)| keep(*u));
        self.attempts.retain(|(u, _), _| keep(*u));
        self.cache.retain(keep);
        self.registry.retain(|u, _| keep(*u));
    }

    /// The local player's textures for the inventory preview, once per change
    /// (a skin and a cape arrive separately). Handed out un-uploaded: the
    /// preview uploads them into its own atlas the next time it draws.
    pub(super) fn take_own_preview(&mut self) -> Option<PreviewTextures> {
        if !std::mem::take(&mut self.own_changed) {
            return None;
        }
        Some(PreviewTextures {
            skin: self.own_preview.skin.clone(),
            cape: self.own_preview.cape.clone(),
            skin_uv: None,
            cape_origin: None,
        })
    }
}

/// Tracks when each entity's gesture-driving state changed. The wire
/// carries only the *current* pose/state — vanilla clients time the rigs
/// from the transition instant, so we record it here. Both a wall-clock
/// second (the rig's time base) and the network tick of the transition are
/// kept: the tick is what the entity-event ownership rules compare against
/// (vanilla orders `AnimationState.start`/`.stop` by tick — see
/// [`resolve_mob_anim`]).
#[derive(Clone, Copy)]
pub(super) struct GestureEntry {
    pub(super) gesture: rewo_gpu::mobs::Gesture,
    pub(super) start_seconds: f32,
    pub(super) start_tick: i64,
}

#[derive(Default)]
pub(crate) struct GestureTracker {
    pub(super) map: std::collections::HashMap<i32, GestureEntry>,
}

impl GestureTracker {
    /// Record `wanted` for this entity at `now` seconds / `tick`; returns the
    /// active gesture, its age in seconds, and the network tick it started on.
    /// `head_start` pre-advances a *newly entered* gesture's clock (vanilla's
    /// SCARED `fastForward`). A repeated *same* gesture keeps its original
    /// transition tick — so metadata that re-arrives without an actual pose
    /// change is not a new transition.
    pub(super) fn update(
        &mut self,
        id: i32,
        wanted: Option<rewo_gpu::mobs::Gesture>,
        now: f32,
        head_start: f32,
        tick: i64,
    ) -> Option<(rewo_gpu::mobs::Gesture, f32, i64)> {
        match wanted {
            None => {
                self.map.remove(&id);
                None
            }
            Some(g) => {
                let e = match self.map.get(&id) {
                    Some(e) if e.gesture == g => *e,
                    _ => {
                        let e = GestureEntry {
                            gesture: g,
                            start_seconds: now - head_start,
                            start_tick: tick,
                        };
                        self.map.insert(id, e);
                        e
                    }
                };
                Some((g, now - e.start_seconds, e.start_tick))
            }
        }
    }
}

/// Elapsed seconds since a one-shot event's receipt tick, using vanilla's
/// `ageInTicks = tickCount + partialTick` convention: `(now_tick − start +
/// partial) · 0.05`. `None` passes through (the event never fired). Clamped
/// non-negative for the degenerate same-tick-receipt case.
pub(super) fn event_age_seconds(start_tick: Option<i64>, tick: i64, alpha: f32) -> Option<f32> {
    start_tick.map(|s| ((((tick - s) as f32) + alpha) * 0.05).max(0.0))
}

/// Resolve a mob's per-frame rig inputs from its wire pose/state plus the
/// one-shot entity-event side-table, applying the exact vanilla ownership
/// rules. Shared by the live collector and the `eventshot` oracle so those
/// rules are exercised through production code, not a copy.
///
/// - **Warden id 4 (attack)** calls `roarAnimationState.stop()` and never
///   restarts the roar until a fresh ROARING pose transition. So the metadata
///   roar is suppressed iff the attack event's receipt tick is at/after the
///   roar's transition tick — vanilla's start/stop tick ordering. The attack
///   rig itself still plays (through the returned `events`).
/// - **Armadillo id 64 (peek)** stops and re-`startIfStopped`s the SCARED
///   peek — the SAME shared `peekAnimationState` as the metadata 2.5 s hold —
///   so it re-clocks the existing SCARED gesture from age 0 rather than adding
///   a second rig. Only when the event landed during this SCARED episode
///   (receipt tick at/after the SCARED transition), else a stale event from a
///   previous roll can't disturb the current hold.
///
/// Returns the gesture (post-ownership-rules) and the per-`ModelEvent` ages.
pub(crate) fn resolve_mob_anim(
    kind: EntityModelKind,
    pose: u8,
    state: u8,
    attack_tick: Option<i64>,
    sonic_tick: Option<i64>,
    peek_tick: Option<i64>,
    gestures: &mut GestureTracker,
    id: i32,
    now: f32,
    tick: i64,
    alpha: f32,
) -> (
    Option<(rewo_gpu::mobs::Gesture, f32)>,
    [Option<f32>; rewo_gpu::mobs::ModelEvent::COUNT],
) {
    use rewo_gpu::mobs::Gesture;
    let events = [
        event_age_seconds(attack_tick, tick, alpha),
        event_age_seconds(sonic_tick, tick, alpha),
    ];
    let wanted = wanted_gesture(kind, pose, state);
    // Entering SCARED starts the peek at its held ball pose — vanilla
    // `fastForward(SCARED.animationDuration())` = 2.5 s.
    let head_start = if wanted == Some(Gesture::ArmadilloScared) {
        2.5
    } else {
        0.0
    };
    let resolved = gestures.update(id, wanted, now, head_start, tick);
    let gesture = resolved.and_then(|(g, age, start_tick)| match g {
        // Attack stopped the roar durably (until a fresh ROARING transition).
        Gesture::WardenRoar if attack_tick.is_some_and(|a| a >= start_tick) => None,
        // Peek re-clocks the SCARED hold from age 0.
        Gesture::ArmadilloScared if peek_tick.is_some_and(|p| p >= start_tick) => {
            Some((g, event_age_seconds(peek_tick, tick, alpha).unwrap_or(0.0)))
        }
        _ => Some((g, age)),
    });
    (gesture, events)
}

/// Wire state → gesture for the rigged kinds. Pose ordinals are
/// `Pose.java`'s ids; state ordinals are the `Sniffer.State` /
/// `ArmadilloState` enum orders (metadata index 17).
pub(super) fn wanted_gesture(
    kind: EntityModelKind,
    pose: u8,
    state: u8,
) -> Option<rewo_gpu::mobs::Gesture> {
    use rewo_gpu::mobs::Gesture::*;
    Some(match kind {
        EntityModelKind::Warden => match pose {
            11 => WardenRoar,
            12 => WardenSniff,
            13 => WardenEmerge,
            14 => WardenDig,
            _ => return None,
        },
        EntityModelKind::Frog => match pose {
            8 => FrogCroak,
            9 => FrogTongue,
            _ => return None,
        },
        EntityModelKind::Breeze => match pose {
            6 => BreezeJump,
            15 => BreezeSlide,
            16 => BreezeShoot,
            17 => BreezeInhale,
            _ => return None,
        },
        EntityModelKind::Sniffer => match state {
            1 => SnifferFeelingHappy,
            2 => SnifferScenting,
            3 => SnifferSniffing,
            4 => SnifferSearching,
            5 => SnifferDigging,
            6 => SnifferRising,
            _ => return None,
        },
        EntityModelKind::Armadillo => match state {
            1 => ArmadilloRoll,
            2 => ArmadilloScared,
            3 => ArmadilloUnroll,
            _ => return None,
        },
        _ => return None,
    })
}

/// Resolve whether one entity shows a floating health bar, and with what
/// numbers — the production seam shared by the live collector and the
/// `healthbarshot` oracle (M59).
///
/// `REWO_HEALTH_BAR_SPEC.md` owns the bar's *appearance*; this owns the two
/// questions below the line where a vanilla oracle exists — may a floating
/// label appear at all, and is the denominator real?
///
/// The order is deliberate, and every step returns `None` rather than a
/// number:
///
/// 1. **Living only.** [`rewo_world::attributes::resolve`] answers `None` when
///    the entity type is unknown, is absent from `DefaultAttributes.SUPPLIERS`
///    (it is not a `LivingEntity`), or has no such attribute. A boat has no
///    max health, so a boat gets no bar — with no `matches!` list to keep in
///    sync.
/// 2. **Name-tag distance.** `EntityRenderer.extractNameTags` gates the whole
///    label on `distanceToCameraSq < Mth.square(nameTagDistance)`, and in 26.x
///    `nameTagDistance` is itself an attribute —
///    `entity.getAttribute(Attributes.NAME_TAG_DISTANCE).getValue()`, a
///    `RangedAttribute` defaulting to **64.0** over `[0, 512]`. So it resolves
///    through exactly the machinery above, modifiers and clamp included.
/// 3. **Invisible.** `LivingEntityRenderer.shouldShowName` ends in
///    `... && isVisibleToPlayer && ...`, where `isVisibleToPlayer` is
///    `!entity.isInvisibleTo(player)` and, with no teams and a non-spectating
///    viewer, that is `!entity.isInvisible()` — shared-flag **5**.
/// 4. **A synced max.** Spec rule 4: only a max health an `update_attributes`
///    actually established draws a bar. [`rewo_world::attributes::Source`]
///    exists precisely so this can be asked, and **there is no fallback to
///    20.0**: the supplier's default is a real number for every living entity,
///    which is what makes a wrong denominator so easy to draw confidently.
///    Rewo cannot tell "the server never sent health" from "this mob has 1 HP"
///    either — `DATA_HEALTH_ID` is seeded at `1.0F` — so a bar with an
///    unverified denominator would be a confident lie in both directions.
///
/// Deliberately **not** gated on (each a documented gap, not an oversight):
/// scoreboard team name-tag visibility and `canSeeFriendlyInvisibles` (Rewo
/// decodes no teams), `isDiscrete()`'s 32-block sneak cut-off, `isVehicle()`,
/// and `hud.isHidden()`. The local player never reaches here — it is not in
/// the entity table — which is also the spec's exclusion.
/// `CapeLayer.submit`'s four gates and `AvatarRenderer.extractCapeState`'s
/// three angles, resolved for one entity on one frame (M60).
///
/// Shared by the renderer and `capeshot` so the oracle cannot grade a second
/// copy of these rules — M41 and M45 both shipped gates that had quietly
/// stopped testing their subject exactly that way.
///
/// The gates, in vanilla's order:
///
/// 1. `!state.isInvisible && state.showCape` — shared flag 5, and bit 0 of
///    the index-16 customisation mask.
/// 2. `skin.cape() != null` — here, whether a cape sheet was uploaded for
///    this profile.
/// 3. `!hasLayer(chestEquipment, WINGS)` — an equipped **elytra** replaces
///    the cape outright.
/// 4. not a gate but the same block: `hasLayer(chestEquipment, HUMANOID)`
///    shifts the cape clear of a chestplate.
///
/// Gates 3 and 4 are separate questions about the same slot, which is why
/// [`rewo_data::equipment::ArmorLayer::Wings`] had to exist: a **carved
/// pumpkin** occupies the chest slot in neither sense — it names no
/// equipment asset at all — so it suppresses nothing and shifts nothing,
/// and before M60 it was indistinguishable from an elytra.
pub(crate) fn resolve_cape(
    ents: &rewo_world::entities::EntityTable,
    id: i32,
    kind: EntityModelKind,
    alpha: f32,
    cape_origin: Option<(u32, u32)>,
    items: &rewo_data::items::Items,
    equipment: &rewo_data::equipment::EquipmentAssets,
) -> Option<rewo_gpu::entities::CapeDraw> {
    use rewo_data::equipment::ArmorLayer;
    // `CapeLayer` is added by `AvatarRenderer` alone — a zombie has a torso
    // and no cape layer.
    if !rewo_gpu::mobs::wears_cape(kind) {
        return None;
    }
    // Gate 1.
    if ents.is_invisible(id) || !ents.shows_cape(id) {
        return None;
    }
    // Gate 2.
    let origin = cape_origin?;
    // Gates 3 + 4 — one lookup of the chest item, two different questions.
    let chest = ents.armor(id)[1].and_then(|p| items.name(p.item));
    if chest.is_some_and(|n| equipment.has_layer(n, ArmorLayer::Wings)) {
        return None;
    }
    let chest_humanoid = chest.is_some_and(|n| equipment.has_layer(n, ArmorLayer::Humanoid));

    let e = ents.get(id)?;
    let a = rewo_world::cape::cape_angles(
        e.cloak_pos(alpha),
        e.render_pos(alpha),
        e.yaw,
        e.fall_fly_ticks() as f32 + alpha,
        // `bob` and `walkDistance` are structurally zero for every entity
        // Rewo renders: only `LocalPlayer.move` ever advances `walkDist`,
        // and the local player is not in this table. Passing literal zeros
        // rather than modelling them is exact, not an approximation — see
        // `rewo_world::cape::cape_angles`, which explains why `bob` alone
        // would have been the wrong thing to key off.
        0.0,
        0.0,
    );
    Some(rewo_gpu::entities::CapeDraw {
        origin,
        flap: a.flap,
        lean: a.lean,
        lean2: a.lean2,
        chest_humanoid,
        // M61. `None` whenever the wavy cape is switched off — the table
        // holds no chain at all then — and the renderer falls through to the
        // vanilla rigid slab. The simulation is *read* here, never advanced:
        // this runs once per frame and `interpolated` takes `&self`.
        wavy: resolve_wavy_cape(ents, id, alpha),
    })
}

/// The frame's interpolated cape spine, or `None` for the vanilla cape.
pub(super) fn resolve_wavy_cape(
    ents: &rewo_world::entities::EntityTable,
    id: i32,
    alpha: f32,
) -> Option<rewo_gpu::entities::CapeJoints> {
    let sim = ents.wavy_cape(id)?;
    let mut buf = [[0.0f32; 3]; rewo_gpu::entities::CAPE_MAX_JOINTS];
    let n = sim.interpolated(alpha, &mut buf);
    rewo_gpu::entities::CapeJoints::from_slice(&buf[..n])
}

/// Everything the label predicate reads about the **viewer**, gathered once
/// per frame rather than once per entity (M70).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LabelViewer<'a> {
    /// `minecraft.getCameraEntity()`. Fed by `set_camera` since M74, so it
    /// really does diverge from `local_player` while spectating — before that
    /// it was hard-wired to the player with a note saying Rewo never detaches
    /// the camera. Vanilla reads the two in different clauses, which is why
    /// they were kept apart even while they could not differ.
    pub camera_entity: Option<i32>,
    /// `minecraft.player`.
    pub local_player: Option<i32>,
    /// `gui.hud.isHidden()` — F1.
    pub hud_hidden: bool,
    /// `player.isSpectator()`.
    pub spectator: bool,
    /// `minecraft.player.getTeam()`.
    pub team: Option<&'a str>,
    /// `entityRenderDispatcher.crosshairPickEntity` (M73) — the id
    /// [`resolve_crosshair_pick`] returned for this frame, or `None`.
    ///
    /// One value per frame rather than a per-entity question, because vanilla
    /// resolves it exactly once: `Minecraft.pick` runs one raycast and
    /// `EntityRenderDispatcher.prepare` hands the single result to every
    /// renderer. M70 fed this a hard `false`.
    pub crosshair_pick: Option<i32>,
}

impl<'a> LabelViewer<'a> {
    /// Read the viewer half out of the session once.
    ///
    /// `crosshair_pick` is left `None` here and filled by the caller, because
    /// it needs the frame's interpolation factor and the pick tables — see
    /// [`resolve_crosshair_pick`].
    pub(crate) fn from_session(session: &'a PlaySession, hud_hidden: bool) -> LabelViewer<'a> {
        LabelViewer {
            // M74: the server's `set_camera`, falling back to the local
            // player when it has never sent one — which is exactly
            // `Minecraft.cameraEntity`, a field initialised to the player and
            // only ever reassigned.
            camera_entity: session.client_state.camera_entity_or(session.player_id),
            local_player: session.player_id,
            hud_hidden,
            spectator: session.own_game_mode().is_some_and(|g| g.is_spectator()),
            team: session.own_team(),
            crosshair_pick: None,
        }
    }
}

/// Everything [`resolve_crosshair_pick`] needs that is not the entity table —
/// the version's static tables, gathered once so the seam takes one argument
/// rather than five.
#[derive(Clone, Copy)]
pub(crate) struct PickTables<'a> {
    pub types: &'a EntityTypes,
    pub classes: &'a rewo_data::entity_types::EntityClasses,
    pub shapes: &'a rewo_data::entity_pick::EntityPickTable,
    /// `EntityTypeTags.REDIRECTABLE_PROJECTILE`, the tag
    /// `Projectile.isPickable()` reads.
    pub redirectable: &'a rewo_data::entity_pick::EntityTypeTag,
    pub attributes: &'a rewo_data::attributes::AttributeRegistry,
}

/// `Minecraft.pick` → `crosshairPickEntity` (M73) — the production seam shared
/// by the live collector and the `labelshot` oracle.
///
/// Shared for the same reason as [`label_inputs_from_table`]: the gate has to
/// grade the resolution the client actually renders through. M41 and M45 both
/// shipped gates that quietly stopped testing their subject by reimplementing
/// a slice of the app's setup, and M73's whole output is one entity id — the
/// easiest possible thing for a parallel derivation to get subtly wrong.
///
/// The four per-entity predicates, and where each comes from:
///
/// * **`isPickable()`** — the type's [`rewo_data::entity_pick::PickRule`],
///   evaluated against the one live input each rule needs. `Alive` is
///   unconditionally true here because Rewo's table *deletes* a removed
///   entity, so `!isRemoved()` holds for every row by construction.
/// * **`getPickRadius()`** — `1.0` for a pickable `Projectile`, else `0.0`.
///   The rule already carries which is which.
/// * **`canBePickedFromInside()`** — `true` for everything Rewo models; the
///   only override is `SulfurCube` carrying a body item, whose flag Rewo does
///   not decode. A wrong answer there costs an inside-pick on one mob.
/// * **`getRootVehicle() == except.getRootVehicle()`** — walked through the
///   riding graph from both ends.
///
/// Two inputs Rewo cannot evaluate and answers the *permissive* way, which is
/// the opposite of the usual house rule and is deliberate:
/// `Player.isSpectator()` for a **remote** player and `ArmorStand.isMarker()`.
/// Both are metadata Rewo does not decode; suppressing on them would make
/// every player and every armour stand unpickable, which is a far larger error
/// than the one it avoids. The local player's own spectator state *is* known
/// and is not the question — you are never your own crosshair target, because
/// `getEntities(except, …)` excludes the camera entity.
pub(crate) fn resolve_crosshair_pick(
    session: &PlaySession,
    tables: PickTables<'_>,
    eye: [f64; 3],
    dir: [f64; 3],
    alpha: f32,
) -> Option<rewo_world::entity_pick::EntityHit> {
    crosshair_pick_from_table(
        &session.world.entities,
        session.player_id?,
        [session.player.x, session.player.y, session.player.z],
        session.local_attributes(),
        tables,
        eye,
        dir,
        alpha,
        // `cameraEntity.pick(maxDistance, partialTicks, false)`.
        &|from, d, reach| session.target_block(from, d, reach).map(|h| h.distance),
    )
}

/// `Entity.getRootVehicle()` — walk up `set_passengers`'s riding graph.
///
/// Read-only. The loop is bounded by a visit set because a malformed roster
/// could name a cycle, which vanilla's `while (result.isPassenger())` would
/// spin on forever.
pub(super) fn root_vehicle_of(ents: &rewo_world::entities::EntityTable, id: i32) -> i32 {
    let mut cur = id;
    let mut seen = std::collections::HashSet::new();
    while seen.insert(cur) {
        match ents.vehicle_of(cur) {
            Some(next) => cur = next,
            None => break,
        }
    }
    cur
}

/// The table-level half of [`resolve_crosshair_pick`], split out so a gate can
/// drive the real candidate construction without a live session (M73) — the
/// same split [`label_inputs_from_table`] has.
///
/// `block_ray` is `cameraEntity.pick(maxDistance, partialTicks, false)`: it
/// takes `(from, dir, reach)` and returns the distance to the block hit, or
/// `None` for a miss. A closure rather than a world reference so the oracle
/// can place a block at an exact distance and grade the reconciliation
/// directly.
#[allow(clippy::too_many_arguments)]
pub(crate) fn crosshair_pick_from_table(
    ents: &rewo_world::entities::EntityTable,
    camera: i32,
    camera_feet: [f64; 3],
    local_attributes: &rewo_world::attributes::EntityAttributes,
    tables: PickTables<'_>,
    eye: [f64; 3],
    dir: [f64; 3],
    alpha: f32,
    block_ray: &dyn Fn([f64; 3], [f64; 3], f64) -> Option<f64>,
) -> Option<rewo_world::entity_pick::EntityHit> {
    use rewo_data::entity_pick::PickRule;
    use rewo_world::entity_pick::{
        bounding_box, crosshair_pick, Candidate, DimensionInputs, InteractionRanges, PickInputs,
    };

    // Both ranges come from the camera entity's attributes. The local player
    // is not in the entity table (the server sends no `add_entity` for you),
    // so its snapshots are kept beside it — see `PlaySession::local_attributes`.
    let ranges = InteractionRanges::resolve(
        Some(local_attributes),
        Some("minecraft:player"),
        tables.attributes,
    );
    let camera_root = root_vehicle_of(ents, camera);
    let player_type = tables.types.id_of("minecraft:player");

    let mut candidates: Vec<Candidate> = Vec::new();
    for (id, e) in ents.iter() {
        if id == camera {
            continue; // `level.getEntities(except, …)`
        }
        let Some(shape) = tables.shapes.get(e.type_id) else {
            continue;
        };
        let projectile_pickable = tables.redirectable.contains(e.type_id);
        let pickable = match shape.rule {
            PickRule::Never => false,
            PickRule::Always => true,
            // `!isRemoved()`; a removed entity is not in this table.
            PickRule::Alive | PickRule::AliveUnlessSpectator | PickRule::AliveUnlessMarker => true,
            PickRule::RedirectableProjectile => projectile_pickable,
            // `super.isPickable() && !isInGround()`; the ground flag is not
            // decoded, so a landed arrow stays pickable.
            PickRule::RedirectableProjectileNotInGround => projectile_pickable,
        };
        let living = tables.classes.is_living(e.type_id);
        let scale = if living {
            rewo_world::attributes::resolve(
                ents.attributes(id),
                tables.types.name(e.type_id),
                "scale",
                tables.attributes,
            )
            .map_or(1.0, |(v, _)| v as f32)
        } else {
            1.0
        };
        let dims = DimensionInputs {
            width: shape.width,
            height: shape.height,
            living,
            avatar: Some(e.type_id) == player_type,
            pose: ents.pose(id),
            baby: ents.is_baby(id),
            scale,
        };
        candidates.push(Candidate {
            id,
            bb: bounding_box(e.render_pos(alpha), &dims),
            pickable,
            pick_radius: if matches!(
                shape.rule,
                PickRule::RedirectableProjectile | PickRule::RedirectableProjectileNotInGround
            ) && projectile_pickable
            {
                1.0
            } else {
                0.0
            },
            can_be_picked_from_inside: true,
            shares_root_vehicle: root_vehicle_of(ents, id) == camera_root,
        });
    }

    // The block half. `LocalPlayer.pick` casts the block ray to
    // `max(block, entity)`, **not** to the block range — a nearer mob has to
    // be able to shadow a block that is itself out of block reach.
    let block_hit = block_ray(eye, dir, ranges.max());
    // The camera entity's own box seeds the broad-phase search volume. The
    // local player is not in the table, so it is built from the player type's
    // own dimensions at the feet the physics tracks.
    let camera_bb = bounding_box(
        camera_feet,
        &DimensionInputs {
            width: 0.6,
            height: 1.8,
            living: true,
            avatar: true,
            pose: 0,
            baby: false,
            scale: 1.0,
        },
    );
    crosshair_pick(&PickInputs {
        eye,
        dir,
        camera_bb,
        ranges,
        block_hit_distance: block_hit,
        candidates: &candidates,
    })
}

/// Build one entity's [`rewo_world::label::LabelInputs`] — the production seam
/// shared by the live collector and the `labelshot` oracle (M70).
///
/// Shared for the same reason as [`resolve_attack_anim`]: the gate has to prove
/// the mapping the client actually renders through. M45 and M41 both shipped
/// gates that quietly stopped testing their subject by reimplementing a slice
/// of the app's setup.
///
/// **Renderer selection.** Vanilla picks the `shouldShowName` override by
/// renderer class. Rewo has no renderer registry, so: the player type maps to
/// `Avatar`, anything `rewo_world::attributes::resolve` can answer
/// `name_tag_distance` for maps to `Mob`, and everything else to `Other`. That
/// middle equivalence is `DefaultAttributes.SUPPLIERS`, which is keyed by
/// `EntityType<? extends LivingEntity>` — so having a supplier is having a
/// `LivingEntity` renderer. `LabelRenderer::ArmorStand` is transcribed and
/// unit-tested but **never selected here**, because Rewo models no armour
/// stand (`mobs.rs` renders it as a capsule); it exists so the ladder is
/// complete when one lands.
pub(crate) fn resolve_label_inputs<'a>(
    session: &'a PlaySession,
    id: i32,
    entity_name: Option<&str>,
    attr_reg: Option<&rewo_data::attributes::AttributeRegistry>,
    is_player: bool,
    distance_sq: f64,
    viewer: &LabelViewer<'a>,
) -> rewo_world::label::LabelInputs<'a> {
    label_inputs_from_table(
        &session.world.entities,
        id,
        entity_name,
        attr_reg,
        is_player,
        distance_sq,
        viewer,
        session.label_team_of(id),
    )
}

/// The table-level half of [`resolve_label_inputs`], split out so a gate can
/// drive the real input resolution without a live session (M70).
///
/// The caller supplies the entity's team, because that is the one input that
/// needs the scoreboard rather than the entity table.
#[allow(clippy::too_many_arguments)]
pub(crate) fn label_inputs_from_table<'a>(
    ents: &rewo_world::entities::EntityTable,
    id: i32,
    entity_name: Option<&str>,
    attr_reg: Option<&rewo_data::attributes::AttributeRegistry>,
    is_player: bool,
    distance_sq: f64,
    viewer: &LabelViewer<'a>,
    team: Option<rewo_world::label::TeamView<'a>>,
) -> rewo_world::label::LabelInputs<'a> {
    use rewo_world::label::{LabelInputs, LabelRenderer, DEFAULT_NAME_TAG_DISTANCE};
    // `LivingEntityRenderer.extractNameTags` reads
    // `Attributes.NAME_TAG_DISTANCE`; the base passes a literal 64.0. Resolving
    // it is also the living test, so the two answers come from one lookup.
    let resolved = attr_reg.and_then(|reg| {
        rewo_world::attributes::resolve(ents.attributes(id), entity_name, "name_tag_distance", reg)
    });
    let renderer = if is_player {
        LabelRenderer::Avatar
    } else if entity_name == Some("minecraft:armor_stand") {
        // M271 — `ArmorStandRenderer` overrides the name rule with
        // `isCustomNameVisible()` alone; until M271 a stand took the `Mob`
        // rule, so an invisible stand (a server hologram) hid its name.
        LabelRenderer::ArmorStand
    } else if resolved.is_some() {
        LabelRenderer::Mob
    } else {
        LabelRenderer::Other
    };
    LabelInputs {
        renderer,
        distance_sq,
        name_tag_distance: resolved.map_or(DEFAULT_NAME_TAG_DISTANCE, |(d, _)| d),
        is_discrete: ents.is_discrete(id),
        is_invisible: ents.is_invisible(id),
        is_vehicle: ents.is_vehicle(id),
        is_camera_entity: viewer.camera_entity == Some(id),
        is_local_player: viewer.local_player == Some(id),
        hud_hidden: viewer.hud_hidden,
        viewer_spectator: viewer.spectator,
        team,
        viewer_team: viewer.team,
        // `entity.shouldShowName()` — `Player` overrides it to a literal
        // `true`; everything else inherits `isCustomNameVisible()`.
        entity_should_show_name: is_player || ents.is_custom_name_visible(id),
        has_custom_name: ents.custom_name(id).is_some(),
        // `entity == entityRenderDispatcher.crosshairPickEntity` (M73). M70
        // fed this a hard `false` because Rewo's raycast was voxel-only; the
        // frame's single pick now answers it — see `resolve_crosshair_pick`.
        is_crosshair_pick: viewer.crosshair_pick == Some(id),
    }
}

/// Both of an `EntityDraw`'s label fields, resolved together from one
/// predicate — the production seam shared by the live collector and the
/// `labelshot` oracle (M70).
///
/// This exists so "the nametag and the health bar agree" is a property of one
/// function rather than of two call sites that happen to line up. Before M70
/// they did not: the bar had a three-gate subset of `shouldShowName` and the
/// tag had none at all, so an invisible named mob showed a name and no bar.
///
/// `name` is the candidate string the caller already chose — a player's
/// profile name or anything else's metadata custom name. Whether it is *drawn*
/// is the predicate's answer, not the mere existence of the string.
pub(crate) fn resolve_labels<'a>(
    ents: &rewo_world::entities::EntityTable,
    id: i32,
    entity_name: Option<&str>,
    attr_reg: Option<&rewo_data::attributes::AttributeRegistry>,
    label: &rewo_world::label::LabelInputs<'_>,
    name: Option<&'a str>,
) -> (Option<&'a str>, Option<rewo_gpu::entities::HealthBar>) {
    let shown = rewo_world::label::should_show_name(label)
        .then_some(name)
        .flatten();
    // `None` without an attribute registry, which is the same fail-closed
    // answer the resolver gives for an unsynced max — a bar is never drawn on
    // a guess.
    let bar = attr_reg.and_then(|reg| resolve_health_bar(ents, id, entity_name, reg, label));
    (shown, bar)
}

/// `REWO_HEALTH_BAR_SPEC.md` rules 4 and 5 — whether this entity gets a bar.
///
/// **M70 moved rule 5 out of here.** It used to be three hand-rolled gates
/// (living, name-tag distance, invisible) that were a strict subset of what
/// suppresses a nametag, and the nametag path had a *different* subset — namely
/// none. Both now go through [`rewo_world::label`], so "suppressed by
/// everything that suppresses a nametag" is true by construction rather than by
/// two lists happening to agree.
///
/// What stays here is rule 4, which is not a visibility question: a bar needs a
/// max health an `update_attributes` actually established. `Source::Default` is
/// rejected even though the supplier's 20.0 is a real number for every living
/// entity, because Rewo cannot tell "the server never sent health" from "this
/// mob has 1 HP" — `DATA_HEALTH_ID` is seeded at `1.0F` — so a bar with an
/// unverified denominator would be a confident lie in both directions.
pub(crate) fn resolve_health_bar(
    ents: &rewo_world::entities::EntityTable,
    id: i32,
    entity_name: Option<&str>,
    reg: &rewo_data::attributes::AttributeRegistry,
    label: &rewo_world::label::LabelInputs<'_>,
) -> Option<rewo_gpu::entities::HealthBar> {
    use rewo_world::attributes::{resolve, Source};
    // Rule 5, in one call, shared with the nametag.
    if !rewo_world::label::should_show_health_bar(label) {
        return None;
    }
    // Rule 4.
    let (max, source) = resolve(ents.attributes(id), entity_name, "max_health", reg)?;
    if source != Source::Synced {
        return None;
    }
    Some(rewo_gpu::entities::HealthBar {
        current: ents.death_state(id).health,
        max: max as f32,
    })
}

/// Resolve an entity's Allay dance render inputs — the production seam shared by
/// the live collector and the `danceshot` oracle, so the kind gate + the
/// counter → `(is_spinning, spinning_progress)` → [`rewo_gpu::mobs::AllayDance`]
/// mapping are the same code the gate proves. `Some` only for an Allay-kind
/// entity that is currently dancing; every other kind is inert here even if the
/// entity somehow carried a dance clock.
pub(crate) fn resolve_allay_dance(
    kind: EntityModelKind,
    entities: &rewo_world::entities::EntityTable,
    id: i32,
    alpha: f32,
) -> Option<rewo_gpu::mobs::AllayDance> {
    (kind == EntityModelKind::Allay)
        .then(|| entities.allay_dance_render(id, alpha))
        .flatten()
        .map(
            |(is_spinning, spinning_progress)| rewo_gpu::mobs::AllayDance {
                is_spinning,
                spinning_progress,
            },
        )
}

/// Resolve an entity's combat-swing render inputs — the production seam shared
/// by the live collector and the `swingshot` oracle, so the
/// `ArmedEntityRenderState` extraction (`attackTime` / `attackArm` /
/// `swingAnimationType` / `ageScale`) is the same code the gate proves.
///
/// Deliberately **not** kind-gated: `extractArmedEntityRenderState` runs for
/// every armed entity, and the value also feeds CEM's `swing_progress`. Only a
/// model built from `HumanoidModel.createMesh` carries parts that pose from it,
/// so a mob simply ignores a non-zero `attackTime` (witnessed in the gate).
pub(crate) fn resolve_attack_anim(
    entities: &rewo_world::entities::EntityTable,
    id: i32,
    alpha: f32,
) -> rewo_gpu::mobs::SwingPose {
    use rewo_data::swing_anim::SwingAnimationType;
    use rewo_gpu::mobs::{SwingKind, SwingPose};
    use rewo_world::entities::HumanoidArm;
    // An input that could not be resolved exactly suppresses the whole pose —
    // `attack_time` 0 short-circuits `setupAttackAnimation` and publishes 0 to
    // CEM's `swing_progress`, which is the honest answer when the held item is
    // unknowable. A later exact equipment update lifts it.
    let Some(kind) = entities.swing_animation_type(id) else {
        return SwingPose {
            inputs_known: false,
            ..SwingPose::NONE
        };
    };
    if !entities.swing_inputs_known(id) {
        return SwingPose {
            inputs_known: false,
            ..SwingPose::NONE
        };
    }
    SwingPose {
        attack_time: entities.attack_anim(id, alpha),
        left_arm: entities.attack_arm(id) == HumanoidArm::Left,
        kind: match kind {
            SwingAnimationType::None => SwingKind::None,
            SwingAnimationType::Whack => SwingKind::Whack,
            SwingAnimationType::Stab => SwingKind::Stab,
        },
        // `LivingEntity.getAgeScale()` — `isBaby() ? 0.5 : 1.0`.
        age_scale: if entities.is_baby(id) { 0.5 } else { 1.0 },
        inputs_known: true,
    }
}

/// `AvatarRenderer.getArmPose` / `HumanoidMobRenderer.getArmPose` for both
/// arms, plus every `HumanoidRenderState` field the pose dispatch reads — the
/// *hold* baseline applied before `setupAttackAnimation`.
///
/// Shared by the live collector and the `swingshot` oracle for the same reason
/// as [`resolve_attack_anim`]: the gate must prove the mapping the client
/// actually renders through, not a parallel copy of it.
///
/// **Two different functions, selected by renderer.** This was collapsed to one
/// through M22 and is split here, because the two disagree on the common case:
///
/// - `AvatarRenderer.getArmPose` (players) runs the full eleven-pose ladder and
///   falls through to `ITEM`.
/// - `HumanoidMobRenderer.getArmPose` (every humanoid mob) checks only
///   STAB-while-swinging and the `minecraft:spears` tag, and otherwise returns
///   **`EMPTY`** — so an armed zombie's arm hangs at its walk pose, not 18°
///   higher. Subclasses layer on top: skeletons add `BOW_AND_ARROW`, the
///   drowned adds `THROW_TRIDENT`.
///
/// **Vanilla computes a pose per *hand*, then selects by arm.** Resolving
/// directly per arm is *not* the same function once two-handed poses exist:
/// `if (mainHandPose.isTwoHanded()) offHandPose = offHandItem.isEmpty() ? EMPTY
/// : ITEM` rewrites the off-hand pose from the main hand's, which has no
/// per-arm expression. So the per-hand shape is transcribed literally.
pub(crate) fn resolve_arm_poses(
    entities: &rewo_world::entities::EntityTable,
    id: i32,
    kind: rewo_gpu::mobs::EntityModelKind,
    spears: &rewo_data::item_tags::ItemTag,
    bow_item: Option<i32>,
    crossbow_item: Option<i32>,
) -> rewo_gpu::mobs::ArmPoses {
    use rewo_data::swing_anim::SwingAnimationType;
    use rewo_data::use_item::ItemUseAnimation as A;
    use rewo_gpu::mobs::{ArmPose, ArmPoses, EntityModelKind as K};
    use rewo_world::entities::{HandItem, HumanoidArm, InteractionHand};

    let swinging = entities.is_swinging(id);
    let is_avatar = matches!(kind, K::Player | K::PlayerSlim);
    let use_state = entities.use_state(id);
    let mut known = true;

    // `AvatarRenderer.getArmPose(avatar, itemInHand, hand)` — the per-hand
    // ladder, in vanilla's order. The order is load-bearing: the
    // charged-crossbow hold is tested *before* the use gate, so a crossbow that
    // is already charged holds rather than charges.
    let avatar_pose = |hand: InteractionHand, known: &mut bool| -> ArmPose {
        let held = match entities.hand_item(id, hand) {
            HandItem::Empty => return ArmPose::Empty,
            HandItem::Unknown => {
                *known = false;
                return ArmPose::Empty;
            }
            HandItem::Held(h) => h,
        };
        if !swinging && Some(held.item_id) == crossbow_item && held.charged.is_some() {
            return ArmPose::CrossbowHold;
        }
        if use_state.poses_hand(hand) {
            // `switch (itemInHand.getUseAnimation())`. EAT, DRINK, BUNDLE and
            // NONE have no case, so they fall out of the switch and continue to
            // the STAB / spear-tag tail below — they are not poses.
            match held.use_profile.animation {
                A::Block => return ArmPose::Block,
                A::Bow => return ArmPose::BowAndArrow,
                A::Trident => return ArmPose::ThrowTrident,
                A::Crossbow => return ArmPose::CrossbowCharge,
                A::Spyglass => return ArmPose::Spyglass,
                A::TootHorn => return ArmPose::TootHorn,
                A::Brush => return ArmPose::Brush,
                A::Spear => return ArmPose::Spear,
                A::None | A::Eat | A::Drink | A::Bundle => {}
            }
        }
        if (held.swing.kind == SwingAnimationType::Stab && swinging)
            || spears.contains(held.item_id)
        {
            ArmPose::Spear
        } else {
            ArmPose::Item
        }
    };

    // `HumanoidMobRenderer.getArmPose(mob, arm)` plus the two subclass
    // overrides Rewo's kinds can reach. Takes an *arm*, not a hand: the mob
    // path never rewrites the off-hand pose, so there is nothing to express
    // per-hand.
    let mob_pose = |arm: HumanoidArm, known: &mut bool| -> ArmPose {
        // `AbstractSkeletonRenderer`: main arm && isAggressive && main hand is
        // a bow. Checked first because it short-circuits `super.getArmPose`.
        let skeletal = matches!(
            kind,
            K::Skeleton | K::Stray | K::Bogged | K::WitherSkeleton | K::Parched
        );
        if skeletal
            && entities.main_arm(id) == arm
            && entities.mob_state(id).is_aggressive()
            && entities
                .hand_item(id, InteractionHand::MainHand)
                .held()
                .is_some_and(|h| Some(h.item_id) == bow_item)
        {
            return ArmPose::BowAndArrow;
        }
        let held = match entities.item_by_arm(id, arm) {
            HandItem::Empty => return ArmPose::Empty,
            HandItem::Unknown => {
                *known = false;
                return ArmPose::Empty;
            }
            HandItem::Held(h) => h,
        };
        // `DrownedRenderer`: main arm && isAggressive && holding a trident.
        // Reached through the trident's own use animation rather than an item
        // id, which is exact — `TridentItem` is the only thing that answers
        // `ItemUseAnimation.TRIDENT`.
        if kind == K::Drowned
            && entities.main_arm(id) == arm
            && entities.mob_state(id).is_aggressive()
            && held.use_profile.animation == A::Trident
        {
            return ArmPose::ThrowTrident;
        }
        if (held.swing.kind == SwingAnimationType::Stab && swinging)
            || spears.contains(held.item_id)
        {
            ArmPose::Spear
        } else {
            // The mob path's fall-through is EMPTY, not ITEM.
            ArmPose::Empty
        }
    };

    let (right, left) = if is_avatar {
        let main = avatar_pose(InteractionHand::MainHand, &mut known);
        let mut off = avatar_pose(InteractionHand::OffHand, &mut known);
        // `if (mainHandPose.isTwoHanded()) offHandPose = offHandItem.isEmpty()
        //      ? EMPTY : ITEM;`
        if main.is_two_handed() {
            off = match entities.hand_item(id, InteractionHand::OffHand) {
                HandItem::Empty => ArmPose::Empty,
                _ => ArmPose::Item,
            };
        }
        // `return avatar.getMainArm() == arm ? mainHandPose : offHandPose;`
        if entities.main_arm(id) == HumanoidArm::Right {
            (main, off)
        } else {
            (off, main)
        }
    } else {
        (
            mob_pose(HumanoidArm::Right, &mut known),
            mob_pose(HumanoidArm::Left, &mut known),
        )
    };

    // `HumanoidMobRenderer.extractHumanoidRenderState` — a static helper, so
    // players carry these too (`AvatarRenderer:168` calls it).
    let charging = right == ArmPose::CrossbowCharge || left == ArmPose::CrossbowCharge;
    ArmPoses {
        right,
        left,
        right_handed: entities.main_arm(id) == HumanoidArm::Right,
        known,
        using_item: use_state.using,
        main_hand_used: use_state.hand == InteractionHand::MainHand,
        ticks_using_item: use_state.ticks_using_item_partial(0.0),
        // `CrossbowItem.getChargeDuration(entity.getUseItem(), entity)`. The
        // helper computes it unconditionally, but only the CROSSBOW_CHARGE pose
        // reads it, and an enchanted crossbow never gets this far.
        max_crossbow_charge: if charging {
            ArmPoses::CROSSBOW_CHARGE_DURATION
        } else {
            0.0
        },
    }
}

/// The synced mob state the M20 arm rigs read, plus the derived
/// `IllagerArmPose` — the client-side half of `getArmPose()` for each illager
/// class, which vanilla computes per subclass rather than syncing.
///
/// Shared by the live collector and the `swingshot` oracle for the same reason
/// as [`resolve_attack_anim`] and [`resolve_arm_poses`].
///
/// `bow_item` is `Items.BOW`'s protocol id; `None` (a client that could not
/// resolve it) leaves `holding_bow` false, which keeps the skeleton attack rig
/// *enabled* — the conservative direction, since suppressing it would hide a
/// real animation rather than show a wrong one.
pub(crate) fn resolve_mob_combat(
    entities: &rewo_world::entities::EntityTable,
    id: i32,
    kind: rewo_gpu::mobs::EntityModelKind,
    bow_item: Option<i32>,
) -> rewo_gpu::mobs::MobCombat {
    use rewo_gpu::mobs::{EntityModelKind as K, IllagerArmPose as P, MobCombat};
    use rewo_world::entities::{HandItem, HumanoidArm, InteractionHand};

    let st = entities.mob_state(id);
    let main = entities.hand_item(id, InteractionHand::MainHand);
    let main_hand_empty = matches!(main, HandItem::Empty);
    // `entity.getMainHandItem().is(Items.BOW)`.
    let holding_bow = main
        .held()
        .zip(bow_item)
        .is_some_and(|(i, bow)| i.item_id == bow);
    // `AbstractIllager.getArmPose()`, per subclass. The base class answers
    // CROSSED, which is also what a non-illager gets (and never reads).
    let illager_pose = match kind {
        // `Pillager`: charging → CROSSBOW_CHARGE; holding a crossbow →
        // CROSSBOW_HOLD; else aggressive ? ATTACKING : NEUTRAL.
        K::Pillager => {
            if st.charging_crossbow {
                P::CrossbowCharge
            } else if main
                .held()
                .is_some_and(|i| Some(i.item_id) == crossbow_item_id())
            {
                P::CrossbowHold
            } else if st.is_aggressive() {
                P::Attacking
            } else {
                P::Neutral
            }
        }
        // `Vindicator`: aggressive → ATTACKING; else celebrating ?
        // CELEBRATING : CROSSED.
        K::Vindicator => {
            if st.is_aggressive() {
                P::Attacking
            } else if st.celebrating {
                P::Celebrating
            } else {
                P::Crossed
            }
        }
        // `SpellcasterIllager` (Evoker): casting → SPELLCASTING; else
        // celebrating ? CELEBRATING : CROSSED.
        K::Evoker => {
            if st.is_casting_spell() {
                P::Spellcasting
            } else if st.celebrating {
                P::Celebrating
            } else {
                P::Crossed
            }
        }
        // `Illusioner` overrides it: casting → SPELLCASTING; else aggressive ?
        // BOW_AND_ARROW : CROSSED. Note it never celebrates.
        K::Illusioner => {
            if st.is_casting_spell() {
                P::Spellcasting
            } else if st.is_aggressive() {
                P::BowAndArrow
            } else {
                P::Crossed
            }
        }
        _ => P::Crossed,
    };
    MobCombat {
        aggressive: st.is_aggressive(),
        main_hand_empty,
        holding_bow,
        is_baby: entities.is_baby(id),
        main_arm_left: entities.main_arm(id) == HumanoidArm::Left,
        illager_pose,
        // M352 — only an enderman's is synched (index 16, kind-gated).
        carried_block: (kind == K::Enderman)
            .then(|| entities.carried_block(id))
            .flatten(),
        // M353 — only a snow golem's is synched.
        pumpkin: kind == K::SnowGolem && entities.has_pumpkin(id),
        // M354 — set by the caller, which has the world clock the anger needs.
        bee_angry: false,
        bee_stung: false,
        // M355 — likewise the wolf's.
        wolf_tail: None,
        wolf_angry: false,
        // M356 — set by the caller with the wolf's.
        sitting: false,
        age_scale: 1.0,
        // M358 — set by the caller with the sheep's eat clock.
        sheep_eat: None,
        // M363 — set by the caller for a boat.
        under_water: false,
        // M387 — set by the caller for an axolotl.
        axolotl_in_water: false,
        axolotl_moving: false,
        axolotl_playing_dead: false,
        // M381 — set by the caller for a rider.
        riding: false,
        // M379 — set by the caller for a witch.
        witch_nose_rate: 0.0,
        witch_holding: false,
        // M377 — set by the caller for a camel.
        camel_jump_cooldown: 0.0,
        // M371 — set by the caller for a dolphin.
        dolphin_moving: false,
        // M369 — set by the caller for an allay.
        allay_holding: 0.0,
        // M367 — set by the caller for a hoglin.
        headbutt: 0.0,
        // M366 — set by the caller for a strider.
        ridden: false,
        // M365 — set by the caller for a turtle.
        turtle_egg: false,
        turtle_laying: false,
        turtle_on_land: false,
    }
}

/// `Items.CROSSBOW`'s protocol id, resolved once. The pillager pose test is
/// `isHolding(Items.CROSSBOW)`, an item identity check like the skeleton's bow.
pub(super) fn crossbow_item_id() -> Option<i32> {
    CROSSBOW_ITEM.get().copied().flatten()
}

/// Set once at session setup; `None` until then (and on a client that cannot
/// resolve the item), which makes the CROSSBOW_HOLD arm unreachable rather
/// than guessed.
pub(crate) static CROSSBOW_ITEM: std::sync::OnceLock<Option<i32>> = std::sync::OnceLock::new();

/// M234 — the item models that branch on use (the bow's and crossbow's
/// pull, the crossbow's charge, the goat horn's toot) and the firework
/// rocket the crossbow's `Charge` select asks about, set once at session
/// setup beside [`CROSSBOW_ITEM`]. Unset, every item draws its plain model.
pub(crate) static USE_VARIANTS: std::sync::OnceLock<rewo_data::held_items::UseVariants> =
    std::sync::OnceLock::new();
pub(crate) static FIREWORK_ROCKET_ITEM: std::sync::OnceLock<Option<i32>> =
    std::sync::OnceLock::new();

/// `CrossbowItem.getChargeDuration` without Quick Charge:
/// `Mth.floor(1.25F * 20.0F)`.
pub(crate) const CROSSBOW_CHARGE_TICKS: i32 = 25;

/// M234 — the model an entity's stack in `hand` draws with: the item model's
/// use properties read the entity's use when `hand` is its use hand
/// (`IsUsingItem` / `UseDuration` both test `getUseItem() == itemStack`), and
/// a crossbow's loaded projectile (`charged`, its item id) answers `Charge`
/// and zeroes `CrossbowPull`. `name` itself for anything without a variant.
///
/// Recorded: Quick Charge does not shorten [`CROSSBOW_CHARGE_TICKS`] (the
/// enchantment's effect is a datapack value the client does not resolve), so
/// an enchanted crossbow's pull frames run late.
pub(crate) fn use_model(
    name: &str,
    use_state: rewo_world::entities::UseState,
    hand: rewo_world::entities::InteractionHand,
    charged: Option<i32>,
) -> &str {
    let Some(variants) = USE_VARIANTS.get() else {
        return name;
    };
    if !variants.is_dynamic(name) {
        return name;
    }
    let mut ctx = rewo_data::item_models::SelectionContext::hand();
    if use_state.using && use_state.hand == hand {
        let ticks = use_state.ticks_using_item();
        let pull = if charged.is_some() {
            0.0
        } else {
            ticks as f32 / CROSSBOW_CHARGE_TICKS as f32
        };
        ctx = ctx.with_use(ticks, use_state.remaining_ticks(), pull);
    }
    let rocket = FIREWORK_ROCKET_ITEM.get().copied().flatten();
    ctx = ctx.with_charge(charged.map(|p| if Some(p) == rocket { "rocket" } else { "arrow" }));
    variants.select(name, ctx)
}

/// The crosshair pick's two version tables (M73), resolved once at session
/// setup — per-type bounding-box dimensions with `isPickable()`, and the
/// `redirectable_projectile` tag that rule reads.
///
/// Statics for the same reason [`CROSSBOW_ITEM`] is one: both `collect_entities`
/// call sites need them and neither owns the loader. `None` until set, which
/// makes the pick return `None` rather than pick with a guessed hitbox.
pub(crate) static PICK_SHAPES: std::sync::OnceLock<rewo_data::entity_pick::EntityPickTable> =
    std::sync::OnceLock::new();
pub(crate) static REDIRECTABLE: std::sync::OnceLock<rewo_data::entity_pick::EntityTypeTag> =
    std::sync::OnceLock::new();

/// The frame's `crosshairPickEntity`, resolved from the session and the two
/// statics above — the one call the render path makes.
///
/// `None` whenever any input is missing (no attribute registry, no entity
/// classes, tables unset), which is the same fail-closed answer M70 shipped:
/// a name-tagged mob whose `CustomNameVisible` is unset simply stays silent.
pub(crate) fn frame_crosshair_pick(
    session: &PlaySession,
    etypes: &EntityTypes,
    alpha: f32,
) -> Option<i32> {
    frame_crosshair_hit(session, etypes, alpha).map(|h| h.id)
}

/// [`frame_crosshair_pick`]'s whole hit (M216): the click paths need
/// `getLocation()` as well as the entity.
pub(crate) fn frame_crosshair_hit(
    session: &PlaySession,
    etypes: &EntityTypes,
    alpha: f32,
) -> Option<rewo_world::entity_pick::EntityHit> {
    let tables = PickTables {
        types: etypes,
        classes: session.entity_classes.as_deref()?,
        shapes: PICK_SHAPES.get()?,
        redirectable: REDIRECTABLE.get()?,
        attributes: session.attribute_registry.as_deref()?,
    };
    let eye = eye_f64(session);
    let dir = look_dir(session.player.yaw, session.player.pitch);
    resolve_crosshair_pick(session, tables, eye, dir, alpha)
}

/// Convert the baked held-item models across the `rewo-data` → `rewo-gpu`
/// seam (M22). The two shapes are deliberately identical so this stays
/// mechanical; `rewo-gpu` keeps no `rewo-data` dependency, the same rule
/// `SwingKind` / `MobCombat` already follow.
pub(crate) fn to_gpu_held_items(
    src: &rewo_data::held_items::HeldItems,
) -> rewo_gpu::held::HeldItems {
    let conv_models =
        |m: &std::collections::HashMap<String, rewo_data::held_items::HeldItemModel>| {
            m.iter()
                .map(|(k, m)| (k.clone(), to_gpu_held_model(m)))
                .collect()
        };
    rewo_gpu::held::HeldItems {
        models: conv_models(&src.models),
        block_entities: conv_models(&src.block_entities),
        textures: src.textures.iter().map(to_gpu_held_texture).collect(),
    }
}

/// One held model in the GPU crate's shape.
pub(crate) fn to_gpu_held_model(
    m: &rewo_data::held_items::HeldItemModel,
) -> rewo_gpu::held::HeldItemModel {
    use rewo_gpu::held as g;
    let conv_t = |t: &rewo_data::item_models::DisplayTransform| g::DisplayTransform {
        rotation: t.rotation,
        translation: t.translation,
        scale: t.scale,
    };
    let conv_quads = |qs: &[rewo_data::held_items::HeldQuad]| -> Vec<g::HeldQuad> {
        qs.iter()
            .map(|q| g::HeldQuad {
                verts: q.verts,
                uv: q.uv,
                tex: q.tex,
                part: q.part,
                dir: q.dir,
            })
            .collect()
    };
    g::HeldItemModel {
        quads: conv_quads(&m.quads),
        right: conv_t(&m.right),
        left: conv_t(&m.left),
        ground: conv_t(&m.ground),
        gui: conv_t(&m.gui),
        first_right: conv_t(&m.first_right),
        first_left: conv_t(&m.first_left),
        fixed: conv_t(&m.fixed),
        head: conv_t(&m.head),
        from_block: m.from_block,
        gui_quads: m.gui_quads.as_deref().map(conv_quads),
    }
}

/// One held texture in the GPU crate's shape.
pub(crate) fn to_gpu_held_texture(
    t: &rewo_data::held_items::HeldTexture,
) -> rewo_gpu::held::HeldTexture {
    rewo_gpu::held::HeldTexture {
        w: t.w,
        h: t.h,
        rgba: t.rgba.clone(),
    }
}

/// M261 — the block a minecart carries and its display offset in px: the
/// custom display block when the server sent one, else the type's
/// `getDefaultDisplayBlockState` (a chest facing north, a furnace facing
/// north and lit while it has fuel, a hopper, TNT, a spawner, a command
/// block; the plain minecart none), and the offset sent or the type's
/// `getDefaultDisplayOffset` (8 for the chest, 1 for the hopper, else 6).
pub(crate) fn minecart_contents(
    name: &str,
    d: rewo_world::entities::MinecartDisplay,
    blocks: &rewo_data::blocks::Blocks,
) -> Option<(u32, i32)> {
    let default_offset = match name {
        "minecraft:chest_minecart" => 8,
        "minecraft:hopper_minecart" => 1,
        _ => 6,
    };
    let state = match d.custom_block {
        Some(s) => s,
        None => match name {
            "minecraft:chest_minecart" => {
                blocks.state_with("minecraft:chest", &[("facing", "north")])?
            }
            "minecraft:furnace_minecart" => blocks.state_with(
                "minecraft:furnace",
                &[
                    ("facing", "north"),
                    ("lit", if d.fuel { "true" } else { "false" }),
                ],
            )?,
            "minecraft:hopper_minecart" => blocks.default_state("minecraft:hopper")?,
            "minecraft:tnt_minecart" => blocks.default_state("minecraft:tnt")?,
            "minecraft:spawner_minecart" => blocks.default_state("minecraft:spawner")?,
            "minecraft:command_block_minecart" => {
                blocks.default_state("minecraft:command_block")?
            }
            _ => return None,
        },
    };
    Some((state, d.offset.unwrap_or(default_offset)))
}

/// M261 — `AbstractMinecartRenderer`'s pose for the carried block, block
/// units about the entity: the fallback placement `translate(0, 0.375, 0)
/// · YP(180 - yRot)` the cart body shares, then `scale(0.75) ·
/// translate(-0.5, (offset - 8)/16, 0.5) · YP(90)`, the block from its
/// origin.
pub(crate) fn minecart_contents_transform(
    yaw: f32,
    offset: i32,
) -> rewo_data::be_transform::Affine {
    use rewo_data::be_transform::{mul, rot_y, scale, translation};
    mul(
        &mul(&translation(0.0, 0.375, 0.0), &rot_y(180.0 - yaw)),
        &mul(
            &mul(
                &scale(0.75, 0.75, 0.75),
                &translation(-0.5, (offset - 8) as f32 / 16.0, 0.5),
            ),
            &rot_y(90.0),
        ),
    )
}

/// M261 — every minecart's carried block this frame: a block-entity block
/// (the chest) as its block-entity model at rest under its own facing, any
/// other as its block model (`rewo:be/state/<id>`, built at run time when the
/// bake did not make it). Recorded: no slope pitch, rail position or hurt
/// wobble (the cart body has none either), a TNT minecart does not flash,
/// and a spawner's caged mob is not drawn.
pub(super) fn collect_minecart_contents(
    session: &PlaySession,
    etypes: &EntityTypes,
    blocks: &rewo_data::blocks::Blocks,
    chests: &rewo_data::chest_states::ChestStates,
    lightmap: &LightmapState,
    alpha: f32,
) -> Vec<super::block_entities::OwnedBlockEntityDraw> {
    use rewo_data::be_transform::{mul, IDENTITY};
    use rewo_data::chest_states::BlockEntityAnim;
    let mut out = Vec::new();
    for (id, e) in session.world.entities.iter() {
        let Some(name) = etypes.name(e.type_id) else {
            continue;
        };
        let Some((state, offset)) =
            minecart_contents(name, session.world.entities.minecart_display(id), blocks)
        else {
            continue;
        };
        let p = e.render_pos(alpha);
        let base = minecart_contents_transform(e.yaw, offset);
        let at_rest = (
            [IDENTITY; rewo_gpu::entities::MAX_PARTS],
            [[0.0; 3]; rewo_gpu::entities::MAX_PARTS],
        );
        let (model, transform, (part_transforms, part_pivots)) = match chests.draw_for(state) {
            Some(be) => {
                let parts = match be.anim {
                    BlockEntityAnim::ChestLid(_) => super::one_part(
                        rewo_data::block_entity_models::CHEST_LID_PART,
                        rewo_data::be_transform::chest_lid_part(
                            0.0,
                            rewo_data::block_entity_models::CHEST_LID_PIVOT,
                        ),
                        rewo_data::block_entity_models::CHEST_LID_PIVOT,
                    ),
                    BlockEntityAnim::ShulkerLid => super::one_part(
                        rewo_data::block_entity_models::SHULKER_LID_PART,
                        rewo_data::be_transform::shulker_lid_part(0.0),
                        rewo_data::block_entity_models::SHULKER_LID_PIVOT,
                    ),
                    _ => continue,
                };
                (be.model, mul(&base, &be.transform), parts)
            }
            None => (
                rewo_data::block_entity_models::carried_block_model(state),
                base,
                at_rest,
            ),
        };
        out.push(super::block_entities::OwnedBlockEntityDraw {
            pos: [p[0] as f32, p[1] as f32, p[2] as f32],
            model,
            transform,
            light: entity_light(&session.world, p[0], p[1], p[2], lightmap),
            part_transforms,
            part_pivots,
            tint: [1.0; 3],
            fixed_display: false,
            glint: false,
            white: 0.0,
        });
    }
    out
}

/// M351 — `MushroomCowMushroomLayer.submit`'s three mushroom blocks, as
/// block-local → entity-relative transforms (block units), for an adult
/// mooshroom drawn at body `yaw` with its head turned `net_head_yaw` and
/// pitched `pitch` (degrees) and toppled `roll` (radians, the mob pass's
/// `death_roll_for`). `LivingEntityRenderer`'s root first — `YP(180 -
/// bodyRot)`, the topple `ZP`, `scale(-1, -1, 1)`, `translate(0, -1.501, 0)`,
/// the same chain the mob pass applies to the model — then the layer's: two
/// on the body, `translate(0.2, -0.35, 0.5)` then (the second only)
/// `YP(42)`, `translate(0.1, 0, -0.6)`; and one on the head,
/// `getHead().translateAndRotate` (the cow head's pivot (0, 4, -8) px, its
/// `xRot` = pitch and `yRot` = net head yaw), `translate(0, -0.7, -0.2)`.
/// Each ends `YP(-48)` (the head's `YP(-78)`), `scale(-1, -1, 1)`,
/// `translate(-0.5, -0.5, -0.5)` around the block model.
pub(crate) fn mooshroom_mushroom_transforms(
    yaw: f32,
    net_head_yaw: f32,
    pitch: f32,
    roll: f32,
) -> [rewo_data::be_transform::Affine; 3] {
    use rewo_data::be_transform::{mul, rot_y, rot_z, rot_zyx, scale, translation};
    let root = mul(
        &rot_y(180.0 - yaw),
        &mul(
            &rot_z(roll),
            &mul(&scale(-1.0, -1.0, 1.0), &translation(0.0, -1.501, 0.0)),
        ),
    );
    let block = |deg: f32| {
        mul(
            &rot_y(deg),
            &mul(&scale(-1.0, -1.0, 1.0), &translation(-0.5, -0.5, -0.5)),
        )
    };
    let back = translation(0.2, -0.35, 0.5);
    let first = mul(&root, &mul(&back, &block(-48.0)));
    let second = mul(
        &root,
        &mul(
            &back,
            &mul(
                &rot_y(42.0),
                &mul(&translation(0.1, 0.0, -0.6), &block(-48.0)),
            ),
        ),
    );
    let head = mul(
        &translation(0.0, 0.25, -0.5),
        &rot_zyx(pitch.to_radians(), net_head_yaw.to_radians(), 0.0),
    );
    let third = mul(
        &root,
        &mul(&head, &mul(&translation(0.0, -0.7, -0.2), &block(-78.0))),
    );
    [first, second, third]
}

/// M352 — `CarriedBlockLayer.submit`'s block, block-local → entity-relative
/// (block units), for an enderman drawn at body `yaw` and toppled `roll`:
/// `LivingEntityRenderer`'s root, then `translate(0, 0.6875, -0.75)`,
/// `XP(20)`, `YP(45)`, `translate(0.25, 0.1875, 0.25)`, `scale(-0.5, -0.5,
/// 0.5)`, `YP(90)`, and the block model from its corner (no centring).
pub(crate) fn enderman_block_transform(yaw: f32, roll: f32) -> rewo_data::be_transform::Affine {
    use rewo_data::be_transform::{mul, rot_x, rot_y, rot_z, scale, translation};
    let root = mul(
        &rot_y(180.0 - yaw),
        &mul(
            &rot_z(roll),
            &mul(&scale(-1.0, -1.0, 1.0), &translation(0.0, -1.501, 0.0)),
        ),
    );
    let layer = mul(
        &translation(0.0, 0.6875, -0.75),
        &mul(
            &rot_x(20f32.to_radians()),
            &mul(
                &rot_y(45.0),
                &mul(
                    &translation(0.25, 0.1875, 0.25),
                    &mul(&scale(-0.5, -0.5, 0.5), &rot_y(90.0)),
                ),
            ),
        ),
    );
    mul(&root, &layer)
}

/// M353 — `SnowGolemHeadLayer.submit`'s carved pumpkin, block-local →
/// entity-relative (block units), for a snow golem drawn at body `yaw`,
/// head turned `net_head_yaw` and pitched `pitch` (degrees), toppled `roll`:
/// `LivingEntityRenderer`'s root, `getHead().translateAndRotate` (the head's
/// pivot (0, 4, 0) px, its pitch and net yaw), `translate(0, -0.34375, 0)`,
/// `YP(180)`, `scale(0.625, -0.625, -0.625)`, `translate(-0.5, -0.5, -0.5)`.
pub(crate) fn snow_golem_pumpkin_transform(
    yaw: f32,
    net_head_yaw: f32,
    pitch: f32,
    roll: f32,
) -> rewo_data::be_transform::Affine {
    use rewo_data::be_transform::{mul, rot_y, rot_z, rot_zyx, scale, translation};
    let root = mul(
        &rot_y(180.0 - yaw),
        &mul(
            &rot_z(roll),
            &mul(&scale(-1.0, -1.0, 1.0), &translation(0.0, -1.501, 0.0)),
        ),
    );
    let head = mul(
        &translation(0.0, 0.25, 0.0),
        &rot_zyx(pitch.to_radians(), net_head_yaw.to_radians(), 0.0),
    );
    let layer = mul(
        &translation(0.0, -0.34375, 0.0),
        &mul(
            &rot_y(180.0),
            &mul(
                &scale(0.625, -0.625, -0.625),
                &translation(-0.5, -0.5, -0.5),
            ),
        ),
    );
    mul(&root, &mul(&head, &layer))
}

/// M351 — the blocks the mobs drawn this frame carry, built from their own
/// draws so a block follows exactly the pose its mob drew in: M353 — a snow
/// golem's carved pumpkin (`MobCombat::pumpkin`); M352 — an enderman's
/// carried block (`MobCombat::carried_block`); an adult
/// mooshroom's three mushrooms (`MushroomCowMushroomLayer`: none on a calf
/// or an invisible body), red, or brown on the brown colour's sheet. A
/// mooshroom caged in a spawner (a mounted draw) carries none here.
pub(super) fn collect_mob_blocks(
    draws: &[rewo_gpu::entities::EntityDraw<'_>],
    red_mushroom: Option<u32>,
    brown_mushroom: Option<u32>,
    carved_pumpkin: Option<u32>,
) -> Vec<super::block_entities::OwnedBlockEntityDraw> {
    use rewo_data::be_transform::IDENTITY;
    let brown =
        rewo_data::mob_variants::variant_id(rewo_data::mob_variants::mooshroom_texture(1, false));
    let mut out = Vec::new();
    let block = |pos, state, transform, light| super::block_entities::OwnedBlockEntityDraw {
        pos,
        model: rewo_data::block_entity_models::carried_block_model(state),
        transform,
        light,
        part_transforms: [IDENTITY; rewo_gpu::entities::MAX_PARTS],
        part_pivots: [[0.0; 3]; rewo_gpu::entities::MAX_PARTS],
        tint: [1.0; 3],
        fixed_display: false,
        glint: false,
        white: 0.0,
    };
    for d in draws {
        if d.body_hidden || d.mount.is_some() {
            continue;
        }
        // M353 — a snow golem's carved pumpkin (`SnowGolemHeadLayer`).
        if let (EntityModelKind::SnowGolem, true, Some(state)) =
            (d.kind, d.mob.pumpkin, carved_pumpkin)
        {
            let roll = rewo_gpu::entities::death_roll_for(d.kind, d.death_time);
            let net = (d.head_yaw - d.yaw + 180.0).rem_euclid(360.0) - 180.0;
            out.push(block(
                d.pos,
                state,
                snow_golem_pumpkin_transform(d.yaw, net, d.pitch, roll),
                d.light,
            ));
            continue;
        }
        // M352 — an enderman's carried block (`CarriedBlockLayer`).
        if let (EntityModelKind::Enderman, Some(state)) = (d.kind, d.mob.carried_block) {
            let roll = rewo_gpu::entities::death_roll_for(d.kind, d.death_time);
            out.push(block(
                d.pos,
                state,
                enderman_block_transform(d.yaw, roll),
                d.light,
            ));
            continue;
        }
        if d.kind != EntityModelKind::Mooshroom {
            continue;
        }
        let state = if Some(d.variant) == brown {
            brown_mushroom
        } else {
            red_mushroom
        };
        let Some(state) = state else {
            continue;
        };
        let roll = rewo_gpu::entities::death_roll_for(d.kind, d.death_time);
        let net = (d.head_yaw - d.yaw + 180.0).rem_euclid(360.0) - 180.0;
        for transform in mooshroom_mushroom_transforms(d.yaw, net, d.pitch, roll) {
            out.push(block(d.pos, state, transform, d.light));
        }
    }
    out
}

/// M260 — every block display's state this frame, for
/// [`rewo_data::assets::BakedAssets::add_state_model`] to build before the
/// draws are collected.
/// Since M261 also every minecart's carried block.
pub(super) fn block_display_states(
    session: &PlaySession,
    etypes: &EntityTypes,
    blocks: &rewo_data::blocks::Blocks,
) -> Vec<u32> {
    let ents = &session.world.entities;
    let mut out: Vec<u32> = ents
        .iter()
        .filter_map(|(id, e)| match etypes.name(e.type_id)? {
            "minecraft:block_display" => ents.display(id).and_then(|d| d.block),
            // M269 — a falling block or primed TNT of any state; the bake
            // makes only the gravity blocks and TNT.
            "minecraft:falling_block" => Some(e.spawn_data as u32),
            "minecraft:tnt" => ents.primed(id).block,
            // M352 — an enderman's carried block.
            "minecraft:enderman" => ents.carried_block(id),
            // M353 — a snow golem's carved pumpkin.
            "minecraft:snow_golem" if ents.has_pumpkin(id) => {
                blocks.default_state("minecraft:carved_pumpkin")
            }
            // M351 — a mooshroom's mushroom (`MushroomCow.Variant`, CLAMP).
            "minecraft:mooshroom" => blocks.default_state(if ents.variant(id).unwrap_or(0) >= 1 {
                "minecraft:brown_mushroom"
            } else {
                "minecraft:red_mushroom"
            }),
            name => minecart_contents(name, ents.minecart_display(id), blocks).map(|(s, _)| s),
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// M260 — every block display this frame, as `BlockDisplayRenderer` draws
/// it: the display pose (`display_pose`, the billboard's angles) then the
/// state's block model from the origin, no centring. The model is
/// `rewo:be/state/<id>`: the bake's for a falling block's state, else built
/// at run time from the terrain quads. Recorded: a block entity's state
/// (a chest) and a fluid draw nothing, no brightness override, no
/// interpolation.
pub(super) fn collect_block_displays(
    session: &PlaySession,
    etypes: &EntityTypes,
    lightmap: &LightmapState,
    alpha: f32,
) -> Vec<super::block_entities::OwnedBlockEntityDraw> {
    use rewo_data::be_transform::IDENTITY;
    let mut out = Vec::new();
    for (id, e) in session.world.entities.iter() {
        if etypes.name(e.type_id) != Some("minecraft:block_display") {
            continue;
        }
        let Some(d) = session.world.entities.display(id) else {
            continue;
        };
        let Some(state) = d.block else {
            continue;
        };
        let p = e.render_pos(alpha);
        let angles = billboard_angles(
            d.billboard,
            (e.yaw, e.pitch),
            (session.player.yaw, session.player.pitch),
        );
        out.push(super::block_entities::OwnedBlockEntityDraw {
            pos: [0.0; 3],
            model: rewo_data::block_entity_models::carried_block_model(state),
            transform: display_pose(p, angles, d, alpha),
            light: super::sky::display_light(
                &session.world,
                p[0],
                p[1],
                p[2],
                lightmap,
                d.brightness,
            ),
            part_transforms: [IDENTITY; rewo_gpu::entities::MAX_PARTS],
            part_pivots: [[0.0; 3]; rewo_gpu::entities::MAX_PARTS],
            tint: [1.0; 3],
            fixed_display: false,
            glint: false,
            white: 0.0,
        });
    }
    out
}

/// M241 — every fishing line this frame: for each bobber whose owner
/// (`spawn_data`, `FishingHook.getPlayerOwner`) is a known player, the line
/// from the hook to the hand that holds the rod. The local player's hand is
/// `FishingHookRenderer.getPlayerHandPos`'s first-person point: the camera's
/// near-plane point `(invert · 0.525, −0.1)` scaled by `960 / fov` and turned
/// by the swing; another player's is the body-yaw offset from its eye.
/// Recorded: a remote owner's swing and crouch offsets are not applied.
pub(super) fn collect_fishing_lines(
    session: &PlaySession,
    etypes: &EntityTypes,
    items: &rewo_data::items::Items,
    alpha: f32,
    fov: f32,
    (width, height): (f32, f32),
) -> Vec<rewo_gpu::leash::LeashVertex> {
    use rewo_world::entities::{HandItem, HumanoidArm, InteractionHand};
    let Some(bobber) = etypes.id_of("minecraft:fishing_bobber") else {
        return Vec::new();
    };
    let eye = player_eye(session);
    let eye = [eye[0] as f64, eye[1] as f64, eye[2] as f64];
    let world_per_px = 2.0 * (fov.to_radians() / 2.0).tan() / height.max(1.0);
    let width_px = (width / 1920.0 * 2.5).max(2.5);
    let is_rod =
        |item: Option<i32>| item.and_then(|i| items.name(i)) == Some("minecraft:fishing_rod");
    let mut out = Vec::new();
    for (_, e) in session.world.entities.iter() {
        if e.type_id != bobber {
            continue;
        }
        let owner = e.spawn_data;
        let hook = e.render_pos(alpha);
        let hand = if Some(owner) == session.player_id {
            // `getHoldingArm`: the main arm when the main hand holds the
            // rod, else the other one (the local main arm is the default
            // right).
            let invert = if is_rod(session.inventory.held().map(|s| s.item_id)) {
                1.0
            } else {
                -1.0
            };
            let swing = session.local_attack_anim(alpha);
            let swing = (swing.sqrt() * std::f32::consts::PI).sin();
            let bb = |v: [f32; 3]| {
                rewo_gpu::entities::Billboard {
                    cam_yaw: session.player.yaw,
                    cam_pitch: session.player.pitch,
                    scale: 1.0,
                    tint: None,
                }
                .place(v)
            };
            let (forwards, up, left) = (
                bb([0.0, 0.0, -1.0]),
                bb([0.0, 1.0, 0.0]),
                bb([-1.0, 0.0, 0.0]),
            );
            let aspect = width / height.max(1.0);
            let plane_h = (fov.to_radians() / 2.0).tan() * 0.05;
            let plane_w = plane_h * aspect;
            let (px, py) = (invert * 0.525, -0.1f32);
            let k = 960.0 / fov;
            let mut v: [f32; 3] = std::array::from_fn(|i| {
                (forwards[i] * 0.05 + up[i] * plane_h * py - left[i] * plane_w * px) * k
            });
            // `Vec3.yRot(swing * 0.5)` then `xRot(-swing * 0.7)`.
            let (sy, cy) = (swing * 0.5).sin_cos();
            v = [v[0] * cy + v[2] * sy, v[1], v[2] * cy - v[0] * sy];
            let (sx, cx) = (-swing * 0.7).sin_cos();
            v = [v[0], v[1] * cx + v[2] * sx, v[2] * cx - v[1] * sx];
            [
                eye[0] + v[0] as f64,
                eye[1] + v[1] as f64,
                eye[2] + v[2] as f64,
            ]
        } else {
            let Some(o) = session.world.entities.get(owner) else {
                continue;
            };
            let main_rod = matches!(
                session.world.entities.hand_item(owner, InteractionHand::MainHand),
                HandItem::Held(h) if is_rod(Some(h.item_id))
            );
            let main_right = session.world.entities.main_arm(owner) == HumanoidArm::Right;
            let invert = if main_rod == main_right { 1.0 } else { -1.0 };
            let p = o.render_pos(alpha);
            let yaw = o.yaw.to_radians();
            let (sin, cos) = (yaw.sin() as f64, yaw.cos() as f64);
            let right = invert * 0.35;
            let forward = 0.8;
            [
                p[0] - cos * right - sin * forward,
                p[1] + 1.62 - 0.45,
                p[2] - sin * right + cos * forward,
            ]
        };
        out.extend(rewo_gpu::leash::fishing_line(
            hook,
            hand,
            eye,
            world_per_px,
            width_px,
        ));
    }
    out
}

/// M250 — the projectiles drawn by a model under `YP(yRot - 90) · ZP(xRot +
/// extra)` rather than as a capsule: `ArrowRenderer` (the arrow and the
/// spectral arrow, extra 0) and `ThrownTridentRenderer` (extra 90).
pub(crate) fn projectile_model_of(name: &str) -> Option<(&'static str, f32)> {
    Some(match name {
        "minecraft:arrow" => ("rewo:be/arrow", 0.0),
        "minecraft:spectral_arrow" => ("rewo:be/spectral_arrow", 0.0),
        "minecraft:trident" => ("rewo:be/trident", 90.0),
        _ => return None,
    })
}

/// M258 — a small projectile's model and its renderer's pose, block units,
/// at `age` ticks with the entity's `yRot` / `xRot` in degrees:
///
/// - the wither skull: `scale(-1, -1, 1)` then `SkullModel`'s head turned
///   by `yRot` and `xRot` (`rotateZYX`: Y after X);
/// - the llama spit: `translate(0, 0.15, 0) · YP(yRot - 90) · ZP(xRot)`;
/// - the shulker bullet: `translate(0, 0.15, 0)` and its tumble
///   (`YP(sin(t·0.1)·180) · XP(cos(t·0.1)·180) · ZP(sin(t·0.15)·360)`)
///   then `scale(-0.5, -0.5, 0.5)` and the model's own `yRot` / `xRot`.
///
/// The bool is "full-bright" (`getBlockLightLevel` 15: the skull and the
/// bullet).
pub(crate) fn small_projectile_pose(
    name: &str,
    yaw: f32,
    pitch: f32,
    age: f32,
) -> Option<(&'static str, rewo_data::be_transform::Affine, bool)> {
    use rewo_data::be_transform::{mul, rot_x, rot_y, rot_z, scale, translation};
    use rewo_data::block_entity_models::{
        LLAMA_SPIT_MODEL, SHULKER_BULLET_MODEL, WITHER_SKULL_MODEL,
    };
    let turned = || mul(&rot_y(yaw), &rot_x(pitch.to_radians()));
    Some(match name {
        "minecraft:wither_skull" => (
            WITHER_SKULL_MODEL.0,
            mul(&scale(-1.0, -1.0, 1.0), &turned()),
            true,
        ),
        "minecraft:llama_spit" => (
            LLAMA_SPIT_MODEL.0,
            mul(
                &translation(0.0, 0.15, 0.0),
                &mul(&rot_y(yaw - 90.0), &rot_z(pitch.to_radians())),
            ),
            false,
        ),
        "minecraft:shulker_bullet" => {
            let tumble = mul(
                &rot_y((age * 0.1).sin() * 180.0),
                &mul(
                    &rot_x(((age * 0.1).cos() * 180.0).to_radians()),
                    &rot_z(((age * 0.15).sin() * 360.0).to_radians()),
                ),
            );
            (
                SHULKER_BULLET_MODEL.0,
                mul(
                    &mul(&translation(0.0, 0.15, 0.0), &tumble),
                    &mul(&scale(-0.5, -0.5, 0.5), &turned()),
                ),
                true,
            )
        }
        _ => return None,
    })
}

/// M258 — every wither skull, llama spit and shulker bullet this frame
/// (`small_projectile_pose`). Recorded: a dangerous (blue) wither skull
/// draws the ordinary texture, the shulker bullet's translucent outer shell
/// (`scale(1.5)`, `entityTranslucent` at alpha 0x26) is not drawn, the
/// bullet's age is the world clock offset by the entity id, and the
/// rotation is the latest packet's.
pub(super) fn collect_small_projectiles(
    session: &PlaySession,
    etypes: &EntityTypes,
    lightmap: &LightmapState,
    alpha: f32,
) -> Vec<super::block_entities::OwnedBlockEntityDraw> {
    use rewo_data::be_transform::IDENTITY;
    let mut out = Vec::new();
    for (id, e) in session.world.entities.iter() {
        let age = session.game_time() as f32 + alpha + (id & 0xff) as f32 * 7.0;
        let Some((model, transform, bright)) = etypes.name(e.type_id).and_then(|n| {
            small_projectile_pose(n, e.render_yaw(alpha), e.render_pitch(alpha), age)
        }) else {
            continue;
        };
        let p = e.render_pos(alpha);
        let boost = if bright { 15 } else { 0 };
        out.push(super::block_entities::OwnedBlockEntityDraw {
            pos: [p[0] as f32, p[1] as f32, p[2] as f32],
            model: model.to_string(),
            transform,
            light: super::sky::entity_light_boosted(
                &session.world,
                p[0],
                p[1],
                p[2],
                lightmap,
                boost,
            ),
            part_transforms: [IDENTITY; rewo_gpu::entities::MAX_PARTS],
            part_pivots: [[0.0; 3]; rewo_gpu::entities::MAX_PARTS],
            tint: [1.0; 3],
            fixed_display: false,
            glint: false,
            white: 0.0,
        });
    }
    out
}

/// M259 — every evoker fangs this frame whose bite has started, as
/// `EvokerFangsRenderer` draws it: `YP(90 - yRot) · scale(-1, -1, 1) ·
/// translate(0, -1.501, 0)`, the root's lift and scale, then the model with
/// the base rising and the jaws snapping (`evoker_fangs_parts`). Nothing is
/// drawn before the bite event (progress 0), as in vanilla.
pub(super) fn collect_evoker_fangs(
    session: &PlaySession,
    etypes: &EntityTypes,
    lightmap: &LightmapState,
    alpha: f32,
) -> Vec<super::block_entities::OwnedBlockEntityDraw> {
    use rewo_data::be_transform::{
        evoker_fangs_parts, evoker_fangs_progress, mul, rot_y, scale, translation, IDENTITY,
    };
    use rewo_data::block_entity_models::{
        EVOKER_FANGS_BASE_PIVOT, EVOKER_FANGS_LOWER_PIVOT, EVOKER_FANGS_MODEL,
        EVOKER_FANGS_UPPER_PIVOT,
    };
    use rewo_world::entities::EntityEvent;
    let mut out = Vec::new();
    for (id, e) in session.world.entities.iter() {
        if etypes.name(e.type_id) != Some("minecraft:evoker_fangs") {
            continue;
        }
        let bite = evoker_fangs_progress(
            session
                .world
                .entities
                .event_start(id, EntityEvent::EvokerFangsBite),
            session.ticks as i64,
            alpha,
        );
        if bite == 0.0 {
            continue;
        }
        let (root, [base, upper, lower]) = evoker_fangs_parts(bite);
        let p = e.render_pos(alpha);
        out.push(super::block_entities::OwnedBlockEntityDraw {
            pos: [p[0] as f32, p[1] as f32, p[2] as f32],
            model: EVOKER_FANGS_MODEL.0.to_string(),
            transform: mul(
                &mul(&rot_y(90.0 - e.yaw), &scale(-1.0, -1.0, 1.0)),
                &mul(&translation(0.0, -1.501, 0.0), &root),
            ),
            light: entity_light(&session.world, p[0], p[1], p[2], lightmap),
            part_transforms: [IDENTITY, base, upper, lower, IDENTITY, IDENTITY, IDENTITY],
            part_pivots: [
                [0.0; 3],
                EVOKER_FANGS_BASE_PIVOT,
                EVOKER_FANGS_UPPER_PIVOT,
                EVOKER_FANGS_LOWER_PIVOT,
                [0.0; 3],
                [0.0; 3],
                [0.0; 3],
            ],
            tint: [1.0; 3],
            fixed_display: false,
            glint: false,
            white: 0.0,
        });
    }
    out
}

/// M257 — every end crystal this frame, as `EndCrystalRenderer` draws it:
/// `T(pos) · scale(2) · translate(0, -0.5, 0)` then the model, no living
/// flip, its glass spinning and bobbing (`end_crystal_parts`); the base
/// unless `DATA_SHOW_BOTTOM` is false. Recorded: the age is the world clock
/// offset by the entity id (`EndCrystal.time` starts at a random value on
/// the server and is never synced), and no beam.
pub(super) fn collect_end_crystals(
    session: &PlaySession,
    etypes: &EntityTypes,
    lightmap: &LightmapState,
    alpha: f32,
) -> Vec<super::block_entities::OwnedBlockEntityDraw> {
    use rewo_data::be_transform::{mul, scale, translation, IDENTITY};
    use rewo_data::block_entity_models::{END_CRYSTAL_GLASS_PIVOT, END_CRYSTAL_MODELS};
    let mut out = Vec::new();
    for (id, e) in session.world.entities.iter() {
        if etypes.name(e.type_id) != Some("minecraft:end_crystal") {
            continue;
        }
        let shows_bottom = session.world.entities.variant(id) != Some(0);
        let model = END_CRYSTAL_MODELS[usize::from(!shows_bottom)].0;
        let age = session.game_time() as f32 + alpha + (id & 0xff) as f32 * 7.0;
        let [outer, inner, cube] = rewo_data::be_transform::end_crystal_parts(age);
        let p = e.render_pos(alpha);
        out.push(super::block_entities::OwnedBlockEntityDraw {
            pos: [p[0] as f32, p[1] as f32, p[2] as f32],
            model: model.to_string(),
            transform: mul(&scale(2.0, 2.0, 2.0), &translation(0.0, -0.5, 0.0)),
            light: entity_light(&session.world, p[0], p[1], p[2], lightmap),
            part_transforms: [IDENTITY, outer, inner, cube, IDENTITY, IDENTITY, IDENTITY],
            part_pivots: [
                [0.0; 3],
                END_CRYSTAL_GLASS_PIVOT,
                END_CRYSTAL_GLASS_PIVOT,
                END_CRYSTAL_GLASS_PIVOT,
                [0.0; 3],
                [0.0; 3],
                [0.0; 3],
            ],
            tint: [1.0; 3],
            fixed_display: false,
            glint: false,
            white: 0.0,
        });
    }
    out
}

/// M250 — every arrow and thrown trident this frame, as block-entity-style
/// draws: the model at the entity's position under the renderer's own
/// rotation, lit at the entity. Recorded: the rotation is the latest packet's
/// (no per-frame yaw/pitch interpolation), a stuck arrow's shake is not
/// kept, a tipped arrow uses the plain texture, and a foil trident has no
/// glint.
pub(super) fn collect_projectiles(
    session: &PlaySession,
    etypes: &EntityTypes,
    lightmap: &LightmapState,
    alpha: f32,
) -> Vec<super::block_entities::OwnedBlockEntityDraw> {
    use rewo_data::be_transform::{mul, rot_y, rot_z, IDENTITY};
    let mut out = Vec::new();
    for (_, e) in session.world.entities.iter() {
        let Some((model, extra)) = etypes.name(e.type_id).and_then(projectile_model_of) else {
            continue;
        };
        let p = e.render_pos(alpha);
        out.push(super::block_entities::OwnedBlockEntityDraw {
            pos: [p[0] as f32, p[1] as f32, p[2] as f32],
            model: model.to_string(),
            transform: mul(
                &rot_y(e.render_yaw(alpha) - 90.0),
                &rot_z((e.render_pitch(alpha) + extra).to_radians()),
            ),
            light: entity_light(&session.world, p[0], p[1], p[2], lightmap),
            part_transforms: [IDENTITY; rewo_gpu::entities::MAX_PARTS],
            part_pivots: [[0.0; 3]; rewo_gpu::entities::MAX_PARTS],
            tint: [1.0; 3],
            fixed_display: false,
            glint: false,
            white: 0.0,
        });
    }
    out
}

/// M253 — the entities drawn as a block model: a falling block
/// (`FallingBlockRenderer`: its block state, from the spawn data, under
/// `translate(-0.5, 0, -0.5)`) and primed TNT (`TntRenderer`'s chain
/// `translate(0, 0.5, 0) · YP(-90) · translate(-0.5, -0.5, 0.5) · YP(90)`,
/// which is the same `translate(-0.5, 0, -0.5)`, with the TNT block).
/// M269 drew the swell and any primed block; M277 its white flash.
pub(super) fn collect_carried_blocks(
    session: &PlaySession,
    etypes: &EntityTypes,
    lightmap: &LightmapState,
    alpha: f32,
    tnt_state: Option<u32>,
) -> Vec<super::block_entities::OwnedBlockEntityDraw> {
    use rewo_data::be_transform::{translation, IDENTITY};
    let mut out = Vec::new();
    for (id, e) in session.world.entities.iter() {
        let (state, transform, white) = match etypes.name(e.type_id) {
            Some("minecraft:falling_block") => {
                (Some(e.spawn_data as u32), translation(-0.5, 0.0, -0.5), 0.0)
            }
            Some("minecraft:tnt") => {
                // M269 — the primed block (TNT unless the server set one)
                // and the swell of the last ten fuse ticks:
                // `fuseRemainingInTicks = getFuse() - partial + 1`.
                let primed = session.world.entities.primed(id);
                let fuse = primed.fuse.map_or(80.0, |f| f as f32 - alpha + 1.0);
                // M277 — and its white flash, `isLit(fuse)` at full white.
                let white = if tnt_is_lit(fuse) { 1.0 } else { 0.0 };
                (primed.block.or(tnt_state), tnt_transform(fuse), white)
            }
            _ => continue,
        };
        let Some(state) = state else {
            continue;
        };
        let p = e.render_pos(alpha);
        out.push(super::block_entities::OwnedBlockEntityDraw {
            pos: [p[0] as f32, p[1] as f32, p[2] as f32],
            model: rewo_data::block_entity_models::carried_block_model(state),
            transform,
            light: entity_light(&session.world, p[0], p[1] + 0.5, p[2], lightmap),
            part_transforms: [IDENTITY; rewo_gpu::entities::MAX_PARTS],
            part_pivots: [[0.0; 3]; rewo_gpu::entities::MAX_PARTS],
            tint: [1.0; 3],
            fixed_display: false,
            glint: false,
            white,
        });
    }
    out
}

/// M281 — the entity types `EntityTypes` builds `.fireImmune()`: they never
/// burn, so never show flames.
pub(crate) const FIRE_IMMUNE: &[&str] = &[
    "area_effect_cloud",
    "blaze",
    "ender_dragon",
    "end_crystal",
    "ghast",
    "magma_cube",
    "shulker",
    "strider",
    "tnt",
    "vex",
    "warden",
    "wither",
    "wither_skeleton",
    "zoglin",
    "zombified_piglin",
];

/// M281 — `Entity.displayFireAnimation` (`isOnFire() && !isSpectator()`;
/// the client's `isOnFire` is `!fireImmune() && getSharedFlag(0)`), as the
/// two flame frames to show at animation time `now` (seconds, a frame a
/// tick); `None` for an entity that does not burn.
pub(crate) fn fire_draw(
    session: &PlaySession,
    id: i32,
    entity: &str,
    now: f32,
) -> Option<[usize; 2]> {
    if !session.world.entities.shared_flag(id, 0)
        || FIRE_IMMUNE.contains(&entity.strip_prefix("minecraft:").unwrap_or(entity))
    {
        return None;
    }
    let tick = (now * 20.0).max(0.0) as u32;
    Some([0, 1].map(|s| rewo_data::block_entity_models::fire_frame(s, tick)))
}

/// M278 — `Minecraft.shouldEntityAppearGlowing` (`isCurrentlyGlowing`, the
/// client's reading of shared flag 6, `FLAG_GLOWING`) and
/// `Entity.getTeamColor()`: the team's `ChatFormatting` colour, white with no
/// team or a colourless one — as gamma rgb for the outline pass.
pub(crate) fn glow_colour(session: &PlaySession, id: i32) -> Option<[f32; 3]> {
    if !session.world.entities.shared_flag(id, 6) {
        return None;
    }
    let rgb = session
        .scoreboard_name_of(id)
        .and_then(|n| {
            let team = session.team_of_name(&n)?;
            session
                .scoreboard
                .teams
                .team(team)?
                .parameters
                .as_ref()?
                .color
        })
        .and_then(|c| rewo_world::chat_style::NAMED_COLORS.get(c as usize))
        .map_or(0xFF_FFFF, |(_, v)| *v);
    Some([
        ((rgb >> 16) & 255) as f32 / 255.0,
        ((rgb >> 8) & 255) as f32 / 255.0,
        (rgb & 255) as f32 / 255.0,
    ])
}

/// M277 — `CreeperRenderer.scale`: `wobble = 1 + sin(g · 100) · g · 0.01`,
/// then `g` clamped and raised to the fourth, `s = (1 + g · 0.4) · wobble`,
/// `hs = (1 + g · 0.1) / wobble` — as `[s, hs]` for `EntityDraw::stretch`.
pub(crate) fn creeper_stretch(swelling: f32) -> [f32; 2] {
    let wobble = 1.0 + (swelling * 100.0).sin() * swelling * 0.01;
    let g = swelling.clamp(0.0, 1.0).powi(4);
    [(1.0 + g * 0.4) * wobble, (1.0 + g * 0.1) / wobble]
}

/// M277 — `CreeperRenderer.getWhiteOverlayProgress`: white on every other
/// tenth of the swell, `clamp(step, 0.5, 1)`.
pub(crate) fn creeper_white_overlay(step: f32) -> f32 {
    if (step * 10.0) as i32 % 2 == 0 {
        0.0
    } else {
        step.clamp(0.5, 1.0)
    }
}

/// M277 — `TntRenderer.isLit(fuse)`: lit TNT flashes white for five ticks
/// in every ten, its whole fuse long.
pub(crate) fn tnt_is_lit(fuse: f32) -> bool {
    fuse >= 0.0 && (fuse / 5.0) as i32 % 2 == 0
}

/// M269 — `TntRenderer.getSwellAmount(fuse)`: nothing until the last ten
/// ticks, then `g⁴ · 0.3` with `g = clamp(1 - fuse / 10)`.
pub(crate) fn tnt_swell(fuse: f32) -> f32 {
    let g = (1.0 - fuse / 10.0).clamp(0.0, 1.0);
    let g = g * g;
    g * g * 0.3
}

/// M269 — `TntRenderer.submit`'s chain, block units about the entity:
/// `translate(0, 0.5, 0)`, the swell's scale, then `YP(-90) · translate(-0.5,
/// -0.5, 0.5) · YP(90)`, which is `translate(-0.5, -0.5, -0.5)`. At rest it
/// is M253's `translate(-0.5, 0, -0.5)`. Recorded: the white flash
/// (`isLit`) is not drawn.
pub(crate) fn tnt_transform(fuse: f32) -> rewo_data::be_transform::Affine {
    use rewo_data::be_transform::{mul, scale, translation};
    let s = 1.0 + if fuse < 10.0 { tnt_swell(fuse) } else { 0.0 };
    mul(
        &mul(&translation(0.0, 0.5, 0.0), &scale(s, s, s)),
        &translation(-0.5, -0.5, -0.5),
    )
}

/// M254 — every painting this frame, as `PaintingRenderer` draws it: the
/// variant's model (`painting_variant` registry, by the entity's raw holder
/// id) turned by `YP(180 - direction.get2DDataValue() · 90)`, the direction
/// from the spawn data (`get3DDataValue`), at the entity's position.
/// Recorded: one light for the whole painting (vanilla samples one per
/// block).
pub(super) fn collect_paintings(
    session: &PlaySession,
    etypes: &EntityTypes,
    lightmap: &LightmapState,
    alpha: f32,
) -> Vec<super::block_entities::OwnedBlockEntityDraw> {
    use rewo_data::be_transform::{rot_y, IDENTITY};
    let mut out = Vec::new();
    for (id, e) in session.world.entities.iter() {
        if etypes.name(e.type_id) != Some("minecraft:painting") {
            continue;
        }
        let Some(asset) = session
            .world
            .entities
            .variant(id)
            .and_then(|v| session.painting_variants.get(usize::try_from(v).ok()?))
            .and_then(|v| v.asset_id.as_deref())
        else {
            continue;
        };
        // `Direction.get3DDataValue` → `get2DDataValue`: north 2 → 2,
        // south 3 → 0, west 4 → 1, east 5 → 3.
        let d2 = match e.spawn_data {
            2 => 2.0,
            3 => 0.0,
            4 => 1.0,
            5 => 3.0,
            _ => continue,
        };
        let p = e.render_pos(alpha);
        out.push(super::block_entities::OwnedBlockEntityDraw {
            pos: [p[0] as f32, p[1] as f32, p[2] as f32],
            model: rewo_data::block_entity_models::painting_model(asset),
            transform: rot_y(180.0 - d2 * 90.0),
            light: entity_light(&session.world, p[0], p[1], p[2], lightmap),
            part_transforms: [IDENTITY; rewo_gpu::entities::MAX_PARTS],
            part_pivots: [[0.0; 3]; rewo_gpu::entities::MAX_PARTS],
            tint: [1.0; 3],
            fixed_display: false,
            glint: false,
            white: 0.0,
        });
    }
    out
}

/// M255 — a quaternion `[x, y, z, w]` as an affine rotation.
fn quat_affine(q: [f32; 4]) -> rewo_data::be_transform::Affine {
    let [x, y, z, w] = q;
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
            0.0,
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
            0.0,
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
            0.0,
        ],
    ]
}

/// M255 — a text display's font-px → world pose: `DisplayRenderer.submit`'s
/// `T(pos) · orientation · Transformation(T · L · S · R)` then
/// `TextDisplayRenderer.submitInner`'s `YP(π) · scale(-0.025)`. The
/// orientation is `rotationYXZ(-yaw, pitch, 0)` with the billboard mode
/// choosing entity or camera angles (`calculateOrientation`; the camera's
/// yaw less 180, its pitch negated).
pub(crate) fn text_display_transform(
    pos: [f64; 3],
    entity: (f32, f32),
    camera: (f32, f32),
    d: &rewo_world::entities::DisplayState,
    alpha: f32,
) -> rewo_data::be_transform::Affine {
    use rewo_data::be_transform::{mul, rot_y_rad, scale};
    let (yaw, pitch) = billboard_angles(d.billboard, entity, camera);
    let text = mul(
        &rot_y_rad(std::f32::consts::PI),
        &scale(-0.025, -0.025, -0.025),
    );
    mul(&display_pose(pos, (yaw, pitch), d, alpha), &text)
}

/// M255/M256 — `DisplayRenderer.submit`'s pose shared by every display:
/// `T(pos) · rotationYXZ(-yaw, pitch, 0) · Transformation(T · L · S · R)`,
/// the angles already chosen by the billboard mode. Since M265 the
/// transformation is the interpolated one at `alpha`
/// (`DisplayState::render_transformation`).
fn display_pose(
    pos: [f64; 3],
    (yaw, pitch): (f32, f32),
    d: &rewo_world::entities::DisplayState,
    alpha: f32,
) -> rewo_data::be_transform::Affine {
    use rewo_data::be_transform::{mul, rot_x, rot_y_rad, scale, translation};
    let orient = mul(&rot_y_rad(-yaw.to_radians()), &rot_x(pitch.to_radians()));
    let t = d.render_transformation(alpha);
    let [tx, ty, tz] = t.translation;
    let [sx, sy, sz] = t.scale;
    let tf = mul(
        &mul(&translation(tx, ty, tz), &quat_affine(t.left_rotation)),
        &mul(&scale(sx, sy, sz), &quat_affine(t.right_rotation)),
    );
    let at = translation(pos[0] as f32, pos[1] as f32, pos[2] as f32);
    mul(&mul(&at, &orient), &tf)
}

/// The angles a billboard mode picks (`calculateOrientation`): 0 fixed, 1
/// vertical, 2 horizontal, 3 centre.
fn billboard_angles(billboard: u8, entity: (f32, f32), camera: (f32, f32)) -> (f32, f32) {
    match billboard {
        1 => (camera.0 - 180.0, entity.1),
        2 => (entity.0, -camera.1),
        3 => (camera.0 - 180.0, -camera.1),
        _ => entity,
    }
}

/// M256 — `ItemTransform.apply` then `ItemStackRenderState`'s
/// `translate(-0.5)`, as an affine in block units: `T · rotationXYZ · S ·
/// T(-0.5)`, a left-hand context negating x translation and y/z rotation.
fn item_context_affine(
    t: &rewo_data::item_models::DisplayTransform,
    left: bool,
) -> rewo_data::be_transform::Affine {
    use rewo_data::be_transform::{mul, rot_xyz, scale, translation};
    let m = if left { -1.0 } else { 1.0 };
    let [rx, ry, rz] = t.rotation;
    mul(
        &mul(
            &translation(t.translation[0] * m, t.translation[1], t.translation[2]),
            &rot_xyz(
                rx.to_radians(),
                (ry * m).to_radians(),
                (rz * m).to_radians(),
            ),
        ),
        &mul(
            &scale(t.scale[0], t.scale[1], t.scale[2]),
            &translation(-0.5, -0.5, -0.5),
        ),
    )
}

/// M256 — an item display's model-block-unit → world pose: the display
/// pose, `YP(π)`, then the item's display transform for its context (the
/// third- and first-person hands, gui, ground, fixed; `none`, `head` and
/// `on_shelf` use the identity, the last two because Rewo bakes no such
/// transform) and `translate(-0.5)`.
pub(crate) fn item_display_transform(
    pos: [f64; 3],
    entity: (f32, f32),
    camera: (f32, f32),
    d: &rewo_world::entities::DisplayState,
    model: &rewo_data::held_items::HeldItemModel,
    alpha: f32,
) -> rewo_data::be_transform::Affine {
    use rewo_data::be_transform::{mul, rot_y_rad};
    let identity = rewo_data::item_models::DisplayTransform::IDENTITY;
    let (t, left) = match d.item_context {
        1 => (&model.left, true),
        2 => (&model.right, false),
        3 => (&model.first_left, true),
        4 => (&model.first_right, false),
        6 => (&model.gui, false),
        7 => (&model.ground, false),
        8 => (&model.fixed, false),
        _ => (&identity, false),
    };
    let angles = billboard_angles(d.billboard, entity, camera);
    mul(
        &display_pose(pos, angles, d, alpha),
        &mul(
            &rot_y_rad(std::f32::consts::PI),
            &item_context_affine(t, left),
        ),
    )
}

/// M256 — every item display this frame, as `ItemDisplayRenderer` draws it:
/// [`item_display_transform`]'s pose, with its foil. Recorded: no
/// brightness override, no interpolation.
pub(super) fn collect_item_displays(
    session: &PlaySession,
    etypes: &EntityTypes,
    item_names: &rewo_data::items::Items,
    held: Option<&rewo_data::held_items::HeldItems>,
    lightmap: &LightmapState,
    alpha: f32,
) -> Vec<super::block_entities::OwnedBlockEntityDraw> {
    use rewo_data::be_transform::IDENTITY;
    let mut out = Vec::new();
    let Some(held) = held else {
        return out;
    };
    for (id, e) in session.world.entities.iter() {
        if etypes.name(e.type_id) != Some("minecraft:item_display") {
            continue;
        }
        let Some(d) = session.world.entities.display(id) else {
            continue;
        };
        let Some((item, foil)) = d.item else {
            continue;
        };
        let Some(name) = item_names.name(item) else {
            continue;
        };
        let Some(model) = held.any(name) else {
            continue;
        };
        let p = e.render_pos(alpha);
        let pose = item_display_transform(
            p,
            (e.yaw, e.pitch),
            (session.player.yaw, session.player.pitch),
            d,
            model,
            alpha,
        );
        out.push(super::block_entities::OwnedBlockEntityDraw {
            pos: [0.0; 3],
            model: name.to_string(),
            transform: pose,
            light: super::sky::display_light(
                &session.world,
                p[0],
                p[1],
                p[2],
                lightmap,
                d.brightness,
            ),
            part_transforms: [IDENTITY; rewo_gpu::entities::MAX_PARTS],
            part_pivots: [[0.0; 3]; rewo_gpu::entities::MAX_PARTS],
            tint: [1.0; 3],
            fixed_display: false,
            glint: foil,
            white: 0.0,
        });
    }
    out
}

/// M268 — the depth, in font px along the text's own axis, a text display's
/// drop shadow sits at: behind the glyphs (0), in front of the plate
/// (-0.01). Vanilla separates the three by submission order and polygon
/// offset; Rewo's world text is depth-tested, so by depth.
pub(crate) const TEXT_DISPLAY_SHADOW_DEPTH: f32 = -0.005;

/// M262 — a text display's background as linear RGBA, or `None` when it
/// draws none (`TextDisplayRenderer.submitInner`): the default-background
/// flag (4) takes the options' background opacity (vanilla's default,
/// 0.25, as `(int)(0.25 · 255) << 24`: black at 63/255), else the synced
/// ARGB colour (`0x40000000` unless the server set another); a colour of 0
/// draws no plate.
pub(crate) fn text_display_background(style: u8, argb: i32) -> Option<[f32; 4]> {
    let argb = if style & 4 != 0 {
        ((0.25f32 * 255.0) as i32) << 24
    } else {
        argb
    };
    if argb == 0 {
        return None;
    }
    let [a, r, g, b] = (argb as u32).to_be_bytes();
    let [r, g, b] = linear_rgb(r, g, b);
    Some([r, g, b, f32::from(a) / 255.0])
}

/// M255 — `TextDisplay.cacheDisplay`'s lines: the text split at `\n` and
/// wrapped to `line_width` font px (at the last space that fits, else
/// mid-word), each line as runs `(text, sRGB colour)`, with its width.
pub(crate) fn text_display_lines(
    text: &[(String, [f32; 3])],
    line_width: i32,
    advance: &[u8; 256],
) -> Vec<(Vec<(String, [f32; 3])>, f32)> {
    let adv = |c: char| -> f32 {
        if c.is_ascii() {
            advance[c as usize] as f32
        } else {
            0.0
        }
    };
    let limit = line_width.max(1) as f32;
    let mut lines: Vec<Vec<(char, [f32; 3])>> = vec![Vec::new()];
    for (t, colour) in text {
        for c in t.chars() {
            if c == '\n' {
                lines.push(Vec::new());
            } else {
                lines.last_mut().unwrap().push((c, *colour));
            }
        }
    }
    let mut wrapped: Vec<Vec<(char, [f32; 3])>> = Vec::new();
    for line in lines {
        let mut cur: Vec<(char, [f32; 3])> = Vec::new();
        let mut w = 0.0f32;
        for ch in line {
            if w + adv(ch.0) > limit && !cur.is_empty() {
                match cur.iter().rposition(|(c, _)| *c == ' ') {
                    Some(sp) => {
                        let rest: Vec<_> = cur.split_off(sp + 1);
                        cur.pop();
                        wrapped.push(std::mem::take(&mut cur));
                        cur = rest;
                    }
                    None => wrapped.push(std::mem::take(&mut cur)),
                }
                w = cur.iter().map(|(c, _)| adv(*c)).sum();
            }
            w += adv(ch.0);
            cur.push(ch);
        }
        wrapped.push(cur);
    }
    wrapped
        .into_iter()
        .map(|chars| {
            let width = chars.iter().map(|(c, _)| adv(*c)).sum();
            let mut runs: Vec<(String, [f32; 3])> = Vec::new();
            for (c, colour) in chars {
                match runs.last_mut() {
                    Some((s, k)) if *k == colour => s.push(c),
                    _ => runs.push((c.to_string(), colour)),
                }
            }
            (runs, width)
        })
        .collect()
}

/// M255 — every text display's text this frame, as world-text lines on the
/// sign path: `TextDisplayRenderer.submitInner`'s layout (`1 - width/2`
/// across, `-height` down, ten px a line, aligned by the style flags), one
/// line per coloured run. Recorded: no background quad (the entity pass has
/// no blended solid quad), no see-through, opacity not applied (the shadow since M268),
/// no brightness override, no interpolation.
pub(super) fn collect_text_displays(
    session: &PlaySession,
    etypes: &EntityTypes,
    lightmap: &LightmapState,
    alpha: f32,
    advance: &[u8; 256],
) -> Vec<super::block_entities::OwnedSignLine> {
    let mut out = Vec::new();
    for (id, e) in session.world.entities.iter() {
        if etypes.name(e.type_id) != Some("minecraft:text_display") {
            continue;
        }
        let Some(d) = session.world.entities.display(id) else {
            continue;
        };
        if d.text.is_empty() {
            continue;
        }
        let p = e.render_pos(alpha);
        let transform = text_display_transform(
            p,
            (e.yaw, e.pitch),
            (session.player.yaw, session.player.pitch),
            d,
            alpha,
        );
        let lines = text_display_lines(&d.text, d.line_width, advance);
        let width = lines.iter().map(|(_, w)| *w).fold(0.0f32, f32::max);
        let height = lines.len() as f32 * 10.0 - 1.0;
        let light =
            super::sky::display_light(&session.world, p[0], p[1], p[2], lightmap, d.brightness);
        // M262 — the background plate, `(-1, -1)..(width, height)` from the
        // block's top-left corner (`1 - width/2`, `-height`), in front of
        // nothing and behind the glyphs.
        if let Some(rgba) = text_display_background(d.style, d.background) {
            out.push(super::block_entities::OwnedSignLine {
                transform,
                text: String::new(),
                x: 0.0,
                y: 0.0,
                z: 0.0,
                color: [1.0; 3],
                light,
                backdrop: Some(([-width / 2.0, -height - 1.0, 1.0 + width / 2.0, 0.0], rgba)),
            });
        }
        for (i, (runs, lw)) in lines.into_iter().enumerate() {
            let offset = if d.style & 8 != 0 {
                0.0
            } else if d.style & 16 != 0 {
                width - lw
            } else {
                width / 2.0 - lw / 2.0
            };
            let mut pen = 1.0 - width / 2.0 + offset;
            for (text, c) in runs {
                let w: f32 = text.bytes().map(|b| advance[b as usize] as f32).sum();
                let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                let y = -height + i as f32 * 10.0;
                if d.style & 1 != 0 {
                    // M268 — the drop shadow (style flag 1): `Font`'s
                    // `ARGB.scaleRGB(color, 0.25)` one font px right and down,
                    // a hair behind the glyphs and in front of the plate.
                    let shade = |v: f32| (f32::from(to8(v)) * 0.25) as u8;
                    out.push(super::block_entities::OwnedSignLine {
                        transform,
                        text: text.clone(),
                        x: pen + 1.0,
                        y: y + 1.0,
                        z: TEXT_DISPLAY_SHADOW_DEPTH,
                        color: linear_rgb(shade(c[0]), shade(c[1]), shade(c[2])),
                        light,
                        backdrop: None,
                    });
                }
                out.push(super::block_entities::OwnedSignLine {
                    transform,
                    text,
                    x: pen,
                    y,
                    z: 0.0,
                    color: linear_rgb(to8(c[0]), to8(c[1]), to8(c[2])),
                    light,
                    backdrop: None,
                });
                pen += w;
            }
        }
    }
    out
}

/// M240 — the entities drawn as a camera-facing item: `(default item or
/// sprite model, scale, fullbright)`, from `EntityRenderers`' registrations
/// of `ThrownItemRenderer` (eye of ender, fireballs fullbright; the fireball
/// at 3, the small one at 0.75), `FireworkEntityRenderer` and
/// `FishingHookRenderer` (its sprite at 0.5). Recorded: a rocket shot at an
/// angle (a crossbow's) is drawn facing the camera too, without vanilla's
/// extra turn.
pub(crate) fn thrown_item_of(name: &str) -> Option<(&'static str, f32, bool)> {
    Some(match name {
        "minecraft:snowball" => ("minecraft:snowball", 1.0, false),
        "minecraft:egg" => ("minecraft:egg", 1.0, false),
        "minecraft:ender_pearl" => ("minecraft:ender_pearl", 1.0, false),
        "minecraft:experience_bottle" => ("minecraft:experience_bottle", 1.0, false),
        "minecraft:splash_potion" => ("minecraft:splash_potion", 1.0, false),
        "minecraft:lingering_potion" => ("minecraft:lingering_potion", 1.0, false),
        "minecraft:eye_of_ender" => ("minecraft:ender_eye", 1.0, true),
        "minecraft:fireball" => ("minecraft:fire_charge", 3.0, true),
        "minecraft:small_fireball" => ("minecraft:fire_charge", 0.75, true),
        "minecraft:firework_rocket" => ("minecraft:firework_rocket", 1.0, false),
        "minecraft:fishing_bobber" => ("rewo:be/fishing_hook", 0.5, false),
        // M258 — `DragonFireballRenderer`'s camera-facing sprite at scale 2,
        // full-bright.
        "minecraft:dragon_fireball" => (
            rewo_data::block_entity_models::DRAGON_FIREBALL_MODEL.0,
            2.0,
            true,
        ),
        _ => return None,
    })
}

/// The item held in each arm (M22), as registry names — `[right, left]`.
///
/// `ArmedEntityRenderState` carries the stacks per *arm*, and the hand-to-arm
/// mapping is `getMainArm()`, which
/// [`rewo_world::entities::EntityTable::item_by_arm`] already implements. An
/// `Unknown` hand yields `None` for the same reason its swing is suppressed:
/// the client cannot know what it is holding, and drawing a guess is worse
/// than drawing nothing.
pub(super) fn resolve_held_items<'a>(
    entities: &rewo_world::entities::EntityTable,
    id: i32,
    items: &'a rewo_data::items::Items,
    // M290 — a held player head takes its profile's skin.
    head_skins: Option<&'a HeadSkins>,
) -> [Option<&'a str>; 2] {
    use rewo_world::entities::{HumanoidArm, InteractionHand};
    let mut out: [Option<&str>; 2] = [None, None];
    let use_state = entities.use_state(id);
    for (i, arm) in [(0usize, HumanoidArm::Right), (1usize, HumanoidArm::Left)] {
        if let Some(held) = entities.item_by_arm(id, arm).held() {
            // M234 — which hand this arm holds, for the use variant.
            let hand = if entities.main_arm(id) == arm {
                InteractionHand::MainHand
            } else {
                InteractionHand::OffHand
            };
            out[i] = items
                .name(held.item_id)
                .map(|n| head_skins.map_or(n, |h| h.item_model(n, entities.hand_profile(id, hand))))
                .map(|n| use_model(n, use_state, hand, held.charged));
        }
    }
    out
}

/// Snapshot every tracked entity into this frame's draw list. `alpha` is
/// the partial-tick blend (0..1). Players get the rose capsule + nametag;
/// everything else gets mauve, sized by the type table. `now` is the
/// render clock in seconds — the gesture rigs' time base.
/// `ItemEntity.bobOffs` — `this.random.nextFloat() * (float)Math.PI * 2`.
///
/// Vanilla rolls this in the entity's constructor from a non-deterministic
/// source and never transmits it, so there is no server value to reproduce and
/// two vanilla clients watching the same dropped stack disagree about its bob
/// phase. Deriving it from the entity id is therefore *as vanilla as vanilla*
/// — it is a valid roll — with the added property of being stable across
/// frames and reproducible in a gate.
///
/// The hash is the 64-bit splitmix64 finalizer, used only to decorrelate
/// consecutive entity ids; nothing depends on its exact value.
/// `ItemStack.hasFoil()` for what an entity holds in one hand (M45).
///
/// The flag comes off the wire with the equipment, because it lives only in
/// the component patch — there is nothing about the item *id* that says
/// whether a particular stack is enchanted.
/// The armour atlas key each slot wears, head first (M46).
///
/// Two lookups, and both are **prototype** data rather than wire data: the
/// item's `Equippable.assetId()` comes from the generated item table, and the
/// asset's layer names from the jar. The wire sends only an item id.
///
/// The **leggings take the inner sheet** and the other three the outer one —
/// `usesInnerModel` is `slot == LEGS`, which is the whole reason two layer
/// types exist.
/// What each armour slot draws, head first — the atlas key and tint of every
/// sub-layer that survives `getColorForLayer` (M46, dyed in M47).
/// The sprite path one worn piece's trim resolves to, or `None` if the piece
/// carries no trim (M48).
///
/// `ArmorTrim.layerAssetId`, with the two halves looked up in the registries
/// the server synced:
///
/// ```java
/// MaterialAssetGroup.AssetInfo materialAsset = material.assets().assetId(equipmentAsset);
/// return pattern.assetId().withPath(p -> layerAssetPrefix + "/" + p + "_" + materialAsset.suffix());
/// ```
///
/// The `assetId(equipmentAsset)` step is what stops an iron trim on iron
/// armour vanishing: the material's `override_armor_assets` sends that pairing
/// to `iron_darker`.
pub(crate) fn trim_sprite_path(
    session: &PlaySession,
    piece: &rewo_world::entities::WornPiece,
    equipment_asset: &str,
    layer: rewo_data::equipment::ArmorLayer,
) -> Option<String> {
    let (material_id, pattern_id) = piece.trim?;
    let material = session.trim_materials.get(material_id as usize)?;
    let pattern = session.trim_patterns.get(pattern_id as usize)?;
    let suffix = material.suffix_for(equipment_asset);
    // `LayerType.trimAssetPrefix()` — `"trims/entity/" + this.id`.
    let prefix = format!("trims/entity/{}", layer.dir());
    Some(rewo_net::trim_parse::layer_asset_path(
        &pattern.asset_id,
        &prefix,
        suffix,
    ))
}

/// Permute and upload every trim sprite this frame needs, returning where each
/// one landed (M48).
///
/// A pre-pass because the upload needs `&mut` on the renderer while
/// [`collect_entities`] hands out borrows of the session. `upload_trim` caches
/// by path, so the second frame — and every frame after — does no work beyond
/// the lookups.
pub(crate) fn ensure_trims(
    session: &PlaySession,
    items: &rewo_data::items::Items,
    trims: &rewo_data::equipment::TrimAssets,
    gpu: &mut Gpu,
    wr: &mut WorldRenderer,
) -> std::collections::HashMap<String, (u32, u32)> {
    use rewo_data::equipment::ArmorLayer;
    let mut out = std::collections::HashMap::new();
    if trims.is_empty() {
        return out;
    }
    for (id, _) in session.world.entities.iter() {
        let worn = session.world.entities.armor(id);
        for (i, piece) in worn.iter().enumerate() {
            let Some(piece) = piece else { continue };
            if piece.trim.is_none() {
                continue;
            }
            let Some(asset) = items
                .name(piece.item)
                .and_then(rewo_data::item_props_table::equip_asset)
            else {
                continue;
            };
            let layer = if i == 2 {
                ArmorLayer::Leggings
            } else {
                ArmorLayer::Humanoid
            };
            let Some(path) = trim_sprite_path(session, piece, asset, layer) else {
                continue;
            };
            if out.contains_key(&path) {
                continue;
            }
            // `<prefix>/<pattern>_<suffix>` splits back into the source file
            // and the palette: the source is named without the material,
            // because one greyscale sheet makes all seventeen permutations.
            let Some((stem, suffix)) = path.rsplit_once('_') else {
                continue;
            };
            let Some(o) = trims
                .permute(stem, suffix)
                .and_then(|(rgba, w, h)| wr.upload_entity_trim(gpu, &path, &rgba, w, h))
            else {
                continue;
            };
            out.insert(path, o);
        }
    }
    out
}

/// M273 — `CustomHeadLayer.Transforms` per renderer, `(yOffset,
/// skullYOffset, horizontalScale)`; `None` for an entity whose renderer has
/// no `CustomHeadLayer` (a cow wearing a skull draws none).
pub(crate) fn custom_head_transforms(entity: &str) -> Option<(f32, f32, f32)> {
    match entity.strip_prefix("minecraft:").unwrap_or(entity) {
        // `VillagerRenderer.CUSTOM_HEAD_TRANSFORMS` (the zombie villager's
        // too).
        "villager" | "zombie_villager" => Some((-0.117_187_5, -0.074_218_75, 1.0)),
        // `PiglinRenderer.PIGLIN_CUSTOM_HEAD_TRANSFORMS`.
        "piglin" | "piglin_brute" | "zombified_piglin" => Some((0.0, 0.0, 1.001_953_1)),
        "armor_stand" | "copper_golem" | "player" | "mannequin" | "zombie" | "husk" | "drowned"
        | "skeleton" | "stray" | "bogged" | "wither_skeleton" | "parched" | "evoker"
        | "illusioner" | "pillager" | "vindicator" | "wandering_trader" => Some((0.0, 0.0, 1.0)),
        _ => None,
    }
}

/// M273 — what `id` wears on its head, if its renderer has a
/// `CustomHeadLayer` (`LivingEntityRenderer.extractRenderState`): a head item
/// whose block is an `AbstractSkullBlock` draws as that skull (a player head
/// in its profile's skin once that has arrived); M274 — any other item that
/// `HumanoidArmorLayer` does not draw (no equipment asset: a carved pumpkin,
/// a block, a banner) draws as itself in the `head` display context.
fn worn_head<'a>(
    session: &'a PlaySession,
    id: i32,
    entity: &str,
    items: &'a rewo_data::items::Items,
    head_skins: Option<&'a HeadSkins>,
) -> Option<rewo_gpu::entities::WornHead<'a>> {
    let (y_offset, skull_y_offset, horizontal_scale) = custom_head_transforms(entity)?;
    let item = items.name(session.world.entities.armor(id)[0]?.item)?;
    let Some(base) = rewo_data::chest_states::skull_model(item) else {
        if rewo_data::item_props_table::equip_asset(item).is_some() {
            return None;
        }
        return Some(rewo_gpu::entities::WornHead {
            model: item,
            item: true,
            y_offset,
            skull_y_offset,
            horizontal_scale,
        });
    };
    let skinned = session
        .world
        .entities
        .head_profile(id)
        .zip(head_skins)
        .and_then(|(v, h)| h.ready(v));
    Some(rewo_gpu::entities::WornHead {
        model: skinned.unwrap_or(base),
        item: false,
        y_offset,
        skull_y_offset,
        horizontal_scale,
    })
}

pub(crate) fn armor_keys<'a>(
    session: &PlaySession,
    id: i32,
    items: &rewo_data::items::Items,
    equipment: &'a rewo_data::equipment::EquipmentAssets,
    trim_slots: &std::collections::HashMap<String, (u32, u32)>,
) -> [Option<rewo_gpu::entities::ArmorPiece<'a>>; 4] {
    use rewo_data::equipment::{color_for_layer, dye_argb, ArmorLayer};
    let worn = session.world.entities.armor(id);
    std::array::from_fn(|i| {
        let piece = worn[i]?;
        let asset = rewo_data::item_props_table::equip_asset(items.name(piece.item)?)?;
        // head 0, chest 1, legs 2, feet 3 — only the legs are inner.
        let layer = if i == 2 {
            ArmorLayer::Leggings
        } else {
            ArmorLayer::Humanoid
        };
        let dye = dye_argb(piece.dye);
        let defs = equipment.layers(asset, layer);
        if defs.len() > rewo_gpu::entities::MAX_ARMOR_SUBLAYERS {
            log::warn!(
                "armour: {asset} {:?} declares {} layers, drawing the first {}",
                layer,
                defs.len(),
                rewo_gpu::entities::MAX_ARMOR_SUBLAYERS
            );
        }
        let mut out = rewo_gpu::entities::ArmorPiece {
            // M50: `hasFoil`, decoded off the equipment packet's component
            // patch. One flag per piece — vanilla submits the foil once,
            // riding whichever layer draws first.
            foil: piece.foil,
            ..Default::default()
        };
        // The trim, if this piece has one and its sprite made it into the pool.
        out.trim = trim_sprite_path(session, &piece, asset, layer)
            .and_then(|p| trim_slots.get(&p).copied());
        let mut n = 0;
        for def in defs {
            let color = color_for_layer(def.dyeable, dye);
            // **Zero is not a black tint, it is no draw at all** — vanilla's
            // `if (color != 0)`. An undyed `onlyIfDyed` layer lands here.
            if color == 0 {
                continue;
            }
            if n == rewo_gpu::entities::MAX_ARMOR_SUBLAYERS {
                break;
            }
            out.layers[n] = Some((
                def.key.as_str(),
                [
                    ((color >> 16) & 0xFF) as f32 / 255.0,
                    ((color >> 8) & 0xFF) as f32 / 255.0,
                    (color & 0xFF) as f32 / 255.0,
                ],
            ));
            n += 1;
        }
        // Every layer suppressed is the same as no piece — but a trim alone
        // still draws, because vanilla submits it outside the layer loop.
        (n > 0 || out.trim.is_some()).then_some(out)
    })
}

/// M294 — the body slot's equipment layers (`HORSE_BODY`), resolved as
/// [`armor_keys`] resolves a humanoid piece's: the item's asset, its
/// `horse_body` layer list, each layer's colour (`color_for_layer`, zero for
/// no draw).
/// M311 — `Crackiness.WOLF_ARMOR.byFraction((max - damage) / max)`: below
/// 0.32 of its durability left HIGH, below 0.69 MEDIUM, below 0.95 LOW.
pub(crate) fn wolf_crack_level(damage: i32, max: i32) -> Option<&'static str> {
    if max <= 0 {
        return None;
    }
    let left = (max - damage) as f32 / max as f32;
    if left < 0.32 {
        Some("wolf_armor_crackiness_high")
    } else if left < 0.69 {
        Some("wolf_armor_crackiness_medium")
    } else if left < 0.95 {
        Some("wolf_armor_crackiness_low")
    } else {
        None
    }
}

/// M311 — an adult wolf's worn armour's crack sheet, if it is damaged enough
/// (`Crackiness.byDamage` reads a damageable stack's damage and maximum; the
/// maximum is the patch's or the item's prototype's).
pub(crate) fn wolf_armor_crack(
    session: &PlaySession,
    id: i32,
    kind: EntityModelKind,
    items: &rewo_data::items::Items,
) -> Option<&'static str> {
    if kind != EntityModelKind::Wolf || session.world.entities.is_baby(id) {
        return None;
    }
    let piece = session.world.entities.body_armor(id)?;
    let (damage, max) = piece.wear?;
    let max = max.or_else(|| rewo_data::item_props_table::max_damage(items.name(piece.item)?))?;
    wolf_crack_level(damage, max)
}

pub(crate) fn body_armor_piece<'a>(
    session: &PlaySession,
    id: i32,
    kind: EntityModelKind,
    items: &rewo_data::items::Items,
    equipment: &'a rewo_data::equipment::EquipmentAssets,
) -> Option<rewo_gpu::entities::ArmorPiece<'a>> {
    use rewo_data::equipment::{color_for_layer, dye_argb, ArmorLayer};
    // The renderer's layer type: `HORSE_BODY` on the horse family,
    // `LLAMA_BODY` on the llamas (M297; `LlamaDecorLayer` draws a carpet only
    // on an adult, and the trader llama's own blanket when it wears none).
    let layer = match kind {
        EntityModelKind::Horse | EntityModelKind::SkeletonHorse | EntityModelKind::ZombieHorse => {
            ArmorLayer::HorseBody
        }
        EntityModelKind::Llama | EntityModelKind::TraderLlama => ArmorLayer::LlamaBody,
        // M299 — `WolfArmorLayer`'s `WOLF_BODY`, on an adult only.
        EntityModelKind::Wolf => ArmorLayer::WolfBody,
        // M301 — `NAUTILUS_BODY` (both nautilus renderers).
        EntityModelKind::Nautilus
        | EntityModelKind::ZombieNautilus
        | EntityModelKind::ZombieNautilusCoral => ArmorLayer::NautilusBody,
        // M302 — `HAPPY_GHAST_BODY` (Rewo draws a baby at the adult size,
        // so with the adult harness).
        EntityModelKind::HappyGhast => ArmorLayer::HappyGhastBody,
        _ => return None,
    };
    let adult_only = matches!(layer, ArmorLayer::LlamaBody | ArmorLayer::WolfBody);
    let worn = session
        .world
        .entities
        .body_armor(id)
        .filter(|_| !adult_only || !session.world.entities.is_baby(id))
        .and_then(|piece| {
            let asset = rewo_data::item_props_table::equip_asset(items.name(piece.item)?)?;
            Some((piece, asset))
        });
    let (piece, asset) = match worn {
        Some((piece, asset)) => (Some(piece), asset),
        // `EquipmentAssets.TRADER_LLAMA` (the baby's own blanket is not drawn:
        // Rewo draws foals at the adult size).
        None if kind == EntityModelKind::TraderLlama => (None, "minecraft:trader_llama"),
        None => return None,
    };
    let dye = dye_argb(piece.and_then(|p| p.dye));
    let mut out = rewo_gpu::entities::ArmorPiece {
        foil: piece.is_some_and(|p| p.foil),
        ..Default::default()
    };
    let mut n = 0;
    for def in equipment.layers(asset, layer) {
        let color = color_for_layer(def.dyeable, dye);
        if color == 0 || n == rewo_gpu::entities::MAX_ARMOR_SUBLAYERS {
            continue;
        }
        out.layers[n] = Some((
            def.key.as_str(),
            [
                ((color >> 16) & 0xFF) as f32 / 255.0,
                ((color >> 8) & 0xFF) as f32 / 255.0,
                (color & 0xFF) as f32 / 255.0,
            ],
        ));
        n += 1;
    }
    (n > 0).then_some(out)
}

/// M295 — the saddle slot's equipment layers for the horse family's
/// renderers (`HORSE_SADDLE`, `DONKEY_SADDLE`, `MULE_SADDLE`,
/// `SKELETON_HORSE_SADDLE`, `ZOMBIE_HORSE_SADDLE`), resolved as
/// [`body_armor_piece`] resolves the body slot.
pub(crate) fn saddle_piece<'a>(
    session: &PlaySession,
    id: i32,
    kind: EntityModelKind,
    items: &rewo_data::items::Items,
    equipment: &'a rewo_data::equipment::EquipmentAssets,
) -> Option<rewo_gpu::entities::ArmorPiece<'a>> {
    use rewo_data::equipment::{color_for_layer, dye_argb, ArmorLayer};
    let layer = match kind {
        EntityModelKind::Horse => ArmorLayer::HorseSaddle,
        EntityModelKind::Donkey => ArmorLayer::DonkeySaddle,
        EntityModelKind::Mule => ArmorLayer::MuleSaddle,
        EntityModelKind::SkeletonHorse => ArmorLayer::SkeletonHorseSaddle,
        EntityModelKind::ZombieHorse => ArmorLayer::ZombieHorseSaddle,
        // M298 — `PIG_SADDLE`, `STRIDER_SADDLE`.
        // M349 — the cold pig wears the same `PIG_SADDLE`.
        EntityModelKind::Pig | EntityModelKind::ColdPig => ArmorLayer::PigSaddle,
        EntityModelKind::Strider => ArmorLayer::StriderSaddle,
        // M300 — `CAMEL_SADDLE`.
        EntityModelKind::Camel => ArmorLayer::CamelSaddle,
        // M303 — `CAMEL_HUSK_SADDLE`.
        EntityModelKind::CamelHusk => ArmorLayer::CamelHuskSaddle,
        // M301 — `NAUTILUS_SADDLE` (both nautilus renderers).
        EntityModelKind::Nautilus
        | EntityModelKind::ZombieNautilus
        | EntityModelKind::ZombieNautilusCoral => ArmorLayer::NautilusSaddle,
        _ => return None,
    };
    let piece = session.world.entities.saddle_piece(id)?;
    let asset = rewo_data::item_props_table::equip_asset(items.name(piece.item)?)?;
    let dye = dye_argb(piece.dye);
    let mut out = rewo_gpu::entities::ArmorPiece {
        foil: piece.foil,
        ..Default::default()
    };
    let mut n = 0;
    for def in equipment.layers(asset, layer) {
        let color = color_for_layer(def.dyeable, dye);
        if color == 0 || n == rewo_gpu::entities::MAX_ARMOR_SUBLAYERS {
            continue;
        }
        out.layers[n] = Some((
            def.key.as_str(),
            [
                ((color >> 16) & 0xFF) as f32 / 255.0,
                ((color >> 8) & 0xFF) as f32 / 255.0,
                (color & 0xFF) as f32 / 255.0,
            ],
        ));
        n += 1;
    }
    (n > 0).then_some(out)
}

pub(crate) fn held_foil(
    session: &PlaySession,
    id: i32,
    hand: rewo_world::entities::InteractionHand,
) -> bool {
    matches!(
        session.world.entities.hand_item(id, hand),
        rewo_world::entities::HandItem::Held(h) if h.glint
    )
}

pub(crate) fn bob_offset_for(id: i32) -> f32 {
    let mut z = (id as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // The top 24 bits as a [0,1) float, matching `nextFloat`'s precision.
    let unit = (z >> 40) as f32 * 5.960_464_5e-8;
    unit * std::f32::consts::TAU
}

/// Resolve one entity's vanilla emissive-layer inputs (M52).
///
/// Both are real wire state now, so neither is the branch's "vanilla synched
/// default" placeholder any more:
///
/// * `tendril` — `Warden.getTendrilAnimation(partial)`. Entity event **61**
///   sets `tendrilAnimation = 10` and `Warden.tick` decrements it once per
///   client tick, so a continuous clock reads it as `max(0, 10 - elapsed) / 10`.
///   A warden that has heard nothing has no receipt tick and reads 0 — still,
///   dark tendrils, which is exactly what vanilla shows.
/// * `eyes_glow` — `Creaking.isActive()`, metadata index 17 BOOLEAN, whose
///   `defineSynchedData` default is `false`.
pub(super) fn emissive_state(
    session: &PlaySession,
    id: i32,
    kind: EntityModelKind,
    _now: f32,
) -> rewo_gpu::entities::EmissiveState {
    use rewo_world::entities::EntityEvent;
    let ents = &session.world.entities;
    let tendril = match kind {
        EntityModelKind::Warden => {
            ents.event_start(id, EntityEvent::WardenTendril)
                .map_or(0.0, |start| {
                    let elapsed = (session.ticks as i64 - start).max(0) as f32;
                    ((WARDEN_TENDRIL_TICKS - elapsed) / WARDEN_TENDRIL_TICKS).clamp(0.0, 1.0)
                })
        }
        _ => 0.0,
    };
    rewo_gpu::entities::EmissiveState {
        tendril,
        eyes_glow: matches!(kind, EntityModelKind::Creaking) && ents.creaking_active(id),
    }
}

/// `Warden.tendrilAnimation`'s start value, and the divisor
/// `getTendrilAnimation` normalizes by.
pub(super) const WARDEN_TENDRIL_TICKS: f32 = 10.0;

/// Choose one entity's ETF texture variant. The properties are keyed by the
/// mob-texture key, so a mob whose model uses several textures varies on its
/// *first* — the one a pack's `<entity>.properties` names.
pub(super) fn etf_variant(
    etf: &rewo_data::etf::EtfPack,
    kind: EntityModelKind,
    e: &rewo_world::entities::EntityState,
    id: i32,
    session: &PlaySession,
    cube_size: Option<i32>,
) -> u16 {
    let Some(key) = rewo_gpu::mobs::MOBS
        .iter()
        .find(|d| d.kind == kind)
        .and_then(|d| d.textures.first())
    else {
        return 0;
    };
    let props = rewo_data::etf::EntityProps {
        uuid: e.uuid,
        name: session.world.entities.custom_name(id),
        baby: session.world.entities.is_baby(id),
        size: cube_size,
        y: e.y.floor() as i32,
        // Before the first time packet the world clock reads 0 (dawn), which is
        // the same assumption the renderer makes elsewhere.
        day_ticks: session.day_ticks.unwrap_or(0),
    };
    etf.pick(key, &props) as u16
}

/// Vanilla's own metadata-driven texture variant for one entity (M64).
///
/// 0 — the baked base texture — for a mob that has none, for one whose server
/// never sent one, and for one whose variant names a texture the jar does not
/// ship. That last case is the M57b rule: a variant we cannot resolve leaves
/// the mob on its vanilla sheet rather than painting an invented one.
///
/// **The value's units depend on the mob and the kind is what says which.**
/// Cat, wolf and frog carry a raw datapack-registry id, so this walks the
/// registry the server synced and joins on the *texture path* — never on the
/// id, which is the server's to choose (REWO_PLAN §0.0, and M16/M42's rule).
/// Horse, llama and axolotl carry an enum ordinal, so those are transcribed
/// tables, each with its own out-of-bounds strategy.
///
/// The wolf is the one that reads a second field: `Wolf.getTexture` picks
/// `assets.tame` over `assets.wild` on `isTame()`, which is bit 0x04 of the
/// index-18 byte. Its third sheet, `angry`, is not chosen here — see
/// `rewo_data::mob_variants`.
/// `SheepWoolUndercoatLayer.submit`'s gate (M68), transcribed:
///
/// ```java
/// if (!state.isInvisible && (state.isJebSheep || state.woolColor != DyeColor.WHITE) && !state.isBaby)
/// ```
///
/// Two things to read off it. **There is no `isSheared` test** — shearing
/// takes `SheepWoolLayer` and leaves this one, which is why a shorn dyed sheep
/// keeps colour where a shorn white one is bare. And `woolColor` defaults to
/// `WHITE`, so an un-synced sheep gets nothing.
///
/// `isJebSheep` is deliberately absent here: it selects `ColorLerper
/// .getLerpedColor`'s rainbow, which Rewo does not render at all, so
/// including the disjunct would draw the layer in a colour the fleece beside
/// it is not wearing. Left out rather than approximated.
///
/// Shared with the `--tint-check` gate so the gate cannot grade a rule the
/// client does not actually apply (M18's lesson).
pub(crate) fn undercoat_visible(kind: EntityModelKind, dye: Option<u8>, is_baby: bool) -> bool {
    kind == EntityModelKind::Sheep && dye.unwrap_or(0) != 0 && !is_baby
}

/// The baby's own model, where vanilla's renderer swaps one in: M307 —
/// `SulfurCubeRenderer.submit`'s `this.model = state.isBaby ? smallModel :
/// normalModel`; M314 — `AgeableMobRenderer`'s for the happy ghast.
pub(crate) fn baby_model_kind(kind: EntityModelKind, baby: bool) -> EntityModelKind {
    match kind {
        EntityModelKind::SulfurCube if baby => EntityModelKind::SulfurCubeSmall,
        EntityModelKind::HappyGhast if baby => EntityModelKind::HappyGhastBaby,
        // M315 — `TurtleRenderer`'s `BabyTurtleModel`.
        EntityModelKind::Turtle if baby => EntityModelKind::TurtleBaby,
        // M316 — `PolarBearRenderer`'s `BabyPolarBearModel`.
        EntityModelKind::PolarBear if baby => EntityModelKind::PolarBearBaby,
        // M317 — `ZombieRenderer`'s `BabyZombieModel` (the husk's too).
        EntityModelKind::Zombie if baby => EntityModelKind::ZombieBaby,
        EntityModelKind::Husk if baby => EntityModelKind::HuskBaby,
        // M318 — `VillagerRenderer`'s `BabyVillagerModel`.
        EntityModelKind::Villager if baby => EntityModelKind::VillagerBaby,
        // M319 — `DrownedRenderer`'s `BabyDrownedModel`.
        EntityModelKind::Drowned if baby => EntityModelKind::DrownedBaby,
        // M320 — `BabyPiglinModel`, for both piglins.
        EntityModelKind::Piglin if baby => EntityModelKind::PiglinBaby,
        EntityModelKind::ZombifiedPiglin if baby => EntityModelKind::ZombifiedPiglinBaby,
        // M321 — `BabyCowModel`, the cow's and the mooshroom's.
        EntityModelKind::Cow if baby => EntityModelKind::CowBaby,
        // M348 — a cold or warm calf is the same `BabyCowModel`.
        EntityModelKind::ColdCow | EntityModelKind::WarmCow if baby => EntityModelKind::CowBaby,
        // M349 — a cold piglet or chick is the one baby model too.
        EntityModelKind::ColdPig if baby => EntityModelKind::PigBaby,
        EntityModelKind::ColdChicken if baby => EntityModelKind::ChickenBaby,
        EntityModelKind::Mooshroom if baby => EntityModelKind::MooshroomBaby,
        // M322 — `BabyPigModel` and `BabyChickenModel`.
        EntityModelKind::Pig if baby => EntityModelKind::PigBaby,
        EntityModelKind::Chicken if baby => EntityModelKind::ChickenBaby,
        // M323 — `BabySheepModel`.
        EntityModelKind::Sheep if baby => EntityModelKind::SheepBaby,
        // M324 — `BabyDolphinModel`.
        EntityModelKind::Dolphin if baby => EntityModelKind::DolphinBaby,
        // M325 — `BabyZombieVillagerModel`.
        EntityModelKind::ZombieVillager if baby => EntityModelKind::ZombieVillagerBaby,
        // M326 — `BabySquidModel`, the squid's and the glow squid's.
        EntityModelKind::Squid if baby => EntityModelKind::SquidBaby,
        EntityModelKind::GlowSquid if baby => EntityModelKind::GlowSquidBaby,
        // M327 — `BabyHoglinModel`, the hoglin's and the zoglin's.
        EntityModelKind::Hoglin if baby => EntityModelKind::HoglinBaby,
        EntityModelKind::Zoglin if baby => EntityModelKind::ZoglinBaby,
        // M328 — `BabyStriderModel`.
        EntityModelKind::Strider if baby => EntityModelKind::StriderBaby,
        // M330 — `BabyFoxModel`.
        EntityModelKind::Fox if baby => EntityModelKind::FoxBaby,
        // M331 — `BabyPandaModel`.
        EntityModelKind::Panda if baby => EntityModelKind::PandaBaby,
        // M332 — `BabyOcelotModel` (the cat's kittens wait for their variants).
        EntityModelKind::Ocelot if baby => EntityModelKind::OcelotBaby,
        // M333 — `BabyBeeModel`.
        EntityModelKind::Bee if baby => EntityModelKind::BeeBaby,
        // M334 — `BabyDonkeyModel`, the donkey's and the mule's.
        EntityModelKind::Donkey if baby => EntityModelKind::DonkeyBaby,
        EntityModelKind::Mule if baby => EntityModelKind::MuleBaby,
        // M335 — `BabyCamelModel`.
        EntityModelKind::Camel if baby => EntityModelKind::CamelBaby,
        // M336 — `BabyRabbitModel`.
        EntityModelKind::Rabbit if baby => EntityModelKind::RabbitBaby,
        // M337 — `BabyGoatModel`.
        EntityModelKind::Goat if baby => EntityModelKind::GoatBaby,
        // M338 — `BabyLlamaModel`.
        EntityModelKind::Llama if baby => EntityModelKind::LlamaBaby,
        // M339 — `BabyHorseModel`.
        EntityModelKind::Horse if baby => EntityModelKind::HorseBaby,
        // M340 — `BabyCatModel`, the ocelot kitten's layer.
        EntityModelKind::Cat if baby => EntityModelKind::CatBaby,
        // M341 — `BabyWolfModel`.
        EntityModelKind::Wolf if baby => EntityModelKind::WolfBaby,
        k => k,
    }
}

/// A baby's `(render scale, bounds scale)`. Rewo approximates a baby it has
/// no model for as the adult at half size (both halves); a baby drawn by its
/// own model takes no render scale (`AgeableMobRenderer` adds none) and its
/// bounds shrink by the entity's `getAgeScale` (M307, M314, M315).
pub(crate) fn baby_scales(kind: EntityModelKind) -> (f32, f32) {
    match kind {
        EntityModelKind::SulfurCubeSmall => (1.0, 0.5),
        EntityModelKind::HappyGhastBaby => (1.0, 0.2375),
        EntityModelKind::TurtleBaby => (1.0, 0.3),
        EntityModelKind::PolarBearBaby => (1.0, 0.5),
        EntityModelKind::ZombieBaby | EntityModelKind::HuskBaby => (1.0, 0.5),
        EntityModelKind::VillagerBaby | EntityModelKind::DrownedBaby => (1.0, 0.5),
        EntityModelKind::PiglinBaby | EntityModelKind::ZombifiedPiglinBaby => (1.0, 0.5),
        EntityModelKind::CowBaby | EntityModelKind::MooshroomBaby => (1.0, 0.5),
        EntityModelKind::PigBaby | EntityModelKind::ChickenBaby => (1.0, 0.5),
        EntityModelKind::SheepBaby | EntityModelKind::DolphinBaby => (1.0, 0.5),
        EntityModelKind::ZombieVillagerBaby => (1.0, 0.5),
        EntityModelKind::SquidBaby | EntityModelKind::GlowSquidBaby => (1.0, 0.5),
        EntityModelKind::HoglinBaby | EntityModelKind::ZoglinBaby => (1.0, 0.5),
        EntityModelKind::StriderBaby | EntityModelKind::FoxBaby => (1.0, 0.5),
        EntityModelKind::PandaBaby | EntityModelKind::OcelotBaby => (1.0, 0.5),
        EntityModelKind::BeeBaby => (1.0, 0.5),
        EntityModelKind::DonkeyBaby | EntityModelKind::MuleBaby => (1.0, 0.5),
        EntityModelKind::CamelBaby | EntityModelKind::RabbitBaby => (1.0, 0.5),
        EntityModelKind::GoatBaby | EntityModelKind::LlamaBaby => (1.0, 0.5),
        EntityModelKind::HorseBaby | EntityModelKind::CatBaby => (1.0, 0.5),
        EntityModelKind::WolfBaby => (1.0, 0.5),
        _ => (0.5, 0.5),
    }
}

/// Which of the two tropical-fish meshes a packed variant selects (M68).
///
/// `TropicalFishRenderer.submit` assigns `this.model` from
/// `state.pattern.base()` before *every* submission, so the shape is not a
/// property of the entity type — it is a field of the synched int, and the
/// low bit at that (`Pattern.packedId` is `base.id | index << 8`). One wire
/// name, two `EntityModelKind`s, chosen by the caller: the `PlayerSlim`
/// shape. Shared with `--variant-check` so the gate grades the client's own
/// mapping.
pub(crate) fn fish_kind(v: rewo_data::mob_variants::FishVariant) -> EntityModelKind {
    match v.base {
        rewo_data::mob_variants::FishBase::Small => EntityModelKind::TropicalFish,
        rewo_data::mob_variants::FishBase::Large => EntityModelKind::TropicalFishLarge,
    }
}
/// M342/M343 — a wolf's or a cat's collar (and their young's): none unless
/// tame, then the colour the server sent or the class's
/// `DEFAULT_COLLAR_COLOR` (red, 14, for both) when it sent none.
pub(crate) fn mob_collar(kind: EntityModelKind, tame: bool, sent: Option<i32>) -> Option<u8> {
    let collared = matches!(
        kind,
        EntityModelKind::Wolf
            | EntityModelKind::WolfBaby
            | EntityModelKind::Cat
            | EntityModelKind::CatBaby
    );
    (collared && tame).then(|| (sent.unwrap_or(14) & 15) as u8)
}

/// M340 — the variant a mob holds when the server sent none: each class's
/// `defineSynchedData` default. The registry mobs' default is a key
/// (`Cat.DEFAULT_VARIANT` = black, `WolfVariants.DEFAULT` = pale,
/// `Frog.DEFAULT_VARIANT` = temperate) resolved in the synced registry by
/// `VariantUtils.getDefaultOrAny`, which falls back to any entry (the
/// first) when the key is missing; the ordinal mobs' is 0 (horse WHITE with
/// no markings, llama CREAMY, axolotl LUCY). `None` for a kind with no
/// variant here.
///
/// M348: `defs` is the kind's own registry ([`variant_registry`]).
pub(crate) fn default_variant(
    kind: EntityModelKind,
    defs: &[rewo_net::variant_parse::MobVariantDef],
) -> Option<i32> {
    let by_key = |key: &str| {
        let i = defs.iter().position(|d| d.id == key).unwrap_or(0);
        i32::try_from(i).ok()
    };
    match kind {
        EntityModelKind::Cat | EntityModelKind::CatBaby => by_key("minecraft:black"),
        EntityModelKind::Wolf | EntityModelKind::WolfBaby => by_key("minecraft:pale"),
        // M348 — `CowVariants.TEMPERATE` too, adult (any mesh) and calf;
        // M349 — the pig's and the chicken's.
        EntityModelKind::Frog
        | EntityModelKind::Cow
        | EntityModelKind::ColdCow
        | EntityModelKind::WarmCow
        | EntityModelKind::CowBaby
        | EntityModelKind::Pig
        | EntityModelKind::ColdPig
        | EntityModelKind::PigBaby
        | EntityModelKind::Chicken
        | EntityModelKind::ColdChicken
        | EntityModelKind::ChickenBaby
        // M350 — `ZombieNautilusVariants.TEMPERATE`.
        | EntityModelKind::ZombieNautilus
        | EntityModelKind::ZombieNautilusCoral => by_key("minecraft:temperate"),
        EntityModelKind::Horse
        | EntityModelKind::HorseBaby
        | EntityModelKind::Llama
        | EntityModelKind::LlamaBaby
        | EntityModelKind::Axolotl
        | EntityModelKind::Parrot
        | EntityModelKind::Fox
        | EntityModelKind::FoxBaby
        | EntityModelKind::Rabbit
        | EntityModelKind::RabbitBaby
        | EntityModelKind::Mooshroom
        | EntityModelKind::MooshroomBaby
        // M354 — the bee's sheet is not a synched variant; any value reaches
        // its arm, which reads the flags and the anger.
        | EntityModelKind::Bee
        | EntityModelKind::BeeBaby => Some(0),
        _ => None,
    }
}

/// M348 — the datapack registry a kind's variant indexes (empty for the
/// enum-variant mobs).
pub(crate) fn variant_registry(
    kind: EntityModelKind,
    session: &PlaySession,
) -> &[rewo_net::variant_parse::MobVariantDef] {
    match kind {
        EntityModelKind::Cat | EntityModelKind::CatBaby => &session.cat_variants,
        EntityModelKind::Wolf | EntityModelKind::WolfBaby => &session.wolf_variants,
        EntityModelKind::Frog => &session.frog_variants,
        EntityModelKind::Cow
        | EntityModelKind::ColdCow
        | EntityModelKind::WarmCow
        | EntityModelKind::CowBaby => &session.cow_variants,
        // M349.
        EntityModelKind::Pig | EntityModelKind::ColdPig | EntityModelKind::PigBaby => {
            &session.pig_variants
        }
        EntityModelKind::Chicken | EntityModelKind::ColdChicken | EntityModelKind::ChickenBaby => {
            &session.chicken_variants
        }
        // M350.
        EntityModelKind::ZombieNautilus | EntityModelKind::ZombieNautilusCoral => {
            &session.zombie_nautilus_variants
        }
        _ => &[],
    }
}

/// M348 — the registry entry a mob's variant names: the synched holder id,
/// or the class's default when the server sent none.
pub(crate) fn variant_def(
    kind: EntityModelKind,
    id: i32,
    session: &PlaySession,
) -> Option<&rewo_net::variant_parse::MobVariantDef> {
    let defs = variant_registry(kind, session);
    let v = session
        .world
        .entities
        .variant(id)
        .or_else(|| default_variant(kind, defs))?;
    defs.get(usize::try_from(v).ok()?)
}

/// M348 — `CowRenderer.models`: COLD and WARM have their own adult meshes;
/// NORMAL (no `model`) and anything else is the base cow. M349 —
/// `PigRenderer` / `ChickenRenderer`: COLD has its own; WARM is NORMAL's.
/// M350 — `ZombieNautilusRenderer`: WARM is the coral mesh.
pub(crate) fn farm_model_kind(kind: EntityModelKind, model: Option<&str>) -> EntityModelKind {
    match (kind, model) {
        (EntityModelKind::Cow, Some("cold")) => EntityModelKind::ColdCow,
        (EntityModelKind::Cow, Some("warm")) => EntityModelKind::WarmCow,
        (EntityModelKind::Pig, Some("cold")) => EntityModelKind::ColdPig,
        (EntityModelKind::Chicken, Some("cold")) => EntityModelKind::ColdChicken,
        (EntityModelKind::ZombieNautilus, Some("warm")) => EntityModelKind::ZombieNautilusCoral,
        _ => kind,
    }
}

/// M386 — the draw yaw and the horizontal offset (x, z, in blocks) for a
/// kind: the ender dragon's renderer turns it `rotY(−yr)` and moves it a
/// block forward in that frame, `R_y(−yr)·(0, 0, 1) = (−sin yr, cos yr)`;
/// the generic draw turns `180 − yaw`, so the dragon's yaw gains 180°.
/// Every other kind: its yaw, no offset.
pub(crate) fn dragon_frame(kind: EntityModelKind, yaw: f32) -> (f32, [f32; 2]) {
    if kind != EntityModelKind::EnderDragon {
        return (yaw, [0.0; 2]);
    }
    let r = yaw.to_radians();
    (yaw + 180.0, [-r.sin(), r.cos()])
}

/// M367 — `HoglinModel.setupAnim`'s `headbuttLerpFactor = 1 − |10 − 2r| /
/// 10` for `r` remaining attack ticks (0 when none remain).
pub(crate) fn headbutt_factor(remaining: i64) -> f32 {
    let r = remaining.clamp(0, 10) as f32;
    1.0 - (10.0 - 2.0 * r).abs() / 10.0
}

/// M358 — `Sheep.getHeadEatPositionScale(a)` and `getHeadEatAngleScale(a)`
/// from the eat clock `k` (`eatAnimationTick`: 40 at entity event 10, one
/// less each client tick) and the partial tick `a`; `None` once it has run
/// out, when the head looks again (the angle's last branch, the pitch).
pub(crate) fn sheep_eat_pose(k: i64, a: f32) -> Option<(f32, f32)> {
    use std::f32::consts::PI;
    if k <= 0 {
        return None;
    }
    let kf = k as f32;
    let position = if (4..=36).contains(&k) {
        1.0
    } else if k < 4 {
        (kf - a) / 4.0
    } else {
        -(kf - 40.0 - a) / 4.0
    };
    let angle = if k > 4 && k <= 36 {
        PI / 5.0 + 0.219_911_49 * ((kf - 4.0 - a) / 32.0 * 28.7).sin()
    } else {
        PI / 5.0
    };
    Some((position, angle))
}

/// M355 — `Wolf.getTailAngle()`: an angry wolf's tail stands at 1.5393804,
/// a tame one's at (0.55 − damage · 0.4)·π by its missing health (full
/// health when unknown), a wild one's at π/5.
pub(crate) fn wolf_tail_angle(angry: bool, tame: bool, health: Option<(f32, f32)>) -> f32 {
    use std::f32::consts::PI;
    if angry {
        1.539_380_4
    } else if tame {
        let damage = health
            .filter(|(_, max)| *max > 0.0)
            .map_or(0.0, |(h, max)| ((max - h) / max).clamp(0.0, 1.0));
        (0.55 - damage * 0.4) * PI
    } else {
        PI / 5.0
    }
}

/// M354 — `Bee.isAngry()`, M355 the wolf's: `NeutralMob`'s
/// `DATA_ANGER_END_TIME` against the world clock.
pub(crate) fn mob_is_angry(session: &PlaySession, id: i32) -> bool {
    rewo_net::tickable::is_angry(
        session.world.entities.anger_end_time(id).unwrap_or(-1),
        session.game_time(),
    )
}

pub(crate) fn vanilla_variant(kind: EntityModelKind, id: i32, session: &PlaySession) -> u16 {
    // M223 — a boat's wood is its TYPE, not a metadata variant.
    if matches!(kind, EntityModelKind::Boat | EntityModelKind::ChestBoat) {
        return session
            .world
            .entities
            .get(id)
            .and_then(|e| session.entity_types.as_deref()?.name(e.type_id))
            .and_then(rewo_data::mob_variants::boat_texture)
            .and_then(|t| rewo_data::mob_variants::variant_id(&t))
            .unwrap_or(0);
    }
    // M340 — a variant the server never sent is the class's default: it
    // sends only non-default metadata (`getNonDefaultValues`), and the
    // client's `SynchedEntityData` starts at `defineSynchedData`'s value. A
    // black cat or a white horse is that default, so before M340 every one
    // drew Rewo's base sheet instead (tabby, brown).
    let Some(v) = session
        .world
        .entities
        .variant(id)
        .or_else(|| default_variant(kind, variant_registry(kind, session)))
    else {
        return 0;
    };
    let registry = |defs: &[rewo_net::variant_parse::MobVariantDef], tame: bool| {
        usize::try_from(v)
            .ok()
            .and_then(|i| defs.get(i))
            .and_then(|d| d.texture(tame))
            .and_then(rewo_data::mob_variants::variant_id)
            .unwrap_or(0)
    };
    // M340 — the same entry's baby sheet (`assetInfo(isBaby)`).
    let registry_baby = |defs: &[rewo_net::variant_parse::MobVariantDef], tame: bool| {
        usize::try_from(v)
            .ok()
            .and_then(|i| defs.get(i))
            .and_then(|d| d.baby_texture(tame))
            .and_then(rewo_data::mob_variants::variant_id)
            .unwrap_or(0)
    };
    match kind {
        EntityModelKind::Cat => registry(&session.cat_variants, false),
        EntityModelKind::CatBaby => registry_baby(&session.cat_variants, false),
        // M355 — `Wolf.getTexture` whole: tame, else angry, else wild.
        EntityModelKind::Wolf | EntityModelKind::WolfBaby => usize::try_from(v)
            .ok()
            .and_then(|i| session.wolf_variants.get(i))
            .and_then(|d| {
                d.wolf_texture(
                    kind == EntityModelKind::WolfBaby,
                    session.world.entities.is_tame(id),
                    mob_is_angry(session, id),
                )
            })
            .and_then(rewo_data::mob_variants::variant_id)
            .unwrap_or(0),
        EntityModelKind::Frog => registry(&session.frog_variants, false),
        // M348 — the cow's sheet, on whichever mesh its `model` picked, and
        // the calf's (`CowRenderer.getTextureLocation`).
        EntityModelKind::Cow | EntityModelKind::ColdCow | EntityModelKind::WarmCow => {
            registry(&session.cow_variants, false)
        }
        EntityModelKind::CowBaby => registry_baby(&session.cow_variants, false),
        // M349 — the pig's and the chicken's, the same way.
        EntityModelKind::Pig | EntityModelKind::ColdPig => registry(&session.pig_variants, false),
        EntityModelKind::PigBaby => registry_baby(&session.pig_variants, false),
        EntityModelKind::Chicken | EntityModelKind::ColdChicken => {
            registry(&session.chicken_variants, false)
        }
        EntityModelKind::ChickenBaby => registry_baby(&session.chicken_variants, false),
        // M350 — the zombie nautilus's sheet, on either mesh.
        EntityModelKind::ZombieNautilus | EntityModelKind::ZombieNautilusCoral => {
            registry(&session.zombie_nautilus_variants, false)
        }
        EntityModelKind::Axolotl => {
            rewo_data::mob_variants::variant_id(rewo_data::mob_variants::axolotl_texture(v))
                .unwrap_or(0)
        }
        EntityModelKind::Llama => {
            rewo_data::mob_variants::variant_id(rewo_data::mob_variants::llama_texture(v))
                .unwrap_or(0)
        }
        // M354 — `BeeRenderer`: angry, then nectar, adult or baby.
        EntityModelKind::Bee | EntityModelKind::BeeBaby => {
            rewo_data::mob_variants::variant_id(rewo_data::mob_variants::bee_texture(
                kind == EntityModelKind::BeeBaby,
                mob_is_angry(session, id),
                session.world.entities.bee_flags(id) & 0x08 != 0,
            ))
            .unwrap_or(0)
        }
        // M347 — `MushroomCowRenderer`: red or brown, adult or calf.
        EntityModelKind::Mooshroom | EntityModelKind::MooshroomBaby => {
            let calf = kind == EntityModelKind::MooshroomBaby;
            rewo_data::mob_variants::variant_id(rewo_data::mob_variants::mooshroom_texture(v, calf))
                .unwrap_or(0)
        }
        // M346 — `RabbitRenderer`: "Toast" first, then the coat.
        EntityModelKind::Rabbit | EntityModelKind::RabbitBaby => {
            let toast = session.world.entities.custom_name(id) == Some("Toast");
            let kit = kind == EntityModelKind::RabbitBaby;
            rewo_data::mob_variants::variant_id(&rewo_data::mob_variants::rabbit_texture(
                v, kit, toast,
            ))
            .unwrap_or(0)
        }
        // M345 — `FoxRenderer`'s idle sheet for the coat, adult or kit.
        EntityModelKind::Fox | EntityModelKind::FoxBaby => {
            let kit = kind == EntityModelKind::FoxBaby;
            rewo_data::mob_variants::variant_id(rewo_data::mob_variants::fox_texture(v, kit))
                .unwrap_or(0)
        }
        // M344 — `ParrotRenderer.getVariantTexture`.
        EntityModelKind::Parrot => {
            rewo_data::mob_variants::variant_id(rewo_data::mob_variants::parrot_texture(v))
                .unwrap_or(0)
        }
        // M339 — the same low byte on the foal sheets.
        EntityModelKind::HorseBaby => {
            rewo_data::mob_variants::variant_id(rewo_data::mob_variants::horse_baby_texture(v))
                .unwrap_or(0)
        }
        // M338 — the same id on `BABY_TEXTURES` (this is the swapped kind).
        EntityModelKind::LlamaBaby => {
            rewo_data::mob_variants::variant_id(rewo_data::mob_variants::llama_baby_texture(v))
                .unwrap_or(0)
        }
        EntityModelKind::Horse => {
            rewo_data::mob_variants::variant_id(rewo_data::mob_variants::horse_texture(v))
                .unwrap_or(0)
        }
        // M68. The one variant here that is not a *base* texture swap: it
        // moves the fish's **pattern layer** onto one of its shape's six
        // sheets, leaving the body slot alone. The shape itself is already
        // decided (it chose this `kind`), so unpacking again is just reading
        // the other field of the same int.
        EntityModelKind::TropicalFish | EntityModelKind::TropicalFishLarge => {
            let f = rewo_data::mob_variants::FishVariant::unpack(v);
            rewo_data::mob_variants::fish_pattern_variant(f.base, f.pattern)
        }
        _ => 0,
    }
}

// -- the quad-leash (the happy-ghast holder branch of M170's gather) ---------
//
// `EntityRenderer.extractLeashStates`'s quad branch: when the HOLDER
// supports it (`supportQuadLeashAsHolder` — only the happy ghast overrides
// it true) AND the leashed entity does (`supportQuadLeash`), the rope is
// FOUR ribbons, each from one of the leashed entity's four shoulder points
// to one of the holder's four underside points, `slack = false` throughout.

/// `Leashable.createQuadLeashOffsets` — the four points are
/// `(±leftRight·w, height·h, ±frontBack·w + front·w)`, ordered
/// back-left, front-left(-z), front-right, back-right(+z).
pub(crate) fn quad_leash_offsets(w: f64, h: f64, shape: [f64; 4]) -> [[f64; 3]; 4] {
    let (front, front_back, left_right, height) = (shape[0], shape[1], shape[2], shape[3]);
    let fo = front * w;
    let fb = front_back * w;
    let lr = left_right * w;
    let hh = height * h;
    [
        [-lr, hh, fb + fo],
        [-lr, hh, -fb + fo],
        [lr, hh, -fb + fo],
        [lr, hh, fb + fo],
    ]
}

/// `supportQuadLeash()`'s per-class `getQuadLeashOffsets` shapes, as
/// `(front, front_back, left_right, height)` multipliers — verbatim from
/// the decompile. Class knowledge translated to names:
/// * `AbstractHorse` TRUE with `(0.04, 0.52, 0.23, 0.87)`
///   (`AbstractHorse.java:205`) — horse, skeleton horse.
/// * `AbstractChestedHorse` overrides to `(0.04, 0.41, 0.18, 0.73)`
///   (`:178`) — donkey, mule; `ZombieHorse` overrides to the same (`:192`).
///   **Llama is the named exception**: it overrides `supportQuadLeash`
///   back to FALSE (`Llama.java:418`), and trader_llama descends from it.
/// * `Camel` overrides to `(0.02, 0.48, 0.25, 0.82)` (`Camel.java:414`);
///   camel husk extends it.
/// * `AbstractBoat` TRUE with `(0.0, 0.64, 0.382, 0.88)` — the whole
///   generated boat set, rafts included.
/// * `Sniffer` overrides to `(-0.01, 0.63, 0.38, 1.15)` (`Sniffer.java:159`).
///
/// (Before 2026-10-18 every horse-line species and the camels took the
/// `AbstractHorse` shape and the sniffer the interface default; the review
/// of the M194-M203 run caught it.)
pub(crate) fn quad_leash_shape(name: &str) -> Option<[f64; 4]> {
    const HORSE: [f64; 4] = [0.04, 0.52, 0.23, 0.87];
    const CHESTED: [f64; 4] = [0.04, 0.41, 0.18, 0.73];
    const CAMEL: [f64; 4] = [0.02, 0.48, 0.25, 0.82];
    const BOAT: [f64; 4] = [0.0, 0.64, 0.382, 0.88];
    const SNIFFER: [f64; 4] = [-0.01, 0.63, 0.38, 1.15];
    if rewo_data::entity_classes::ABSTRACT_BOAT.contains(&name) {
        return Some(BOAT);
    }
    match name {
        "minecraft:horse" | "minecraft:skeleton_horse" => Some(HORSE),
        "minecraft:donkey" | "minecraft:mule" | "minecraft:zombie_horse" => Some(CHESTED),
        "minecraft:camel" | "minecraft:camel_husk" => Some(CAMEL),
        "minecraft:sniffer" => Some(SNIFFER),
        _ => None,
    }
}

/// `HappyGhast.getQuadLeashHolderOffsets` =
/// `createQuadLeashOffsets(this, -0.03125, 0.4375, 0.46875, 0.03125)`.
/// The ghast is the only `supportQuadLeashAsHolder` override in vanilla
/// (`Entity`'s default is false).
pub(crate) const GHAST_HOLDER_SHAPE: [f64; 4] = [-0.03125, 0.4375, 0.46875, 0.03125];

/// The leash ribbons to draw this frame (M170), one per leashed entity that
/// has a resolvable holder. Faithful to `EntityRenderer`'s single-leash branch:
/// `start = entity.pos + leashOffset.yRot(-bodyYaw)`, `end =
/// holder.getRopeHoldPosition`, `slack = true` (the `LeashState` default the
/// branch never overrides), light sampled at each end's eye. A happy-ghast
/// holder with a quad-leashable leashed entity takes the M185 quad branch
/// instead (four `slack = false` ribbons).
///
/// Eye height is `height * 0.85` — the `EntityDimensions.of` default. Vanilla
/// carries per-type overrides (a player's is 1.62, not 1.53); the leashables
/// are mobs where the default is exact or near it, so the divergence is a small
/// vertical shift of one rope end, documented rather than tabulated.
/// Every leash rope this frame draws, and the `(leashed, holder)` pairs that
/// took M185's quad branch (four ropes; read by render-check r72).
pub(super) fn collect_leashes(
    session: &PlaySession,
    etypes: &EntityTypes,
    alpha: f32,
    lightmap: &LightmapState,
) -> (Vec<rewo_gpu::leash::LeashVertex>, Vec<(i32, i32)>) {
    // `Vec3.yRot(angle)`: x' = x·cos + z·sin, z' = z·cos - x·sin.
    fn yrot(v: [f64; 3], angle: f32) -> [f64; 3] {
        let (s, c) = (angle.sin() as f64, angle.cos() as f64);
        [v[0] * c + v[2] * s, v[1], v[2] * c - v[0] * s]
    }
    // A holder's `getRopeHoldPosition` and the world point to sample its light
    // at (its eye). Returns `None` when the holder is not tracked.
    let hold = |holder: i32| -> Option<([f64; 3], [f64; 3])> {
        if Some(holder) == session.player_id {
            let feet = [session.player.x, session.player.y, session.player.z];
            let eye = session.player.eye_y();
            return Some((
                [feet[0], feet[1] + (eye - feet[1]) * 0.7, feet[2]],
                [feet[0], eye, feet[2]],
            ));
        }
        let h = session.world.entities.get(holder)?;
        let pos = h.render_pos(alpha);
        if etypes.name(h.type_id) == Some("minecraft:leash_knot") {
            // LeashFenceKnotEntity.getRopeHoldPosition = pos + (0, 0.2, 0).
            Some((
                [pos[0], pos[1] + 0.2, pos[2]],
                [pos[0], pos[1] + 0.2, pos[2]],
            ))
        } else {
            let (_, hh) = etypes.dimensions(h.type_id);
            let heye = (hh * 0.85) as f64;
            Some((
                [pos[0], pos[1] + heye * 0.7, pos[2]],
                [pos[0], pos[1] + heye, pos[2]],
            ))
        }
    };

    let mut verts: Vec<rewo_gpu::leash::LeashVertex> = Vec::new();
    let mut quads: Vec<(i32, i32)> = Vec::new();
    for (id, e) in session.world.entities.iter() {
        let Some(holder) = session.world.entities.leash_holder(id) else {
            continue;
        };
        // The quad branch needs the holder's own name, dims, pos and yaw —
        // fetched once here, with the player-holder special case falling
        // to the single-rope path (the player is never a quad holder).
        let holder_entity = if Some(holder) == session.player_id {
            None
        } else {
            session.world.entities.get(holder)
        };
        let holder_is_quad_ghast =
            holder_entity.is_some_and(|h| etypes.name(h.type_id) == Some("minecraft:happy_ghast"));
        let leashed_shape = etypes.name(e.type_id).and_then(quad_leash_shape);
        let quad = holder_is_quad_ghast && leashed_shape.is_some();

        let Some((end, end_eye)) = hold(holder) else {
            continue;
        };
        let pos = e.render_pos(alpha);
        let (w, ht) = etypes.dimensions(e.type_id);
        let eye = (ht * 0.85) as f64;
        // Light at each eye (vanilla samples `getEyePosition`, not the rope
        // end), carried as PACKED components - the ribbon interpolates block
        // and sky separately and evaluates the lightmap curve per vertex.
        // One pair serves all four quad ropes (vanilla samples once too).
        let start_packed = session.world.light_at(
            pos[0].floor() as i32,
            (pos[1] + eye).floor() as i32,
            pos[2].floor() as i32,
        );
        let end_packed = session.world.light_at(
            end_eye[0].floor() as i32,
            end_eye[1].floor() as i32,
            end_eye[2].floor() as i32,
        );

        if let (Some(shape), Some(h)) = (leashed_shape.filter(|_| quad), holder_entity) {
            // The quad branch: four `slack = false` ribbons from the
            // leashed entity's four shoulder points to the ghast's four
            // underside points, each rotated by its own body yaw.
            let (hw, hht) = etypes.dimensions(h.type_id);
            let holder_pos = h.render_pos(alpha);
            let entity_yrot = -e.yaw.to_radians();
            let roper_yrot = -h.yaw.to_radians();
            let leashed_points = quad_leash_offsets(w as f64, ht as f64, shape);
            let holder_points = quad_leash_offsets(hw as f64, hht as f64, GHAST_HOLDER_SHAPE);
            for i in 0..4 {
                let offset = yrot(leashed_points[i], entity_yrot);
                let start = [pos[0] + offset[0], pos[1] + offset[1], pos[2] + offset[2]];
                let hoff = yrot(holder_points[i], roper_yrot);
                let rope_end = [
                    holder_pos[0] + hoff[0],
                    holder_pos[1] + hoff[1],
                    holder_pos[2] + hoff[2],
                ];
                verts.extend(rewo_gpu::leash::build_ribbon(
                    start,
                    rope_end,
                    false,
                    start_packed,
                    end_packed,
                    &|b, s| rewo_world::lightmap::sample(b, s, lightmap),
                ));
            }
            quads.push((id, holder));
            continue;
        }

        // leashOffset = (0, eyeHeight, bbWidth * 0.4), rotated by -bodyYaw.
        let offset = yrot([0.0, eye, (w * 0.4) as f64], -e.yaw.to_radians());
        let start = [pos[0] + offset[0], pos[1] + offset[1], pos[2] + offset[2]];
        verts.extend(rewo_gpu::leash::build_ribbon(
            start,
            end,
            true,
            start_packed,
            end_packed,
            &|b, s| rewo_world::lightmap::sample(b, s, lightmap),
        ));
    }
    (verts, quads)
}

pub(super) fn collect_entities<'a>(
    session: &'a PlaySession,
    etypes: &EntityTypes,
    alpha: f32,
    gestures: &mut GestureTracker,
    now: f32,
    skins: &SkinRegistry,
    lightmap: &LightmapState,
    spears: &rewo_data::item_tags::ItemTag,
    bow_item: Option<i32>,
    item_names: &'a rewo_data::items::Items,
    // Armour layer definitions, for resolving what each entity wears into an
    // atlas key (M46).
    equipment: &'a rewo_data::equipment::EquipmentAssets,
    // Where this frame's trim sprites landed in the pool (M48).
    trim_slots: &std::collections::HashMap<String, (u32, u32)>,
    // The resource pack's ETF random-entity rules (M52). Empty without a pack,
    // in which case every entity keeps its vanilla texture.
    etf: &rewo_data::etf::EtfPack,
    // `Minecraft.getInstance().gui.hud.isHidden()` — F1 (M70). Suppresses
    // every floating label on an un-teamed entity, and nothing on a teamed
    // one, because the team switch returns first.
    hud_hidden: bool,
    // `entityRenderDispatcher.crosshairPickEntity` (M73) — resolved once by
    // the caller, because vanilla resolves it once: `Minecraft.pick` runs one
    // raycast and `EntityRenderDispatcher.prepare` hands the single result to
    // every renderer.
    crosshair_pick: Option<i32>,
    // M273 — player heads' own skins, for a worn head (`None`: Steve).
    head_skins: Option<&'a HeadSkins>,
) -> Vec<EntityDraw<'a>> {
    let player_color = linear_rgb(0xE5, 0xB8, 0xC5); // accent rose
    let mob_color = linear_rgb(0x9A, 0x80, 0x87); // text mauve
                                                  // M59: the health bar's two gates that need session-wide state — the
                                                  // attribute registry (max health, name-tag distance) and the camera
                                                  // position `EntityRenderDispatcher.distanceToSqr` measures from, which is
                                                  // the *eye*, not the feet.
    let attr_reg = session.attribute_registry.as_deref();
    let eye = player_eye(session);
    // M70: the viewer half of the label predicate — camera entity, F1, game
    // mode and the viewer's own team. Read once per frame, not per entity.
    let mut viewer = LabelViewer::from_session(session, hud_hidden);
    viewer.crosshair_pick = crosshair_pick;
    // Headless-only verification knob: `REWO_FORCE_LIMB=swing,amount`
    // pins every player's walk pose so a still-target PNG can prove the
    // limb-swing mechanism deterministically (a live walker's phase at
    // capture time is timing-dependent). One-shot; zero-cost when unset.
    let force_limb: Option<(f32, f32)> = crate::knobs::var("REWO_FORCE_LIMB").ok().and_then(|s| {
        let mut it = s.split(',');
        Some((
            it.next()?.trim().parse().ok()?,
            it.next()?.trim().parse().ok()?,
        ))
    });
    // Headless-only knob: `REWO_FORCE_HEAD=<degrees>` cranks every mob's head
    // yaw to body-yaw + this offset, so a PNG can prove head-look turns the
    // head independently of the body without depending on live server AI.
    let force_head: Option<f32> = crate::knobs::var("REWO_FORCE_HEAD")
        .ok()
        .and_then(|s| s.trim().parse().ok());
    // Headless-only knob: `REWO_FORCE_GESTURE=<name>[,<age_s>]` pins every
    // gesture-rigged mob into that state (mobshot names, e.g.
    // "warden_roar,1.5") — deterministic gesture PNGs without server AI.
    let force_gesture: Option<(rewo_gpu::mobs::Gesture, f32)> =
        crate::knobs::var("REWO_FORCE_GESTURE").ok().and_then(|s| {
            let mut it = s.split(',');
            let g = rewo_gpu::mobs::Gesture::from_name(it.next()?.trim())?;
            let age = it.next().and_then(|a| a.trim().parse().ok()).unwrap_or(0.0);
            Some((g, age))
        });
    // M312 — every entity something is leashed to (`isLeashHolder`).
    let leash_holders: std::collections::HashSet<i32> = session
        .world
        .entities
        .iter()
        .filter_map(|(id, _)| session.world.entities.leash_holder(id))
        .collect();
    let mut out = Vec::new();
    for (id, e) in session.world.entities.iter() {
        // M194: an item frame has no body — `ItemFrameRenderer` draws its
        // frame model and item, which `collect_item_frames` emits. Without
        // this it fell through to the capsule.
        if frame_kind(etypes, e.type_id).is_some() {
            continue;
        }
        // M221 — the vehicle the local player rides, when Rewo has no model
        // for it (boats had none until M223): its placeholder capsule is
        // centred on the vehicle and encloses the rider's camera, hiding the
        // whole world. Vanilla draws the vehicle's model around the rider;
        // with no model, not drawing it is the honest stand-in.
        if session.local_vehicle() == Some(id)
            && rewo_gpu::mobs::kind_for_entity_name(etypes.name(e.type_id).unwrap_or(""))
                == rewo_gpu::mobs::EntityModelKind::Capsule
        {
            continue;
        }
        let p = e.render_pos(alpha);
        let name = etypes.name(e.type_id).unwrap_or("");
        let is_player = e.type_id == etypes.player_id;
        // M308 — a mannequin is an `Avatar` too (`AvatarRenderer`, the
        // player's model and skin rules); its skin is keyed by its own UUID.
        let is_avatar = is_player || name == "minecraft:mannequin";
        // A player with a resolved skin wears it (slim → the Alex model);
        // otherwise the default wide Steve until one is uploaded.
        let player_skin = if is_avatar { skins.get(&e.uuid) } else { None };
        let kind = if is_avatar {
            match player_skin {
                Some(ps) if ps.slim => EntityModelKind::PlayerSlim,
                _ => EntityModelKind::Player,
            }
        } else {
            rewo_gpu::mobs::kind_for_entity_name(name)
        };
        // M68: one wire name, two meshes.
        let fish = (kind == EntityModelKind::TropicalFish).then(|| {
            rewo_data::mob_variants::FishVariant::unpack(
                session.world.entities.variant(id).unwrap_or(0),
            )
        });
        let kind = fish.map_or(kind, fish_kind);
        // M348 — `CowRenderer.models`: the variant's `model` picks the adult
        // mesh (every calf is one model).
        let kind = farm_model_kind(
            kind,
            variant_def(kind, id, session).and_then(|d| d.model.as_deref()),
        );
        // M307: and the sulfur cube's small model for a baby.
        let kind = baby_model_kind(kind, session.world.entities.is_baby(id));
        // M24b: a dropped stack. `ItemEntity.DATA_ITEM` arrives as metadata
        // index 8 with the ITEM_STACK serializer; an entity with one renders
        // as the item and nothing else, so the model kind is never consulted.
        // Gated on the type actually being `minecraft:item`: nothing else puts
        // an ITEM_STACK at slot 8, but the gate makes that explicit rather
        // than relying on it.
        let is_item_entity = Some(e.type_id) == etypes.id_of("minecraft:item");
        let mut ground_stack = is_item_entity
            .then(|| session.world.entities.item_stack(id))
            .flatten();
        // M240 — the thrown items `ThrownItemRenderer` / `FireworkEntityRenderer`
        // draw (and the bobber, `FishingHookRenderer`'s sprite): the item off
        // slot 8 (`getItem()`), else the type's default item, scaled and
        // facing the camera. A rocket attached to someone is not drawn
        // (`shouldRender` is false while attached).
        let thrown = thrown_item_of(name);
        if thrown.is_some()
            && name == "minecraft:firework_rocket"
            && session.world.entities.attached_to(id).is_some()
        {
            continue;
        }
        // M250 — an arrow or a thrown trident is a model on the block-entity
        // path (`collect_projectiles`), not a capsule here.
        if projectile_model_of(name).is_some() {
            continue;
        }
        // M253 — a falling block or primed TNT is its block model on the
        // block-entity path (`collect_carried_blocks`).
        if matches!(name, "minecraft:falling_block" | "minecraft:tnt") {
            continue;
        }
        // M254 — a painting is its variant's model (`collect_paintings`).
        if name == "minecraft:painting" {
            continue;
        }
        // M255 — entities vanilla draws no body for (an interaction box, a
        // marker, an effect cloud's particles) or that are not a body at all
        // (the displays: a text display is world text, `collect_text_displays`;
        // an item display its item's model, `collect_item_displays`; a block
        // display its block's, `collect_block_displays`). Never a capsule.
        if matches!(
            name,
            "minecraft:text_display"
                | "minecraft:end_crystal"
                | "minecraft:wither_skull"
                | "minecraft:llama_spit"
                | "minecraft:shulker_bullet"
                | "minecraft:evoker_fangs"
                | "minecraft:block_display"
                | "minecraft:item_display"
                | "minecraft:interaction"
                | "minecraft:marker"
                | "minecraft:area_effect_cloud"
        ) {
            continue;
        }
        let mut thrown_model: Option<&'static str> = None;
        let mut billboard = thrown.map(|(default, scale, _)| {
            if default.starts_with("rewo:") {
                thrown_model = Some(default);
            } else if ground_stack.is_none() {
                ground_stack = session
                    .world
                    .entities
                    .item_stack(id)
                    .or_else(|| item_names.id(default).map(|i| (i, 1, false)));
            }
            rewo_gpu::entities::Billboard {
                cam_yaw: session.player.yaw,
                cam_pitch: session.player.pitch,
                scale,
                tint: None,
            }
        });
        // M251 — an experience orb: `ExperienceOrbRenderer`'s camera-facing
        // icon (`getIcon()` of its `DATA_VALUE`) at scale 0.3, lifted 0.1,
        // in its own pulsing colour `((sin(r) + 1) / 2, 1, (sin(r + 4π/3) +
        // 1) / 10)`, `r = age / 2`, unshaded and lit 7 brighter. Recorded:
        // the age is the world clock offset by the entity id (Rewo keeps no
        // per-entity tick count), so orbs pulse out of step, not in vanilla's
        // exact phase.
        let is_orb = name == "minecraft:experience_orb";
        if is_orb {
            use rewo_data::block_entity_models::{xp_orb_icon, XP_ORB_MODELS};
            let value = session.world.entities.variant(id).unwrap_or(0);
            thrown_model = Some(XP_ORB_MODELS[xp_orb_icon(value)]);
            let r = (session.game_time() as f32 + alpha + (id & 0xff) as f32 * 7.0) / 2.0;
            let srgb = |c: f32| -> f32 {
                // The colour is a vertex colour vanilla multiplies in gamma
                // space; this pass is linear.
                if c <= 0.040_449_936 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            billboard = Some(rewo_gpu::entities::Billboard {
                cam_yaw: session.player.yaw,
                cam_pitch: session.player.pitch,
                scale: 0.3,
                tint: Some([
                    srgb((r.sin() + 1.0) * 0.5),
                    1.0,
                    srgb((((r + std::f32::consts::PI * 4.0 / 3.0).sin()) + 1.0) * 0.1),
                ]),
            });
        }
        let thrown_fullbright = thrown.is_some_and(|(_, _, fullbright)| fullbright);

        // Slime / magma-cube size (metadata index 18 since M305, vanilla
        // default 1, which the server never sends; the model + bbox scale
        // linearly by it). Our slime model is baked at the size-2 look, so
        // scale_mul = size/2 (size 2 → 1.0).
        // M307 — the sulfur cube is a cube mob too, its model carrying the
        // renderer's own downscale, so it scales by the whole size; its
        // bounds are `getDefaultDimensions`, the type's times the size.
        let sulfur = matches!(
            kind,
            EntityModelKind::SulfurCube | EntityModelKind::SulfurCubeSmall
        );
        let cube_size = (matches!(kind, EntityModelKind::Slime | EntityModelKind::MagmaCube)
            || sulfur)
            .then(|| session.world.entities.size(id).unwrap_or(1).clamp(1, 32));
        let (mut w, mut h) = match cube_size {
            Some(sz) if sulfur => {
                let (tw, th) = etypes.dimensions(e.type_id);
                (tw * sz as f32, th * sz as f32)
            }
            Some(sz) => (0.51 * sz as f32, 0.51 * sz as f32),
            None => etypes.dimensions(e.type_id),
        };
        let mut scale_mul =
            cube_size.map_or(1.0, |sz| if sulfur { sz as f32 } else { sz as f32 / 2.0 });
        // Baby mobs (ageable / zombie families) render at ~half scale.
        // Uniform approximation — vanilla keeps the head proportionally
        // larger (a per-part transform), deferred. M307 — not the baby
        // sulfur cube, whose own model is the small one (`getAgeScale`
        // halves only its bounds).
        // M314 / M315 — nor any other baby drawn by its own model
        // (`baby_scales`).
        if !is_player && session.world.entities.is_baby(id) {
            let (render, bounds) = baby_scales(kind);
            scale_mul *= render;
            w *= bounds;
            h *= bounds;
        }
        // M238 — the armour stand's client flags: 0x01 small (the half-scale
        // `ARMOR_STAND_SMALL` layer, `MeshTransformer.scaling(0.5)`), 0x04
        // show arms, 0x08 no base plate.
        let stand_flags = if kind == EntityModelKind::ArmorStand {
            session.world.entities.armor_stand_flags(id)
        } else {
            0
        };
        if stand_flags & 0x01 != 0 {
            scale_mul *= 0.5;
            w *= 0.5;
            h *= 0.5;
        }
        // M271 — `isBodyVisible() = !isInvisible()` for a living entity,
        // drawn only for a spectator viewer (translucent in vanilla; Rewo has
        // no translucent mob pass, so a spectator sees it whole).
        let body_hidden = session.world.entities.is_invisible(id)
            && session
                .entity_classes
                .as_deref()
                .is_some_and(|c| c.is_living(e.type_id))
            && !session.own_game_mode().is_some_and(|g| g.is_spectator());
        // M277 — `CreeperRenderer`'s `state.swelling`.
        let creeper_swelling = (name == "minecraft:creeper")
            .then(|| session.world.entities.creeper_swelling(id, alpha));
        // M288 — a wither's invulnerable phase (`DATA_ID_INV`).
        let wither_inv = (name == "minecraft:wither").then(|| {
            wither_invulnerable_ticks(session.world.entities.wither_invulnerable(id), alpha)
        });
        let stand_pose = session.world.entities.armor_stand_pose(id);
        let stand_parts = rewo_gpu::mobs::StandParts {
            arms: stand_flags & 0x04 != 0,
            base_plate: stand_flags & 0x08 == 0,
            // M270 — the six poses, vanilla's default where none came.
            pose: std::array::from_fn(|i| {
                stand_pose[i].unwrap_or(rewo_gpu::mobs::STAND_DEFAULT_POSE[i])
            }),
        };
        let (limb_swing, limb_amount) = force_limb.unwrap_or_else(|| e.limb());
        // Gesture + wire-event rigs: pose/state → rig, timed from the observed
        // change; one-shot events (warden attack/sonic boom, armadillo peek)
        // resolved against their receipt ticks with the exact vanilla ownership
        // rules (roar-stop, peek re-clock). A forced gesture (headless knob)
        // bypasses the tracker and carries no events.
        let (gesture, events) = if let Some(fg) = force_gesture {
            (Some(fg), [None; rewo_gpu::mobs::ModelEvent::COUNT])
        } else {
            use rewo_world::entities::EntityEvent;
            let ents = &session.world.entities;
            resolve_mob_anim(
                kind,
                ents.pose(id),
                ents.gesture_state(id),
                ents.event_start(id, EntityEvent::WardenAttack),
                ents.event_start(id, EntityEvent::WardenSonicBoom),
                ents.event_start(id, EntityEvent::ArmadilloPeek),
                gestures,
                id,
                now,
                session.ticks as i64,
                alpha,
            )
        };
        // Armadillo shell swap (vanilla `shouldHideInShell` per state):
        // ROLLING balls up after 5 ticks, SCARED always, UNROLLING opens
        // at tick 26.
        let shell = kind == EntityModelKind::Armadillo
            && match gesture {
                Some((rewo_gpu::mobs::Gesture::ArmadilloRoll, age)) => age > 0.25,
                Some((rewo_gpu::mobs::Gesture::ArmadilloScared, _)) => true,
                Some((rewo_gpu::mobs::Gesture::ArmadilloUnroll, age)) => age < 1.3,
                _ => false,
            };
        // Allay dance (index-16 `DATA_DANCING` metadata → client counters).
        // Shared with the `danceshot` oracle so the kind gate + counter mapping
        // are exercised by the gate and can't regress here silently.
        let allay_dance = resolve_allay_dance(kind, &session.world.entities, id, alpha);
        // Combat swing (`ClientboundAnimatePacket` → the swing clock). Shared
        // with the `swingshot` oracle for the same reason as the dance above.
        let attack = resolve_attack_anim(&session.world.entities, id, alpha);
        // The hold pose the swing is layered onto. Same sharing rule as above.
        let arm_poses = resolve_arm_poses(
            &session.world.entities,
            id,
            kind,
            spears,
            bow_item,
            crossbow_item_id(),
        );
        // M20: the synced mob state the undead / skeleton / illager rigs read.
        let mut mob = resolve_mob_combat(&session.world.entities, id, kind, bow_item);
        // M354 — the bee's mood and its stinger (the anger is a deadline on
        // the world clock, so it is read here, where the clock is).
        if matches!(kind, EntityModelKind::Bee | EntityModelKind::BeeBaby) {
            mob.bee_angry = mob_is_angry(session, id);
            mob.bee_stung = session.world.entities.bee_flags(id) & 0x04 != 0;
        }
        // M387 — the axolotl's factors: in water, moving (the tick's
        // horizontal displacement), playing dead (`DATA_PLAYING_DEAD`).
        if kind == EntityModelKind::Axolotl {
            let (a, b) = (e.render_pos(0.0), e.render_pos(1.0));
            mob.axolotl_moving = (b[0] - a[0]).powi(2) + (b[2] - a[2]).powi(2) > 1.0e-7;
            mob.axolotl_in_water =
                session
                    .world
                    .is_water_at_point(b[0], b[1] + 0.2, b[2], &session.water_states);
            mob.axolotl_playing_dead = session.world.entities.axolotl_playing_dead(id);
        }
        // M381 — a seated rider (`isPassenger` / `isRiding`), for the kinds
        // whose model has the pose.
        mob.riding = rewo_gpu::mobs::sits_when_riding(kind)
            && session.world.entities.vehicle_of(id).is_some();
        // M379 — the witch's nose: its id sets the wobble rate, and a held
        // item (her potion) lifts it.
        if kind == EntityModelKind::Witch {
            mob.witch_nose_rate = 0.01 * id.rem_euclid(10) as f32;
            mob.witch_holding = !matches!(
                session
                    .world
                    .entities
                    .hand_item(id, rewo_world::entities::InteractionHand::MainHand),
                rewo_world::entities::HandItem::Empty
            );
        }
        // M378 — `ParrotModel.getPose`'s SITTING: the tamed parrot's sitting
        // pose flag.
        if kind == EntityModelKind::Parrot {
            mob.sitting = session.world.entities.is_in_sitting_pose(id);
        }
        // M377 — `CamelRenderer.getJumpCooldown`: the cooldown less the
        // partial tick while it runs.
        if matches!(
            kind,
            EntityModelKind::Camel | EntityModelKind::CamelHusk | EntityModelKind::CamelBaby
        ) {
            let cd = e.dash_cooldown();
            mob.camel_jump_cooldown = if cd > 0 { cd as f32 - alpha } else { 0.0 };
        }
        // M371 — `DolphinRenderState.isMoving`: the horizontal movement over
        // the tick (`getDeltaMovement().horizontalDistanceSqr() > 1e-7`).
        if matches!(
            kind,
            EntityModelKind::Dolphin | EntityModelKind::DolphinBaby
        ) {
            let (a, b) = (e.render_pos(0.0), e.render_pos(1.0));
            mob.dolphin_moving = (b[0] - a[0]).powi(2) + (b[2] - a[2]).powi(2) > 1.0e-7;
        }
        // M369 — `AllayRenderState.holdingAnimationProgress`, steady state:
        // 1 while the main hand holds anything.
        if kind == EntityModelKind::Allay {
            mob.allay_holding = match session
                .world
                .entities
                .hand_item(id, rewo_world::entities::InteractionHand::MainHand)
            {
                rewo_world::entities::HandItem::Empty => 0.0,
                _ => 1.0,
            };
        }
        // M367 — `HoglinRenderState.attackAnimationRemainingTicks`: 10 at
        // entity event 4, one less each client tick (an int, no partial).
        if matches!(
            kind,
            EntityModelKind::Hoglin
                | EntityModelKind::Zoglin
                | EntityModelKind::HoglinBaby
                | EntityModelKind::ZoglinBaby
        ) {
            mob.headbutt = session
                .world
                .entities
                .event_start(id, rewo_world::entities::EntityEvent::HoglinAttack)
                .map_or(0.0, |start| {
                    headbutt_factor(10 - (session.ticks as i64 - start))
                });
        }
        // M366 — `StriderRenderState.isRidden`: a ridden strider's body stops
        // following its look.
        if matches!(
            kind,
            EntityModelKind::Strider | EntityModelKind::StriderBaby
        ) {
            mob.ridden = !session.mounts.passengers(id).is_empty();
        }
        // M365 — `TurtleRenderer.extractRenderState`: the egg flags and
        // `isOnLand = !isInWater() && onGround()`. Rewo keeps no `onGround`
        // for other entities, so a turtle out of the water is on land (one
        // in the air over land is rare and brief).
        if matches!(kind, EntityModelKind::Turtle | EntityModelKind::TurtleBaby) {
            let (egg, laying) = session.world.entities.turtle_flags(id);
            mob.turtle_egg = egg && kind == EntityModelKind::Turtle;
            mob.turtle_laying = laying;
            let p = e.render_pos(alpha);
            mob.turtle_on_land =
                !session
                    .world
                    .is_water_at_point(p[0], p[1] + 0.2, p[2], &session.water_states);
        }
        // M363 — `Boat.isUnderWater()`: the eye (0.85 of the boat's 0.5625
        // height) in water; a sunk boat draws no water patch.
        if matches!(kind, EntityModelKind::Boat | EntityModelKind::ChestBoat) {
            let p = e.render_pos(alpha);
            mob.under_water = session.world.is_eye_under_water(
                p[0],
                p[1] + 0.5625 * 0.85,
                p[2],
                &session.water_states,
            );
        }
        // M358 — a grazing sheep: the eat clock from entity event 10's
        // receipt tick, and the age scale its head drop scales by.
        if matches!(kind, EntityModelKind::Sheep | EntityModelKind::SheepBaby) {
            let ents = &session.world.entities;
            mob.sheep_eat = ents
                .event_start(id, rewo_world::entities::EntityEvent::SheepEat)
                .and_then(|start| sheep_eat_pose(40 - (session.ticks as i64 - start), alpha));
            mob.age_scale = if kind == EntityModelKind::SheepBaby {
                0.5
            } else {
                1.0
            };
        }
        // M357 — a cat's (or kitten's) sitting pose.
        if matches!(kind, EntityModelKind::Cat | EntityModelKind::CatBaby) {
            mob.sitting = session.world.entities.is_in_sitting_pose(id);
            mob.age_scale = if kind == EntityModelKind::CatBaby {
                0.5
            } else {
                1.0
            };
        }
        // M355 — the wolf's anger and `getTailAngle()` (a tame wolf's tail
        // by its health, when both its health and its max health are known).
        if matches!(kind, EntityModelKind::Wolf | EntityModelKind::WolfBaby) {
            let ents = &session.world.entities;
            let health = attr_reg
                .and_then(|r| {
                    rewo_world::attributes::resolve(
                        ents.attributes(id),
                        etypes.name(e.type_id),
                        "max_health",
                        r,
                    )
                })
                .filter(|(_, s)| *s == rewo_world::attributes::Source::Synced)
                .map(|(max, _)| (ents.death_state(id).health, max as f32));
            mob.wolf_angry = mob_is_angry(session, id);
            // M356 — `TamableAnimal.isInSittingPose()` and the age scale.
            mob.sitting = ents.is_in_sitting_pose(id);
            mob.age_scale = if kind == EntityModelKind::WolfBaby {
                0.5
            } else {
                1.0
            };
            mob.wolf_tail = Some(wolf_tail_angle(mob.wolf_angry, ents.is_tame(id), health));
        }
        // M70: the label-visibility predicate, resolved once and consumed by
        // both the nametag and the health bar so the two cannot disagree.
        // `EntityRenderDispatcher.distanceToSqr` measures from the camera,
        // which is the *eye*, not the feet.
        let dx = p[0] - eye.x as f64;
        let dy = p[1] - eye.y as f64;
        let dz = p[2] - eye.z as f64;
        let label = resolve_label_inputs(
            session,
            id,
            etypes.name(e.type_id),
            attr_reg,
            is_player,
            dx * dx + dy * dy + dz * dz,
            &viewer,
        );
        let (label_name, label_health) = resolve_labels(
            &session.world.entities,
            id,
            etypes.name(e.type_id),
            attr_reg,
            &label,
            // A player shows their profile name; anything else its metadata
            // custom name.
            if is_player {
                session.world.entities.name_of(e.uuid)
            } else {
                session.world.entities.custom_name(id)
            },
        );
        // M386 — `EnderDragonRenderer.submit` is not the living transform:
        // `rotY(−yr)` (the generic draw's `180 − yaw`, so the dragon drew
        // facing backward until M386) then a block forward,
        // `translate(0, 0, 1)` in the turned frame.
        let (draw_yaw, forward) = dragon_frame(kind, e.render_yaw(alpha));
        out.push(EntityDraw {
            pos: [
                p[0] as f32 + forward[0],
                p[1] as f32 + if is_orb { 0.1 } else { 0.0 },
                p[2] as f32 + forward[1],
            ],
            width: w,
            height: h,
            color: if is_player { player_color } else { mob_color },
            // M70: both floating labels now hang off one predicate, resolved
            // together by `resolve_labels` so they cannot disagree. *Whether*
            // either is drawn is `shouldShowName`, not the mere existence of a
            // string — which is all it used to be.
            name: label_name,
            health: label_health,
            kind,
            // M361 — the frame's rotations, blended between ticks (M386: the
            // dragon's own frame).
            yaw: draw_yaw,
            // M24: `state.deathTime = entity.deathTime > 0 ? deathTime + partial : 0`.
            death_time: session
                .world
                .entities
                .death_state(id)
                .render_death_time(alpha),
            // M24b: a dropped stack. `Some` makes the renderer draw the item
            // instead of a model — `ItemEntityRenderer` has no body of its own.
            // M289 — a dropped player head in its profile's skin.
            ground_item: thrown_model.or_else(|| {
                let name = ground_stack.and_then(|(i, _, _)| item_names.name(i))?;
                Some(head_skins.map_or(name, |h| {
                    h.item_model(name, session.world.entities.stack_profile(id))
                }))
            }),
            // `ItemStack.hasFoil()` (M45), decoded at the wire because it
            // lives only in the component patch.
            armor: armor_keys(session, id, item_names, equipment, trim_slots),
            // M294 — the body slot's layers (a horse's armour).
            body_armor: body_armor_piece(session, id, kind, item_names, equipment),
            // M295 — the saddle's layers, and its reins while ridden.
            saddle: saddle_piece(session, id, kind, item_names, equipment),
            ridden: session.world.entities.is_vehicle(id),
            // M296 — a donkey's or mule's chests.
            chested: session.world.entities.has_chest(id),
            // M310 — an avatar's outer layers by its customisation mask.
            avatar_hidden: if is_avatar {
                !session.world.entities.model_customisation(id)
            } else {
                0
            },
            // M311 — the wolf armour's cracks.
            body_armor_crack: wolf_armor_crack(session, id, kind, item_names),
            leash_holder: leash_holders.contains(&id),
            held_glint: [
                held_foil(session, id, rewo_world::entities::InteractionHand::MainHand),
                held_foil(session, id, rewo_world::entities::InteractionHand::OffHand),
            ],
            ground_glint: ground_stack.is_some_and(|(_, _, foil)| foil),
            ground_count: ground_stack.map_or(0, |(_, n, _)| n),
            // `ItemEntity.bobOffs` is `random.nextFloat() * 2 * PI`, rolled in
            // the constructor and never sent. Derived from the entity id here
            // so it is stable per entity; there is no server value it could
            // match, because vanilla's own clients each roll their own.
            bob_offset: bob_offset_for(id),
            ground_seed: ground_stack.map_or(0, |(i, _, _)| i),
            // A live item entity turns on the shared clock; only a pickup
            // animation freezes it (M81).
            ground_age: None,
            head_yaw: force_head.map_or(e.render_head_yaw(alpha), |off| e.render_yaw(alpha) + off),
            pitch: e.render_pitch(alpha),
            limb_swing,
            limb_amount,
            gesture,
            events,
            shell,
            stand_parts,
            body_hidden,
            worn_head: worn_head(session, id, name, item_names, head_skins),
            // M277 — a swelling creeper's flash and swell.
            white_overlay: creeper_swelling.map_or(0.0, creeper_white_overlay),
            stretch: match (creeper_swelling, wither_inv) {
                (Some(s), _) => creeper_stretch(s),
                // M288 — and a wither shrunk by its invulnerable phase.
                (_, Some(t)) => wither_stretch(t),
                _ => [1.0, 1.0],
            },
            // M278 — the Glowing effect's outline, in the team's colour.
            outline: glow_colour(session, id),
            // M285 — a charged creeper's swirl.
            // M287 — and a wither at half health or less (`isPowered`).
            powered: (creeper_swelling.is_some() && session.world.entities.creeper_powered(id))
                || (name == "minecraft:wither"
                    && wither_powered(&session.world.entities, id, attr_reg)),
            // M281 — burning: the flames, at the draw's light probe with
            // block light 15.
            on_fire: fire_draw(session, id, name, now).map(|frames| rewo_gpu::entities::FireDraw {
                light: entity_light_boosted(
                    &session.world,
                    p[0],
                    p[1] + h as f64 * 0.85,
                    p[2],
                    lightmap,
                    15,
                ),
                frames,
            }),
            billboard,
            allay_dance,
            attack,
            arm_poses,
            mob,
            // M21: `hasRedOverlay` — the damage flash.
            // `hasRedOverlay = hurtTime > 0 || deathTime > 0` — the whole
            // disjunction as of M24; M21 shipped only the first term.
            hurt: session.world.entities.has_red_overlay(id),
            // M22: what each arm is holding.
            held: resolve_held_items(&session.world.entities, id, item_names, head_skins),
            skin_uv: player_skin.map(|ps| ps.uv),
            scale_mul,
            mount: None,
            anim_id: (id & 0xffff) as f32,
            light: if thrown_fullbright {
                [1.0; 3]
            } else if is_orb {
                entity_light_boosted(&session.world, p[0], p[1], p[2], lightmap, 7)
            } else {
                entity_light(&session.world, p[0], p[1] + h as f64 * 0.85, p[2], lightmap)
            },
            // M52. Vanilla's synched defaults: the warden's tendril countdown
            // rides entity_event 61 and the creaking's glow rides metadata
            // IS_ACTIVE — both resolved below.
            emissive: emissive_state(session, id, kind, now),
            // Two things can move a mob off its baked texture, and vanilla's
            // own wins. M64's variant is what the *server* says this cat or
            // horse is; M57b's ETF rule is a pack randomising the base
            // texture — and a black cat is not drawing that texture at all,
            // so the two never mean the same slot. Vanilla first, ETF only
            // where vanilla has nothing to say.
            variant: match vanilla_variant(kind, id, session) {
                // M288 — the wither's invulnerable sheet, from its ticks.
                _ if wither_inv.is_some_and(wither_shows_invulnerable) => {
                    rewo_data::mob_variants::variant_id(WITHER_INVULNERABLE).unwrap_or(0)
                }
                0 if !etf.is_empty() => etf_variant(etf, kind, e, id, session, cube_size),
                v => v,
            },
            // The sheep's wool colour (`Sheep.DATA_WOOL_ID`). `None` is
            // vanilla's `DyeColor.WHITE` default, which still tints.
            dye: session.world.entities.wool_color(id),
            // …and bit 0x10 of the same byte (M64), which drops the fleece
            // rather than recolouring it.
            sheared: session.world.entities.is_sheared(id),
            // M342 — `WolfRenderer`: `collarColor = isTame() ?
            // getCollarColor() : null`, the collar the server sent or its
            // default red (14).
            collar: mob_collar(
                kind,
                session.world.entities.is_tame(id),
                session.world.entities.collar(id),
            ),
            // M68: `SheepWoolUndercoatLayer.submit`'s gate.
            undercoat: undercoat_visible(
                kind,
                session.world.entities.wool_color(id),
                session.world.entities.is_baby(id),
            ),
            // M68: the fish's two dyes, `[body, pattern]`.
            fish_dye: fish.map(|f| [f.body_color, f.pattern_color]),
            // M60. `player_skin` is this profile's uploaded textures; its
            // `cape` is `None` both when the profile carries no cape and
            // when one is still in flight, and either way vanilla's second
            // gate says draw nothing.
            cape: resolve_cape(
                &session.world.entities,
                id,
                kind,
                alpha,
                player_skin.and_then(|ps| ps.cape),
                item_names,
                equipment,
            ),
        });
    }
    // Drop tracker entries for despawned entities (recycled server ids
    // must not inherit a stale gesture clock).
    gestures
        .map
        .retain(|id, _| session.world.entities.get(*id).is_some());
    out
}

/// Borrow the baked mob-texture table into the entity pass's view type.
/// Initialize the entity pass, optionally overriding mob models from an
/// OptiFine CEM resource pack (`--pack` / `REWO_PACK`). Shared by the
/// headless and windowed live paths (M9).
///
/// Also loads the pack's ETF texture variants (M52) — models and textures come
/// out of the same zip — and returns its random-entity rules for the draw
/// builder to pick with. Empty when there is no pack.
pub(crate) fn init_entities_maybe_cem(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
    pack: &Option<PathBuf>,
) -> Result<rewo_data::etf::EtfPack, String> {
    let pack = pack
        .clone()
        .or_else(|| std::env::var("REWO_PACK").ok().map(PathBuf::from));
    let etf = match pack {
        Some(path) => {
            let cem = crate::cem_pack::load_cem_overrides(&path)?;
            let etf = rewo_data::etf::load_pack(&path).unwrap_or_else(|e| {
                // A pack with unreadable random-entity data still gets its
                // models; the mobs simply keep vanilla textures.
                log::warn!("live: ETF load failed ({e}) — vanilla textures");
                rewo_data::etf::EtfPack::default()
            });
            log::info!(
                "live: pack {} → {} model overrides, {} texture-variant rules",
                path.display(),
                cem.len(),
                etf.rules.len()
            );
            wr.init_entities_with_cem(
                gpu,
                font_data(baked),
                entity_textures_with(baked, &etf),
                cem,
            )?;
            etf
        }
        None => {
            wr.init_entities(gpu, font_data(baked), entity_textures(baked))?;
            rewo_data::etf::EtfPack::default()
        }
    };
    // **After** the pass exists — `init_entities` builds it, and installing
    // the glint first would have nothing to install into. M44 learned this
    // one the expensive way on the hand pass, where the order was reversed
    // and the shimmer silently never drew.
    if let Some(g) = baked.glint.as_ref() {
        wr.init_entity_glint(gpu, &g.rgba, g.w, g.h)?;
    }
    // M50: the worn-armour foil's own sheet, same ordering rule.
    if let Some(g) = baked.armor_glint.as_ref() {
        wr.init_entity_armor_glint(gpu, &g.rgba, g.w, g.h)?;
    }
    // M285: the charged creeper's swirl sheet, same ordering rule.
    // M287: and the powered wither's.
    for (image, which) in [
        (
            &baked.creeper_armor,
            rewo_gpu::entities::SwirlSheet::Creeper,
        ),
        (&baked.wither_armor, rewo_gpu::entities::SwirlSheet::Wither),
    ] {
        if let Some(g) = image.as_ref() {
            wr.init_entity_swirl(gpu, which, &g.rgba, g.w, g.h)?;
        }
    }
    // M286: the breeze wind's sheet, the jar texture the atlas also packs.
    if let Some(t) = breeze_wind_texture(baked) {
        wr.init_entity_wind(gpu, &t.rgba, t.w, t.h)?;
    }
    // M282: the HUD vignette's texture; M283: the other camera overlays'
    // (`blur` in the pumpkin's mcmeta, none in the other two).
    if let Some(v) = baked.vignette.as_ref() {
        wr.init_vignette(gpu, &v.rgba, v.w, v.h)?;
    }
    for (image, which, linear) in [
        (
            &baked.pumpkin_blur,
            rewo_gpu::world::OverlayImage::PumpkinBlur,
            true,
        ),
        (
            &baked.powder_snow,
            rewo_gpu::world::OverlayImage::PowderSnow,
            false,
        ),
        (
            &baked.spyglass_scope,
            rewo_gpu::world::OverlayImage::SpyglassScope,
            false,
        ),
    ] {
        if let Some(i) = image.as_ref() {
            wr.init_overlay_image(gpu, which, &i.rgba, i.w, i.h, linear)?;
        }
    }
    // M194: the map decoration sprites a framed map draws, into the entity
    // atlas's map band (the held map has its own copy in the hand atlas).
    let decorations: Vec<Option<&[u8]>> = baked
        .map_decorations
        .iter()
        .map(|d| d.as_ref().map(|i| i.rgba.as_slice()))
        .collect();
    wr.init_map_decorations(gpu, &decorations)?;
    Ok(etf)
}

/// M175 — the generated isBaby swaps resolved to (kind index, adult key,
/// baby key), filtered to pairs whose BOTH sheets are in this bake. The GPU
/// side turns each into a per-slot UV offset.
pub(super) fn baby_swap_pairs(
    baked: &assets::BakedAssets,
) -> Vec<(u16, &'static str, &'static str)> {
    rewo_data::baby_texture_table::BABY_SWAPS
        .iter()
        .filter_map(|swap| {
            let baked_keys = || baked.mob_textures.iter().any(|t| t.key == swap.baby_key);
            let adult_key = adult_key_of(swap)?;
            if !baked_keys() {
                return None;
            }
            // `kind_for_entity_name` wants the NAMESPACED wire name (its
            // strip_prefix is the whole lookup); the table stores short
            // names, so re-namespace here — the first cut passed the short
            // name, every row resolved to Capsule, and the swaps were inert.
            let kind = rewo_gpu::mobs::kind_for_entity_name(&format!("minecraft:{}", swap.entity));
            Some((kind.index() as u16, adult_key, swap.baby_key))
        })
        .collect()
}

/// The MOB_TEXTURE_SPECS key of a swap's adult sheet — `None` when the adult
/// path is not one Rewo bakes at all (then no model uses it and there is
/// nothing to offset).
pub(crate) fn adult_key_of(swap: &rewo_data::baby_texture_table::BabySwap) -> Option<&'static str> {
    let rel = swap
        .adult_path
        .strip_prefix("textures/")
        .unwrap_or(swap.adult_path);
    rewo_data::assets::mob_texture_key(rel)
}

/// M288 — `WitherBossRenderer.WITHER_INVULNERABLE_LOCATION`.
pub(crate) const WITHER_INVULNERABLE: &str = "entity/wither/wither_invulnerable.png";

/// M288 — `WitherBossRenderer.extractRenderState`'s
/// `invulnerableTicks > 0 ? invulnerableTicks - partialTicks : 0`.
pub(crate) fn wither_invulnerable_ticks(raw: i32, partial: f32) -> f32 {
    if raw > 0 {
        raw as f32 - partial
    } else {
        0.0
    }
}

/// M288 — `WitherBossRenderer.getTextureLocation`: the invulnerable sheet
/// while `floor(t) > 0` and (`> 80` or not on the off fifth of each ten):
/// steady for the first 140 ticks of the 220, then blinking.
pub(crate) fn wither_shows_invulnerable(t: f32) -> bool {
    let ticks = t.floor() as i32;
    ticks > 0 && (ticks > 80 || ticks / 5 % 2 != 1)
}

/// M288 — `WitherBossRenderer.scale`: `2 - t / 220 · 0.5` against the
/// model's own 2, as `EntityDraw::stretch` (uniform).
pub(crate) fn wither_stretch(t: f32) -> [f32; 2] {
    let s = (2.0 - t / 220.0 * 0.5) / 2.0;
    [s, s]
}

/// M287 — `WitherBoss.isPowered`: `getHealth() <= getMaxHealth() / 2`, the
/// max health from the attribute (synced, or the registry's default when the
/// server never sent it, as vanilla's client reads it). No registry: false.
pub(crate) fn wither_powered(
    ents: &rewo_world::entities::EntityTable,
    id: i32,
    reg: Option<&rewo_data::attributes::AttributeRegistry>,
) -> bool {
    let Some((max, _)) = reg.and_then(|reg| {
        rewo_world::attributes::resolve(
            ents.attributes(id),
            Some("minecraft:wither"),
            "max_health",
            reg,
        )
    }) else {
        return false;
    };
    ents.death_state(id).health <= max as f32 / 2.0
}

/// M286 — `entity/breeze/breeze_wind.png` from the bake's mob textures.
pub(crate) fn breeze_wind_texture(baked: &assets::BakedAssets) -> Option<&assets::MobTexture> {
    baked.mob_textures.iter().find(|t| t.key == "breeze_wind")
}

pub(crate) fn entity_textures(baked: &assets::BakedAssets) -> MobTextures<'_> {
    MobTextures {
        // M64: vanilla's own metadata-driven alternates are always present —
        // they are jar textures, not a pack's, so they do not wait for one.
        // `entity_textures_with` appends a pack's ETF alternates after these;
        // the two live in disjoint id bands.
        variants: vanilla_variants(baked),
        baby_swaps: baby_swap_pairs(baked),
        entries: baked
            .mob_textures
            .iter()
            .map(|t| MobTexEntry {
                key: t.key,
                w: t.w,
                h: t.h,
                rgba: &t.rgba,
            })
            // M46: the armour sheets share the entity atlas. They are 64x32
            // rather than 64x64, so the shelf packer fits them alongside the
            // mob textures with no special case.
            .chain(baked.equipment.textures.iter().map(|t| MobTexEntry {
                key: &t.key,
                w: t.w,
                h: t.h,
                rgba: &t.rgba,
            }))
            .collect(),
    }
}

/// `entity_textures` plus a pack's ETF alternates (M52), which the entity pass
/// packs into the same atlas and addresses by variant id.
pub(crate) fn entity_textures_with<'a>(
    baked: &'a assets::BakedAssets,
    etf: &'a rewo_data::etf::EtfPack,
) -> MobTextures<'a> {
    MobTextures {
        variants: vanilla_variants(baked)
            .into_iter()
            .chain(
                etf.textures
                    .iter()
                    .map(|t| rewo_gpu::entities::VariantTexEntry {
                        base_key: t.key,
                        index: t.index,
                        w: t.w,
                        h: t.h,
                        rgba: &t.rgba,
                    }),
            )
            .collect(),
        ..entity_textures(baked)
    }
}

/// Vanilla's metadata-driven alternates as atlas entries (M64).
///
/// They are appended *after* the base textures in the packer's input, exactly
/// as a pack's ETF alternates are, so every existing texel address is
/// unchanged and `mobshot --check` still grades the geometry it graded before.
pub(super) fn vanilla_variants(
    baked: &assets::BakedAssets,
) -> Vec<rewo_gpu::entities::VariantTexEntry<'_>> {
    baked
        .mob_variant_textures
        .iter()
        .map(|t| rewo_gpu::entities::VariantTexEntry {
            base_key: t.key,
            index: t.index as u32,
            w: t.w,
            h: t.h,
            rgba: &t.rgba,
        })
        .collect()
}

pub(crate) fn font_data(baked: &assets::BakedAssets) -> Option<FontData<'_>> {
    baked.font.as_ref().map(|f: &BakedFont| FontData {
        atlas: &f.atlas,
        size: f.atlas_size,
        cell: f.cell,
        advance: &f.advance,
        white_texel: f.white_texel,
    })
}

#[cfg(test)]
mod wither_phase_tests {
    use super::*;

    /// M288 — the invulnerable sheet is steady above 80 ticks, then off on
    /// the odd fifths (`ticks / 5 % 2 == 1`); none at 0. The scale runs
    /// from 1.5 (relative 0.75) at 220 to the plain 2 at 0.
    #[test]
    fn the_withers_invulnerable_phase_blinks_and_shrinks() {
        assert_eq!(wither_invulnerable_ticks(0, 0.5), 0.0);
        assert_eq!(wither_invulnerable_ticks(220, 0.25), 219.75);
        assert!(wither_shows_invulnerable(219.75));
        assert!(wither_shows_invulnerable(81.0));
        assert!(wither_shows_invulnerable(80.0), "80 / 5 = 16, even: on");
        assert!(!wither_shows_invulnerable(75.0), "75 / 5 = 15, odd: off");
        assert!(!wither_shows_invulnerable(79.9), "floor 79 / 5 = 15: off");
        assert!(wither_shows_invulnerable(70.0));
        assert!(!wither_shows_invulnerable(0.5), "floor 0: none");
        assert_eq!(wither_stretch(220.0), [0.75, 0.75]);
        assert_eq!(wither_stretch(0.0), [1.0, 1.0]);
        assert!(rewo_data::mob_variants::variant_id(WITHER_INVULNERABLE).is_some_and(|v| v != 0));
    }
}

#[cfg(test)]
mod skin_policy_tests {
    use super::*;

    const K: fn(u128) -> (u128, TexKind) = |u| (u, TexKind::Skin);

    /// A cached texture is uploaded again, not downloaded again; only a
    /// texture the cache lost is fetched, and both are rate-limited.
    #[test]
    fn restore_prefers_the_cache_and_rate_limits_both_paths() {
        assert_eq!(plan_restore(true, None, 0.0), Restore::Reupload);
        assert_eq!(plan_restore(false, None, 0.0), Restore::Refetch);
        // Just attempted: wait.
        assert_eq!(plan_restore(true, Some(10.0), 11.0), Restore::Wait);
        assert_eq!(plan_restore(false, Some(10.0), 11.0), Restore::Wait);
        // A cache upload is allowed again after its short cooldown; a
        // download only after the long one.
        let after = 10.0 + REUPLOAD_COOLDOWN_S + 0.01;
        assert_eq!(plan_restore(true, Some(10.0), after), Restore::Reupload);
        assert_eq!(plan_restore(false, Some(10.0), after), Restore::Wait);
        assert_eq!(
            plan_restore(false, Some(10.0), 10.0 + REFETCH_COOLDOWN_S + 0.01),
            Restore::Refetch
        );
    }

    #[test]
    fn the_cache_is_bounded_and_drops_the_least_recently_used() {
        let mut c = TexCache::default();
        for u in 0..TEX_CACHE_CAP as u128 {
            c.insert(K(u), vec![u as u8], false);
        }
        assert_eq!(c.len(), TEX_CACHE_CAP);
        // Touch 0, so 1 is now the oldest.
        assert!(c.get(&K(0)).is_some());
        c.insert(K(1_000), vec![9], true);
        assert_eq!(c.len(), TEX_CACHE_CAP, "still bounded");
        assert!(c.contains(&K(0)) && c.contains(&K(1_000)));
        assert!(!c.contains(&K(1)), "the least recently used went");
        // Re-inserting an existing key never evicts another.
        c.insert(K(0), vec![7], false);
        assert_eq!(c.len(), TEX_CACHE_CAP);
        assert_eq!(c.get(&K(0)).unwrap().0, vec![7]);
    }

    /// An NPC-style player (announced with a skin but no gamemode, latency or
    /// list entry) is still tracked by the session's skin table, which will
    /// never announce their unchanged skin again: pruning against that table
    /// must keep their state, and forget exactly who the table forgot.
    #[test]
    fn pruning_follows_the_sessions_skin_table() {
        use rewo_net::skins::{SkinInfo, SkinTable};
        let info = |n: u32| SkinInfo {
            url: Some(format!("http://textures.minecraft.net/texture/{n:064x}")),
            ..SkinInfo::default()
        };
        let mut table = SkinTable::default();
        assert!(table.note(10, &info(10), 0), "NPC announced");
        assert!(table.note(11, &info(11), 0), "ordinary player announced");

        let mut l = SkinLoader::new();
        for u in [10u128, 11] {
            l.requested.insert((u, TexKind::Skin));
            l.registry.insert(u, Default::default());
        }
        // Nothing but the table says either is present (no entity, no tab
        // entry, no latency): both stay.
        l.retain_players(|u| table.contains(u));
        assert_eq!(l.registry.len(), 2);
        assert!(
            !table.note(10, &info(10), 0),
            "an unchanged skin is not re-announced"
        );

        // Player 11 leaves (Player Info Remove): only theirs goes.
        table.remove(11);
        l.retain_players(|u| table.contains(u));
        assert!(l.registry.contains_key(&10) && !l.registry.contains_key(&11));
        assert!(l.requested.contains(&(10, TexKind::Skin)));
        assert!(!l.requested.contains(&(11, TexKind::Skin)));
    }

    /// Player churn must not turn the request cap into a lifetime cap: a
    /// departed player's state is dropped everywhere, the local player's and
    /// the still-present players' are not.
    #[test]
    fn departed_players_are_forgotten_everywhere_and_present_ones_are_not() {
        let mut l = SkinLoader::new();
        l.own = Some(1);
        for u in 1u128..=6 {
            for kind in [TexKind::Skin, TexKind::Cape] {
                let key = (u, kind);
                l.requested.insert(key);
                l.urls
                    .insert(key, ("http://example.invalid/x".into(), false));
                l.evicted.insert(key);
                l.attempts.insert(key, 1.0);
                l.cache.insert(key, vec![0], false);
            }
            l.registry.insert(u, Default::default());
        }
        // Present: 3 and 4. 1 is the local player (kept regardless).
        l.retain_players(|u| u == 3 || u == 4);
        let mut left: Vec<u128> = l.requested.iter().map(|(u, _)| *u).collect();
        left.sort_unstable();
        left.dedup();
        assert_eq!(left, vec![1, 3, 4]);
        assert!(l.urls.keys().all(|(u, _)| [1, 3, 4].contains(u)));
        assert!(l.evicted.iter().all(|(u, _)| [1, 3, 4].contains(u)));
        assert!(l.attempts.keys().all(|(u, _)| [1, 3, 4].contains(u)));
        assert_eq!(l.cache.len(), 6);
        assert!(!l.cache.contains(&K(2)) && l.cache.contains(&K(3)));
        assert_eq!(l.registry.len(), 3);
        // And the freed room is usable: the cap is on live players.
        assert!(l.requested.len() < 2 * rewo_net::skins::MAX_TRACKED_SKINS);
    }
}

#[cfg(test)]
mod default_skin_tests {
    use super::{
        avatar_skin, default_skin_index, AvatarSkin, SkinLoader, TexKind, DEFAULT_SKIN_SCHEME,
    };
    use rewo_world::entities::AvatarProfile;

    /// M309 — a mannequin's profile: none is the nil UUID's slim Alex; a name
    /// alone is its offline UUID's default (RewoDummyA: 12, wide Kai); an id
    /// is its own; a patch's body texture and model override; a textures
    /// property is a real skin.
    #[test]
    fn a_mannequins_profile_picks_its_skin() {
        let d = |p: Option<&AvatarProfile>| match avatar_skin(p) {
            AvatarSkin::Default { index, slim } => Some((index, slim)),
            AvatarSkin::Real(_) => None,
        };
        assert_eq!(d(None), Some((0, true)));
        let named = AvatarProfile {
            name: Some("RewoDummyA".into()),
            ..Default::default()
        };
        assert_eq!(d(Some(&named)), Some((12, false)));
        let id = AvatarProfile {
            id: Some(0x11112222_3333_4444_5555_666677778888),
            name: Some("RewoDummyA".into()),
            ..Default::default()
        };
        assert_eq!(d(Some(&id)), Some((14, false)), "the id wins over the name");
        let patched = AvatarProfile {
            body_texture: Some("minecraft:entity/player/wide/steve".into()),
            model_slim: Some(true),
            ..Default::default()
        };
        assert_eq!(d(Some(&patched)), Some((15, true)));
        // `{"textures":{"SKIN":{"url":"http://textures.minecraft.net/texture/ab"}}}`.
        let real = AvatarProfile {
            textures: Some(
                "eyJ0ZXh0dXJlcyI6eyJTS0lOIjp7InVybCI6Imh0dHA6Ly90ZXh0dXJlcy5taW5lY3JhZnQubmV0L3RleHR1cmUvYWIifX19"
                    .into(),
            ),
            ..Default::default()
        };
        assert!(matches!(avatar_skin(Some(&real)), AvatarSkin::Real(_)));
    }

    /// M308 — a default queued before the profile's textures arrived gives
    /// way to the real skin; a real request is never overwritten by a
    /// default.
    #[test]
    fn a_real_skin_replaces_a_queued_default_and_not_the_reverse() {
        let real = rewo_net::skins::SkinInfo {
            url: Some("http://textures.minecraft.net/texture/ab".into()),
            ..Default::default()
        };
        let mut l = SkinLoader::new();
        l.request_default(5, 0);
        assert!(l.urls[&(5, TexKind::Skin)]
            .0
            .starts_with(DEFAULT_SKIN_SCHEME));
        assert!(l.urls[&(5, TexKind::Skin)].1, "index 0 is slim Alex");
        l.request(5, &real);
        assert_eq!(l.urls[&(5, TexKind::Skin)].0, real.url.clone().unwrap());
        l.request_default(5, 0);
        assert_eq!(l.urls[&(5, TexKind::Skin)].0, real.url.clone().unwrap());
    }

    /// M308 — `floorMod(UUID.hashCode(), 18)`, against values computed
    /// independently from Java's definitions (the negative hashes are the
    /// cases a plain `%` gets wrong).
    #[test]
    fn default_skins_are_chosen_by_the_profile_ids_java_hash() {
        for (id, want) in [
            (0u128, 0),
            (0x069a79f4_44e9_4726_a5be_fca90e38aaf5, 0),
            (0x853c80ef_3c37_49fd_aa49_938b674adae6, 1),
            (0x11112222_3333_4444_5555_666677778888, 14),
            (0xdeadbeef_cafe_babe_0011_223344556677, 5),
            (0xdeadbeef_cafe_babe_8011_223344556677, 3),
            (0x80000000_0000_0000_0000_000000000001, 17),
            (0xf0f0f0f0_f0f0_f0f0_0000_000000000007, 7),
        ] {
            assert_eq!(default_skin_index(id), want, "{id:032x}");
        }
    }
}

#[cfg(test)]
mod wolf_crack_tests {
    use super::wolf_crack_level;

    /// M311 — `Crackiness.WOLF_ARMOR`'s thresholds on the 64-point wolf
    /// armour: 61 left is none, 60 low, 44 medium, 20 high.
    #[test]
    fn wolf_armour_cracks_by_its_durability_left() {
        assert_eq!(wolf_crack_level(0, 64), None);
        assert_eq!(wolf_crack_level(3, 64), None);
        assert_eq!(wolf_crack_level(4, 64), Some("wolf_armor_crackiness_low"));
        assert_eq!(
            wolf_crack_level(20, 64),
            Some("wolf_armor_crackiness_medium")
        );
        assert_eq!(wolf_crack_level(44, 64), Some("wolf_armor_crackiness_high"));
        assert_eq!(wolf_crack_level(0, 0), None);
    }
}
