use super::*;

/// The GUI-item atlas: one row of slots, each large enough for any item
/// texture the bake produces.
///
/// Deliberately its own atlas rather than the entity pass's. That one is a
/// demand-filled pool sized for mob skins and shared with the held-item path;
/// borrowing it would couple the HUD to the entity pass's residency policy for
/// the sake of at most nine small textures.
pub(super) const GUI_ATLAS_SLOT: u32 = 64;
pub(super) const GUI_ATLAS_COLS: u32 = 8;
pub(super) const GUI_ATLAS_ROWS: u32 = 8;
pub(super) const GUI_ATLAS_W: u32 = GUI_ATLAS_SLOT * GUI_ATLAS_COLS;
pub(super) const GUI_ATLAS_H: u32 = GUI_ATLAS_SLOT * GUI_ATLAS_ROWS;

/// A packed GUI atlas plus where each source texture landed.
pub struct GuiAtlas {
    pub rgba: Vec<u8>,
    /// Texture index -> `(u0, v0, du, dv)`.
    pub uv: std::collections::HashMap<u16, [f32; 4]>,
}

/// Pack every item texture the bake produced, up to the atlas's capacity.
///
/// Built once at startup rather than per frame: the item set is fixed by the
/// bake, and a hotbar swap must not cost an atlas upload. Textures past the
/// capacity are dropped with a log — their items then draw nothing, which is
/// the same "nothing rather than garbage" rule the rest of the item path uses.
pub fn pack_gui_atlas(items: &rewo_gpu::held::HeldItems, wanted: &[u16]) -> GuiAtlas {
    let mut rgba = vec![0u8; (GUI_ATLAS_W * GUI_ATLAS_H * 4) as usize];
    let mut uv = std::collections::HashMap::new();
    let cap = (GUI_ATLAS_COLS * GUI_ATLAS_ROWS) as usize;
    let mut dropped = 0usize;
    for (slot, &tex) in wanted.iter().enumerate() {
        if slot >= cap {
            dropped += 1;
            continue;
        }
        let Some(src) = items.textures.get(tex as usize) else {
            continue;
        };
        if src.w > GUI_ATLAS_SLOT || src.h > GUI_ATLAS_SLOT {
            dropped += 1;
            continue;
        }
        let (ox, oy) = (
            (slot as u32 % GUI_ATLAS_COLS) * GUI_ATLAS_SLOT,
            (slot as u32 / GUI_ATLAS_COLS) * GUI_ATLAS_SLOT,
        );
        for y in 0..src.h {
            let s = (y * src.w * 4) as usize;
            let d = (((oy + y) * GUI_ATLAS_W + ox) * 4) as usize;
            let n = (src.w * 4) as usize;
            rgba[d..d + n].copy_from_slice(&src.rgba[s..s + n]);
        }
        uv.insert(
            tex,
            [
                ox as f32 / GUI_ATLAS_W as f32,
                oy as f32 / GUI_ATLAS_H as f32,
                src.w as f32 / GUI_ATLAS_W as f32,
                src.h as f32 / GUI_ATLAS_H as f32,
            ],
        );
    }
    if dropped > 0 {
        log::warn!(
            "live: {dropped} item textures did not fit the GUI atlas — those icons will not draw"
        );
    }
    GuiAtlas { rgba, uv }
}

/// Every texture index the hotbar could need, in a stable order.
///
/// The whole baked item set is far larger than the atlas, so this takes the
/// textures of the items the *player actually has*, which is at most nine
/// models' worth.
pub fn gui_atlas_wanted(items: &rewo_gpu::held::HeldItems, models: &[String]) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    for name in models {
        if let Some(m) = items.any(name) {
            for q in &m.quads {
                if !out.contains(&q.tex) {
                    out.push(q.tex);
                }
            }
        }
    }
    out
}

/// This frame's hotbar, as model names per slot (`None` for an empty slot).
pub fn hotbar_models(
    inv: &rewo_world::inventory::Inventory,
    items: &rewo_data::items::Items,
    trim_materials: &[rewo_net::trim_parse::TrimMaterialDef],
) -> [Option<String>; 9] {
    std::array::from_fn(|i| {
        let s = inv.hotbar(i)?;
        let base = items.name(s.item_id)?;
        // M49: the same composed name the screen slots use, so a trimmed piece
        // wears its trim in the hotbar too.
        Some(
            match s.trim_material.and_then(|m| trim_materials.get(m as usize)) {
                Some(m) => format!("{base}#{}", m.id),
                None => base.to_string(),
            },
        )
    })
}

/// The inventory preview's cape, from its slot in the **preview** pass's own
/// atlas (M64).
///
/// Extracted so `capeshot` grades the decision the client actually makes
/// rather than a restatement of it — M45's and M41's gates both quietly
/// stopped testing their subject by reimplementing a slice of the app.
///
/// All three angles are zero, which is not a simplification of vanilla so
/// much as the same consequence as the preview's still legs:
/// `capeFlap`/`capeLean`/`capeLean2` are driven entirely by the gap between
/// the player and their lagging cloak anchor, and a player standing in an
/// open inventory has let that gap close. What is genuinely missing is the
/// *moving* case, for the reason the limbs are missing — nothing in Rewo
/// consumes the local player's animation state.
///
/// `chest_humanoid` is false because the preview draws no armour at all
/// (`armor: [None; 4]`): nothing there can be wearing a chestplate to shift
/// the cape clear, and by the same token nothing can be wearing an elytra to
/// suppress it — so `CapeLayer`'s other two gates have nothing to act on.
pub(crate) fn preview_cape(origin: Option<(u32, u32)>) -> Option<rewo_gpu::entities::CapeDraw> {
    origin.map(|origin| rewo_gpu::entities::CapeDraw {
        origin,
        flap: 0.0,
        lean: 0.0,
        lean2: 0.0,
        chest_humanoid: false,
        wavy: None,
    })
}

/// The player model shown in the inventory screen's window (M36).
///
/// `extractEntityInInventoryFollowsMouse` poses it from the cursor: the body
/// turns `180 + xAngle`, the head turns `xAngle` on top of that, and the pitch
/// is `-yAngle`. The 180 is why the model faces you at rest — the same
/// convention every other entity in Rewo uses, where yaw 0 faces +Z.
///
/// It stands still: no limb swing, no gesture, no hurt flash. Vanilla poses it
/// from the live player's render state, so a walking player's legs move in the
/// preview too; that would need the local player's animation state, which
/// nothing else in Rewo consumes yet.
///
/// **The cape (M64)** hangs from `cape_origin`, an address in the preview
/// pass's own atlas — see [`preview_cape`] for why its three angles are zero.
pub(super) fn preview_draw<'a>(
    session: &PlaySession,
    skin: Option<[f32; 2]>,
    slim: bool,
    cape_origin: Option<(u32, u32)>,
    held: [Option<&'a str>; 2],
    w: f32,
    h: f32,
    mouse: (f64, f64),
) -> (EntityDraw<'a>, [[f32; 4]; 4], ash::vk::Rect2D) {
    let (x_angle, y_angle) = rewo_gpu::container::preview_angles(mouse, w, h);
    // `LivingEntityRenderState.boundingBoxHeight / scale`, and the player's
    // scale is 1 — so this is the standing hitbox, which is what centres the
    // model in its window.
    const PLAYER_HEIGHT: f32 = 1.8;
    let draw = EntityDraw {
        pos: [0.0, 0.0, 0.0],
        width: 0.6,
        height: PLAYER_HEIGHT,
        color: [1.0, 1.0, 1.0],
        name: None,
        // M59: no health bar in a still — the gate renders its own.
        health: None,
        kind: if slim {
            EntityModelKind::PlayerSlim
        } else {
            EntityModelKind::Player
        },
        yaw: 180.0 + x_angle,
        death_time: 0.0,
        ground_item: None,
        armor: [None; 4],
        held_glint: [false; 2],
        ground_glint: false,
        ground_count: 0,
        bob_offset: 0.0,
        ground_seed: 0,
        ground_age: None,
        head_yaw: 180.0 + x_angle,
        pitch: -y_angle,
        limb_swing: 0.0,
        limb_amount: 0.0,
        gesture: None,
        events: [None; rewo_gpu::mobs::ModelEvent::COUNT],
        shell: false,
        stand_parts: Default::default(),
        billboard: None,
        allay_dance: None,
        attack: rewo_gpu::mobs::SwingPose::NONE,
        mob: rewo_gpu::mobs::MobCombat::default(),
        hurt: false,
        held,
        arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
        skin_uv: skin,
        scale_mul: 1.0,
        mount: None,
        anim_id: 0.0,
        // `GuiEntityRenderer` sets `renderState.lightCoords = 15728880`, which
        // is both light channels at full — the preview is lit by the GUI's own
        // two-light rig, not by wherever the player happens to be standing.
        light: [1.0, 1.0, 1.0],
        // M52: no emissive state, no pack variant, no dye — the
        // vanilla defaults, which is what this gate renders.
        emissive: rewo_gpu::entities::EmissiveState::default(),
        variant: 0,
        dye: None,
        sheared: false,
        undercoat: false,
        fish_dye: None,
        cape: preview_cape(cape_origin),
    };
    let vp = rewo_gpu::container::preview_view_proj(w, h, PLAYER_HEIGHT, y_angle);
    let (rx, ry, rw, rh) = rewo_gpu::container::preview_rect(w, h);
    let rect = ash::vk::Rect2D {
        offset: ash::vk::Offset2D {
            x: rx as i32,
            y: ry as i32,
        },
        extent: ash::vk::Extent2D {
            width: rw as u32,
            height: rh as u32,
        },
    };
    let _ = session;
    (draw, vp, rect)
}

// -- the first-person hand (M38) ----------------------------------------------

/// The hand's atlas: the same 64-px grid the GUI icons use, with the player's
/// 64×64 skin parked in the bottom-left quadrant.
///
/// One texture rather than two draws, because the arm and the item are one
/// pass — and one pass because they share a matrix chain up to the point where
/// they diverge.
pub(crate) const HAND_ATLAS: u32 = 512;
pub(crate) const HAND_SKIN_X: u32 = 0;
pub(crate) const HAND_SKIN_Y: u32 = HAND_ATLAS - 64;

/// M192 — a held map's three textures in the hand atlas, `(x, y, w, h)`
/// (claimed in `REWO_PLAN.md` §0.0's allocation table). Present only while a
/// map is held; the items then start at [`HAND_MAP_ITEM_Y`] instead of 0.
pub(crate) const HAND_MAP_PIXELS: (u32, u32, u32, u32) = (0, 0, 128, 128);
/// `textures/map/map_background_checkerboard.png`, 128 x 128.
pub(super) const HAND_MAP_CHECKER: (u32, u32, u32, u32) = (128, 0, 128, 128);
/// `textures/map/map_background.png`, 64 x 64.
pub(super) const HAND_MAP_BACKGROUND: (u32, u32, u32, u32) = (256, 0, 64, 64);
pub(super) const HAND_MAP_ITEM_Y: u32 = 128;
/// The decoration sprites' band (8 x 8 each, by raw type id, 24 to a row)
/// right of the plain background: `(320, 0)..(512, 128)`.
pub(crate) const HAND_MAP_DECORATIONS: (u32, u32) = (320, 0);
/// M213 — the banner labels' glyphs: an 8 x 8 grid of 8-px cells under the
/// plain background, `(256, 64)..(320, 128)`. Cells 0..62 take the labels'
/// distinct bytes in order; cell 63 is the background texel (black, alpha
/// 128 — `Font`'s `0x80000000` effect colour).
pub(crate) const HAND_MAP_GLYPHS: (u32, u32) = (256, 64);
/// Glyph cells before the background cell.
pub(crate) const HAND_MAP_GLYPH_CELLS: usize = 63;
const HAND_MAP_DECO_COLS: u32 = 24;
const HAND_MAP_DECO_ROWS: u32 = 16;

/// Where decoration type `id`'s sprite sits in the hand atlas, as a UV rect,
/// or `None` past the band.
pub(crate) fn map_decoration_uv(id: i32) -> Option<[f32; 4]> {
    let id = u32::try_from(id).ok()?;
    if id >= HAND_MAP_DECO_COLS * HAND_MAP_DECO_ROWS {
        return None;
    }
    let (x, y) = (
        HAND_MAP_DECORATIONS.0 + (id % HAND_MAP_DECO_COLS) * 8,
        HAND_MAP_DECORATIONS.1 + (id / HAND_MAP_DECO_COLS) * 8,
    );
    Some(atlas_rect_uv((x, y, 8, 8)))
}

/// The RGBA a held map needs packed (M192): its pixels (`None` while the
/// level does not hold the map — vanilla then draws the plain background and
/// no pixels) and the two backgrounds from the jar.
pub(crate) struct HeldMapTextures<'a> {
    pub pixels: Option<&'a [u8]>,
    pub checker: &'a [u8],
    pub background: &'a [u8],
    /// Each decoration type's 8 x 8 sprite by raw id (`None` = missing).
    pub decorations: &'a [Option<&'a [u8]>],
    /// M213 — the bitmap font sheet `(rgba, size, cell)` and the distinct
    /// bytes the map's labels draw (at most [`HAND_MAP_GLYPH_CELLS`]).
    pub font: Option<(&'a [u8], u32, u32)>,
    pub label_bytes: &'a [u8],
}

/// Where [`pack_hand_atlas`] put a held map's textures, as atlas UV rects
/// `(u0, v0, du, dv)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeldMapUv {
    pub pixels: [f32; 4],
    pub checker: [f32; 4],
    pub background: [f32; 4],
    /// M213 — the label background texel's cell.
    pub label_background: [f32; 4],
}

fn atlas_rect_uv((x, y, w, h): (u32, u32, u32, u32)) -> [f32; 4] {
    let a = HAND_ATLAS as f32;
    [x as f32 / a, y as f32 / a, w as f32 / a, h as f32 / a]
}

/// The atlas pixel origin of label glyph cell `k` (M213).
pub(crate) fn map_glyph_cell(k: usize) -> (u32, u32) {
    let k = k as u32;
    (
        HAND_MAP_GLYPHS.0 + (k % 8) * 8,
        HAND_MAP_GLYPHS.1 + (k / 8) * 8,
    )
}

/// The UV rect of label glyph cell `k`.
pub(crate) fn map_glyph_uv(k: usize) -> [f32; 4] {
    let (x, y) = map_glyph_cell(k);
    atlas_rect_uv((x, y, 8, 8))
}

/// A held map's banner labels (M213): each named decoration's text, flattened
/// as the framed map's are (M204), and the distinct bytes they draw, in
/// first-seen order and capped at the glyph cells.
pub(crate) fn held_map_labels(
    data: &rewo_world::maps::ClientMap,
    lang: Option<&rewo_data::lang::Language>,
) -> (Vec<(i8, i8, String)>, Vec<u8>) {
    let labels: Vec<(i8, i8, String)> = data
        .decorations
        .iter()
        .filter_map(|d| {
            let text = rewo_world::chat_style::flatten(d.name.as_ref()?, lang);
            (!text.is_empty()).then_some((d.x, d.y, text))
        })
        .collect();
    let mut bytes: Vec<u8> = Vec::new();
    for (_, _, t) in &labels {
        for b in t.bytes() {
            if b != b' ' && !bytes.contains(&b) && bytes.len() < HAND_MAP_GLYPH_CELLS {
                bytes.push(b);
            }
        }
    }
    (labels, bytes)
}

/// Copy a `w x h` RGBA image into the atlas at `(x, y)`; a short source is
/// skipped rather than read past.
fn blit_into_atlas(rgba: &mut [u8], src: &[u8], (x, y, w, h): (u32, u32, u32, u32)) {
    if src.len() < (w * h * 4) as usize {
        return;
    }
    for row in 0..h {
        let s = (row * w * 4) as usize;
        let d = (((y + row) * HAND_ATLAS + x) * 4) as usize;
        rgba[d..d + (w * 4) as usize].copy_from_slice(&src[s..s + (w * 4) as usize]);
    }
}

/// Everything the hand needs across frames.
pub struct HandState {
    /// The item textures resident in the atlas, and where each landed.
    pub(super) resident: Vec<u16>,
    /// Whether the resident atlas was packed with the current skin. A skin
    /// that arrives after the first pack must force one, or the arm keeps
    /// sampling the default.
    pub(super) skin_resident: bool,
    pub(super) uv: std::collections::HashMap<u16, [f32; 4]>,
    /// The two equip clocks — `mainHandHeight` and `offHandHeight`.
    pub(super) main_equip: rewo_gpu::hand::EquipHeight,
    pub(super) off_equip: rewo_gpu::hand::EquipHeight,
    /// `LocalPlayer.xBob` / `yBob`.
    pub(super) bob: rewo_gpu::hand::ViewBob,
    /// The skin in the hand atlas, and whether the model is slim.
    ///
    /// Seeded with the jar's own default so an empty hand shows an arm from
    /// the first frame — which is also what vanilla shows on an offline server,
    /// where no player carries a `textures` property. A real skin replaces it
    /// when one arrives.
    pub(super) skin: Option<(Vec<u8>, bool)>,
    pub(super) held: rewo_gpu::held::HeldItems,
    /// M234 — the model name the main hand drew last frame (r100).
    #[cfg(feature = "gates")]
    pub(super) last_main_model: Option<String>,
    /// The tick the clocks were last advanced on, so they step once per client
    /// tick rather than once per frame.
    pub(super) last_tick: u64,
    /// A swing frozen for a headless shot (`REWO_HAND_SWING`). `None` in a
    /// real session, where the clock is the entity table's.
    pub(super) forced_attack: Option<f32>,
    /// When this session started, for the glint's wall-clock phase (M44).
    pub(super) started: std::time::Instant,
    /// `misc/enchanted_glint_item.png` as `(rgba, w, h)`; `None` draws none.
    pub(super) glint: Option<(Vec<u8>, u32, u32)>,
    /// M192 — the held map's two backgrounds, `(map_background,
    /// map_background_checkerboard)` RGBA from the bake; `None` (either
    /// missing) draws no held map.
    pub(super) map_backgrounds: Option<(Vec<u8>, Vec<u8>)>,
    /// M192 — what the resident atlas holds of a held map: `(map id, the
    /// map's version, whether the level held it)`, or `None` for no map. Part
    /// of the repack key, so a new patch re-uploads the pixels.
    pub(super) map_resident: Option<(i32, u64, bool)>,
    /// The decoration sprites by raw type id, from the bake.
    pub(super) map_decoration_sprites: Vec<Option<Vec<u8>>>,
    pub(super) map_uv: Option<HeldMapUv>,
    /// M213 — the bitmap font `(sheet, size, cell, advances)` for a held
    /// map's banner labels, and the glyph bytes the resident atlas holds.
    pub(super) font: Option<(Vec<u8>, u32, u32, [u8; 256])>,
    pub(super) map_glyphs: Vec<u8>,
}

impl HandState {
    pub fn new(baked: &assets::BakedAssets) -> Self {
        Self {
            resident: Vec::new(),
            skin_resident: false,
            uv: std::collections::HashMap::new(),
            main_equip: Default::default(),
            off_equip: Default::default(),
            bob: Default::default(),
            // `entity/player/wide/steve.png`, the 64x64 the bake already
            // carries for the entity pass's default player.
            skin: baked
                .mob_textures
                .iter()
                .find(|t| t.key == "player")
                .map(|t| (t.rgba.clone(), false)),
            held: to_gpu_held_items(&baked.held_items),
            last_tick: 0,
            forced_attack: None,
            #[cfg(feature = "gates")]
            last_main_model: None,
            started: std::time::Instant::now(),
            glint: baked.glint.as_ref().map(|i| (i.rgba.clone(), i.w, i.h)),
            map_backgrounds: match (
                baked.map_background.as_ref(),
                baked.map_background_checkerboard.as_ref(),
            ) {
                (Some(bg), Some(cb)) if (bg.w, bg.h, cb.w, cb.h) == (64, 64, 128, 128) => {
                    Some((bg.rgba.clone(), cb.rgba.clone()))
                }
                _ => None,
            },
            map_resident: None,
            map_uv: None,
            map_decoration_sprites: baked
                .map_decorations
                .iter()
                .map(|d| {
                    d.as_ref()
                        .filter(|d| (d.w, d.h) == (8, 8))
                        .map(|d| d.rgba.clone())
                })
                .collect(),
            font: baked
                .font
                .as_ref()
                .map(|f| (f.atlas.clone(), f.atlas_size, f.cell, f.advance)),
            map_glyphs: Vec::new(),
        }
    }

    /// Advance the two equip clocks and the view bob, once per client tick.
    ///
    /// Separate from the per-frame build because they are *tick* clocks:
    /// running them per frame would make the equip dip three frames long
    /// rather than three ticks, so it would vanish at any sane frame rate.
    pub fn tick(&mut self, session: &PlaySession, items: &rewo_data::items::Items) {
        let now = session.ticks;
        if now == self.last_tick {
            return;
        }
        self.last_tick = now;
        self.step(session, items);
    }

    /// Advance the clocks once, unconditionally.
    ///
    /// Separate from [`Self::tick`] because that one dedupes on the session's
    /// tick counter — which is right per frame and wrong for a headless shot,
    /// where the counter does not move and the equip clock would stay at the
    /// bottom with the item off screen.
    pub(super) fn step(&mut self, session: &PlaySession, items: &rewo_data::items::Items) {
        let id = |s: Option<rewo_world::inventory::ItemSlot>| {
            s.and_then(|s| items.name(s.item_id).map(|_| s.item_id))
        };
        self.main_equip.tick(id(session.inventory.held()));
        self.off_equip.tick(id(session.inventory.offhand()));
        self.bob.tick(session.player.pitch, session.player.yaw);
    }

    /// Run the clocks to rest — the equip dip fully raised — for a shot.
    pub fn settle(&mut self, session: &PlaySession, items: &rewo_data::items::Items) {
        for _ in 0..8 {
            self.step(session, items);
        }
    }
}

/// The hand's own projection.
///
/// Vanilla renders it through the same perspective as the world but with the
/// FOV *unmodified* by the speed/effect multipliers — `getFov(camera, partial,
/// false)`. Rewo has no FOV modifiers yet, so this is the world's projection;
/// the near plane matters more, and it is shared, which is what keeps the
/// item's near corner from clipping.
pub(super) fn hand_view_proj(aspect: f32) -> glam::Mat4 {
    glam::Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
        70f32.to_radians(),
        aspect,
        0.05,
    ))
}

/// Pack the hand atlas: the item textures the two hands need, plus the skin.
pub(crate) fn pack_hand_atlas(
    held: &rewo_gpu::held::HeldItems,
    wanted: &[u16],
    skin: Option<&[u8]>,
    // M192 — a held map's textures, which take the top band and push the
    // items down to `HAND_MAP_ITEM_Y`.
    map: Option<&HeldMapTextures<'_>>,
) -> (
    Vec<u8>,
    std::collections::HashMap<u16, [f32; 4]>,
    Option<HeldMapUv>,
) {
    let mut rgba = vec![0u8; (HAND_ATLAS * HAND_ATLAS * 4) as usize];
    let mut uv = std::collections::HashMap::new();
    let cols = HAND_ATLAS / GUI_ATLAS_SLOT;
    let item_y = if map.is_some() { HAND_MAP_ITEM_Y } else { 0 };
    let map_uv = map.map(|m| {
        if let Some(px) = m.pixels {
            blit_into_atlas(&mut rgba, px, HAND_MAP_PIXELS);
        }
        blit_into_atlas(&mut rgba, m.checker, HAND_MAP_CHECKER);
        blit_into_atlas(&mut rgba, m.background, HAND_MAP_BACKGROUND);
        for (id, sprite) in m.decorations.iter().enumerate() {
            let id32 = id as u32;
            if let (Some(px), true) = (sprite, id32 < HAND_MAP_DECO_COLS * HAND_MAP_DECO_ROWS) {
                let (x, y) = (
                    HAND_MAP_DECORATIONS.0 + (id32 % HAND_MAP_DECO_COLS) * 8,
                    HAND_MAP_DECORATIONS.1 + (id32 / HAND_MAP_DECO_COLS) * 8,
                );
                blit_into_atlas(&mut rgba, px, (x, y, 8, 8));
            }
        }
        // M213 — the labels' glyphs, copied cell by cell from the font sheet,
        // and the half-black background cell after them.
        if let Some((sheet, size, cell)) = m.font {
            for (k, &b) in m.label_bytes.iter().take(HAND_MAP_GLYPH_CELLS).enumerate() {
                let (sx, sy) = (u32::from(b) % 16 * cell, u32::from(b) / 16 * cell);
                let (dx, dy) = map_glyph_cell(k);
                for row in 0..cell.min(8) {
                    for col in 0..cell.min(8) {
                        let s = (((sy + row) * size + sx + col) * 4) as usize;
                        let d = (((dy + row) * HAND_ATLAS + dx + col) * 4) as usize;
                        if s + 4 <= sheet.len() {
                            rgba[d..d + 4].copy_from_slice(&sheet[s..s + 4]);
                        }
                    }
                }
            }
        }
        let (bx, by) = map_glyph_cell(HAND_MAP_GLYPH_CELLS);
        for row in 0..8 {
            for col in 0..8 {
                let d = (((by + row) * HAND_ATLAS + bx + col) * 4) as usize;
                rgba[d..d + 4].copy_from_slice(&[0, 0, 0, 128]);
            }
        }
        HeldMapUv {
            pixels: atlas_rect_uv(HAND_MAP_PIXELS),
            checker: atlas_rect_uv(HAND_MAP_CHECKER),
            background: atlas_rect_uv(HAND_MAP_BACKGROUND),
            // The cell's centre texel only, so linear or nearest sampling
            // never reaches a neighbour.
            label_background: atlas_rect_uv((bx + 3, by + 3, 2, 2)),
        }
    });
    for (slot, &tex) in wanted.iter().enumerate() {
        let (ox, oy) = (
            (slot as u32 % cols) * GUI_ATLAS_SLOT,
            item_y + (slot as u32 / cols) * GUI_ATLAS_SLOT,
        );
        // The bottom row is the skin's; an item that would land there is
        // dropped rather than overlapping it.
        if oy >= HAND_SKIN_Y {
            continue;
        }
        let Some(src) = held.textures.get(tex as usize) else {
            continue;
        };
        if src.w > GUI_ATLAS_SLOT || src.h > GUI_ATLAS_SLOT {
            continue;
        }
        for y in 0..src.h {
            let s = (y * src.w * 4) as usize;
            let d = (((oy + y) * HAND_ATLAS + ox) * 4) as usize;
            let n = (src.w * 4) as usize;
            rgba[d..d + n].copy_from_slice(&src.rgba[s..s + n]);
        }
        uv.insert(
            tex,
            [
                ox as f32 / HAND_ATLAS as f32,
                oy as f32 / HAND_ATLAS as f32,
                src.w as f32 / HAND_ATLAS as f32,
                src.h as f32 / HAND_ATLAS as f32,
            ],
        );
    }
    if let Some(skin) = skin {
        for y in 0..64u32 {
            let s = (y * 64 * 4) as usize;
            let d = (((HAND_SKIN_Y + y) * HAND_ATLAS + HAND_SKIN_X) * 4) as usize;
            if s + 256 <= skin.len() {
                rgba[d..d + 256].copy_from_slice(&skin[s..s + 256]);
            }
        }
    }
    (rgba, uv, map_uv)
}

/// `ItemInHandRenderer`'s map branch (M192, `:440-452`): which hand draws a
/// map, and whether two-handed. Each argument is `(item id, has MAP_ID)` for
/// the hand's held stack, beside the equip clock's visible item.
///
/// - Vanilla draws `this.mainHandItem` / `offHandItem`, the renderer's own
///   fields, which keep the OLD stack until the swap dip bottoms out. So a
///   hand draws its map only once the clock shows that item
///   (`visible == Some(item)`); mid-dip the outgoing item is still drawn.
/// - Two-handed is `isMainHand && offHandItem.isEmpty()`, the off-hand's
///   VISIBLE item.
/// - The main hand wins when both hands hold maps: the atlas carries one
///   map, so the off-hand's falls back to its flat item model (a recorded
///   deviation; vanilla draws two one-handed maps).
pub(super) fn held_map_branch(
    main: Option<(i32, bool)>,
    main_visible: Option<i32>,
    off: Option<(i32, bool)>,
    off_visible: Option<i32>,
) -> Option<(rewo_gpu::hand::Arm, bool)> {
    use rewo_gpu::hand::Arm;
    match (main, off) {
        (Some((item, true)), _) if main_visible == Some(item) => {
            Some((Arm::Right, off_visible.is_none()))
        }
        (_, Some((item, true))) if off_visible == Some(item) => Some((Arm::Left, false)),
        _ => None,
    }
}

/// Whether the held map moved only in its contents (M192): the same map id,
/// held and present before and after, with a different version. That is the
/// one change a region re-upload can serve.
pub(super) fn map_version_only_changed(
    resident: Option<(i32, u64, bool)>,
    now: Option<(i32, u64, bool)>,
) -> bool {
    matches!(
        (resident, now),
        (Some((a, va, true)), Some((b, vb, true))) if a == b && va != vb
    )
}

/// What [`apply_hand`] drew of a held map (M192), for the render check:
/// the map id, whether its data was present, and how many decorations.
pub(super) type HeldMapDrawn = (i32, bool, usize);

/// Build and upload this frame's hand. Returns the held map it drew, if any.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_hand(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    session: &PlaySession,
    // M213 — the language a held map's banner names flatten with.
    lang: Option<&rewo_data::lang::Language>,
    items: &rewo_data::items::Items,
    state: &mut HandState,
    partial: f32,
    aspect: f32,
) -> Option<HeldMapDrawn> {
    use rewo_gpu::hand::{Arm, HandDraw};

    // The item on screen is the one the equip clock says, which lags the held
    // one across a swap — that is the whole point of the dip.
    //
    // M234 — through the item's use variant: a drawn bow's pull frames, a
    // loading or loaded crossbow, a tooting horn. The charge comes from the
    // inventory stack's detail, and only while the stack on screen is still
    // the one in the hand.
    let local_use = session.local_use_state();
    let model_of = |id: Option<i32>, hand: rewo_world::entities::InteractionHand| {
        let id = id?;
        let stack = match hand {
            rewo_world::entities::InteractionHand::MainHand => session.inventory.held(),
            rewo_world::entities::InteractionHand::OffHand => session.inventory.offhand(),
        };
        let charged = stack
            .filter(|s| s.item_id == id)
            .and_then(|s| session.stack_details.get(s.components))
            .and_then(|d| d.charged_projectile);
        let name = use_model(items.name(id)?, local_use, hand, charged);
        state.held.any(name).map(|m| (name, m))
    };
    let main = model_of(
        state.main_equip.visible_item(),
        rewo_world::entities::InteractionHand::MainHand,
    );
    #[cfg(feature = "gates")]
    {
        state.last_main_model = main.map(|(n, _)| n.to_string());
    }
    let main_item = main.map(|(_, m)| m);
    let off_item = model_of(
        state.off_equip.visible_item(),
        rewo_world::entities::InteractionHand::OffHand,
    )
    .map(|(_, m)| m);

    // M192 — `ItemInHandRenderer`'s map branch: a stack that `has(MAP_ID)`
    // is drawn as a map, two-handed in the main hand with an empty off-hand,
    // one-handed otherwise. The held stacks come from the inventory, as the
    // glint's do; the map's data from the level's store (M190).
    let main_stack = session.inventory.held();
    let off_stack = session.inventory.offhand();
    let held_map = state.map_backgrounds.as_ref().and_then(|_| {
        let (arm, two_handed) = held_map_branch(
            main_stack.map(|s| (s.item_id, s.has_map_id)),
            state.main_equip.visible_item(),
            off_stack.map(|s| (s.item_id, s.has_map_id)),
            state.off_equip.visible_item(),
        )?;
        let stack = match arm {
            Arm::Right => main_stack,
            Arm::Left => off_stack,
        }?;
        let id = session.stack_details.get(stack.components)?.map_id?;
        Some((id, arm, two_handed))
    });
    let map_key = held_map.map(|(id, _, _)| {
        let data = session.maps.get(id);
        (id, data.map_or(0, |d| d.version), data.is_some())
    });
    // M213 — the labels this frame draws, and whether the resident atlas
    // holds their glyphs (a new byte forces the full repack below).
    let (map_labels, label_bytes) = held_map
        .and_then(|(id, _, _)| session.maps.get(id))
        .filter(|_| state.font.is_some())
        .map(|d| held_map_labels(d, lang))
        .unwrap_or_default();
    let glyphs_resident = label_bytes.iter().all(|b| state.map_glyphs.contains(b));

    // Every texture the two hands need this frame.
    let mut wanted: Vec<u16> = Vec::new();
    for m in [main_item, off_item].into_iter().flatten() {
        for q in &m.quads {
            if !wanted.contains(&q.tex) {
                wanted.push(q.tex);
            }
        }
    }
    // M192 — a held map whose contents changed (same map, same presence,
    // newer version) re-uploads only its 128 x 128 band; anything else that
    // moved repacks the atlas.
    let map_only_moved = wanted == state.resident
        && state.skin_resident
        && wr.hand_ready()
        && state.map_uv.is_some()
        && glyphs_resident
        && map_version_only_changed(state.map_resident, map_key);
    if map_only_moved {
        let pixels = map_key
            .and_then(|k| session.maps.get(k.0))
            .map(|d| d.rgba());
        match pixels.map(|px| wr.update_hand_region(gpu, &px, HAND_MAP_PIXELS)) {
            Some(Ok(())) => state.map_resident = map_key,
            Some(Err(e)) => log::warn!("live: held map re-upload failed: {e}"),
            None => {}
        }
    } else if wanted != state.resident
        || !state.skin_resident
        || !wr.hand_ready()
        || map_key != state.map_resident
        || !glyphs_resident
    {
        // M192 — a held map's pixels and backgrounds take the top band. A
        // new patch (version bump) repacks the whole atlas for now; the plan
        // (docs/rewo/M192-held-map-plan.md §2.1) names the region re-upload
        // that should replace this.
        let pixels = map_key
            .filter(|k| k.2)
            .and_then(|k| session.maps.get(k.0))
            .map(|d| d.rgba());
        let deco_sprites: Vec<Option<&[u8]>> = state
            .map_decoration_sprites
            .iter()
            .map(|d| d.as_deref())
            .collect();
        let textures =
            map_key
                .and(state.map_backgrounds.as_ref())
                .map(|(bg, cb)| HeldMapTextures {
                    pixels: pixels.as_deref(),
                    checker: cb,
                    background: bg,
                    decorations: &deco_sprites,
                    font: state.font.as_ref().map(|f| (f.0.as_slice(), f.1, f.2)),
                    label_bytes: &label_bytes,
                });
        let (rgba, uv, map_uv) = pack_hand_atlas(
            &state.held,
            &wanted,
            state.skin.as_ref().map(|(px, _)| px.as_slice()),
            textures.as_ref(),
        );
        if let Err(e) = wr.init_hand(gpu, &rgba, HAND_ATLAS, HAND_ATLAS) {
            log::warn!("live: hand atlas upload failed: {e}");
            return None;
        }
        // **After** `init_hand`, not before: that call destroys and rebuilds
        // the pass, so a glint installed first is thrown away with it. The
        // first build had these the other way round and drew no shimmer at
        // all — the GUI path gets this right by having grown in the same
        // order.
        if let Some(g) = state.glint.as_ref() {
            if let Err(e) = wr.init_hand_glint(gpu, &g.0, g.1, g.2) {
                log::warn!("live: hand glint upload failed: {e}");
            }
        }
        state.uv = uv;
        state.resident = wanted;
        state.skin_resident = state.skin.is_some();
        state.map_resident = map_key;
        state.map_uv = map_uv;
        state.map_glyphs = label_bytes.clone();
    }

    let attack = state
        .forced_attack
        .unwrap_or_else(|| session.local_attack_anim(partial));
    // The rig comes from the item's own `SwingAnimation` — the seven spears
    // STAB, everything else WHACK, and an item whose animation is NONE holds
    // still. Resolved through the same prototype table M19 uses for every
    // other entity's swing, so a spear thrusts in first person exactly as it
    // does in third.
    let swing_kind = |id: Option<i32>| {
        use rewo_data::swing_anim::SwingAnimationType;
        use rewo_gpu::hand::SwingKind;
        match id.and_then(|i| session.swing_data.as_ref().and_then(|d| d.prototypes.of(i))) {
            Some(a) => match a.kind {
                SwingAnimationType::None => SwingKind::None,
                SwingAnimationType::Stab => SwingKind::Stab,
                SwingAnimationType::Whack => SwingKind::Whack,
            },
            // An item this build cannot resolve holds still rather than
            // guessing a rig — the rule the click arithmetic uses too.
            None => SwingKind::None,
        }
    };
    let sway = state
        .bob
        .sway(session.player.pitch, session.player.yaw, partial);
    let view = rewo_gpu::hand::view_sway(sway);

    // The use in progress, if any — it poses exactly one hand, and replaces
    // that hand's swing rather than combining with it.
    let use_state = session.local_use_state();
    let use_for = |hand: rewo_world::entities::InteractionHand| {
        use rewo_data::use_item::ItemUseAnimation as A;
        use rewo_gpu::hand::{UseAnim, UsePose};
        if !use_state.poses_hand(hand) {
            return None;
        }
        let id = use_state.item_id?;
        let profile = session.swing_data.as_ref()?.use_profiles.of(id)?;
        let anim = match profile.animation {
            A::None => UseAnim::None,
            A::Eat => UseAnim::Eat,
            A::Drink => UseAnim::Drink,
            A::Block => UseAnim::Block,
            A::Bow => UseAnim::Bow,
            A::Trident => UseAnim::Trident,
            A::Crossbow => UseAnim::Crossbow,
            A::Spyglass => UseAnim::Spyglass,
            A::TootHorn => UseAnim::TootHorn,
            A::Brush => UseAnim::Brush,
            A::Bundle => UseAnim::Bundle,
            A::Spear => UseAnim::Spear,
        };
        Some(UsePose {
            anim,
            remaining: use_state.remaining_ticks(),
            duration: profile.duration,
        })
    };
    // `case BLOCK` excepts a real shield, which carries its own display
    // transform for the context and would otherwise be posed twice.
    let is_shield = |id: Option<i32>| id.and_then(|i| items.name(i)) == Some("minecraft:shield");
    // `ItemStack.hasFoil()` for each hand (M44). It comes from the
    // **inventory**, not the equipment feed: the server never sends a player
    // their own equipment, which is the same reason M38's swing duration reads
    // the inventory too.
    let main_glint = session.inventory.held().is_some_and(|s| s.enchanted);
    let off_glint = session.inventory.offhand().is_some_and(|s| s.enchanted);

    let hands = [
        HandDraw {
            arm: Arm::Right,
            item: main_item,
            attack,
            inverse_height: state.main_equip.inverse(partial),
            swings: swing_kind(state.main_equip.visible_item()),
            main_hand: true,
            glint: main_glint,
            using: use_for(rewo_world::entities::InteractionHand::MainHand),
            is_shield: is_shield(state.main_equip.visible_item()),
        },
        HandDraw {
            arm: Arm::Left,
            item: off_item,
            // Only the swinging hand animates, and the local player's swing is
            // always the main hand's — `LocalPlayer.swing` passes MAIN_HAND.
            attack: 0.0,
            inverse_height: state.off_equip.inverse(partial),
            swings: swing_kind(state.off_equip.visible_item()),
            main_hand: false,
            glint: off_glint,
            using: use_for(rewo_world::entities::InteractionHand::OffHand),
            is_shield: is_shield(state.off_equip.visible_item()),
        },
    ];
    let arm_geo = state
        .skin
        .as_ref()
        .map(|(_, slim)| rewo_gpu::hand::ArmGeometry {
            skin_uv: [
                HAND_SKIN_X as f32 / HAND_ATLAS as f32,
                HAND_SKIN_Y as f32 / HAND_ATLAS as f32,
                64.0 / HAND_ATLAS as f32,
                64.0 / HAND_ATLAS as f32,
            ],
            slim: *slim,
        });
    // M192 — the map's hand leaves the ordinary list (the map branch
    // replaces the item branch; two-handed, the off-hand is empty and draws
    // nothing anyway) and the map's own geometry is appended.
    let map_draw = held_map
        .zip(state.map_uv)
        .map(|((_, arm, two_handed), uv)| {
            let (inverse_height, attack) = match arm {
                Arm::Right => (state.main_equip.inverse(partial), attack),
                Arm::Left => (state.off_equip.inverse(partial), 0.0),
            };
            rewo_gpu::hand::MapDraw {
                arm,
                two_handed,
                x_rot: session.player.pitch,
                inverse_height,
                attack,
                data_present: map_key.is_some_and(|k| k.2),
                uv_pixels: uv.pixels,
                uv_checker: uv.checker,
                uv_background: uv.background,
            }
        });
    let hands: Vec<HandDraw<'_>> = hands
        .into_iter()
        .filter(|h| map_draw.is_none_or(|m| m.arm != h.arm))
        .collect();
    let mut verts = rewo_gpu::hand::build_vertices(
        view,
        &hands,
        &|t| state.uv.get(&t).copied(),
        arm_geo.as_ref(),
    );
    let mut drawn = None;
    if let Some(m) = map_draw.as_ref() {
        // The map's decorations, in the store's order (vanilla's `count`).
        let decos: Vec<rewo_gpu::hand::MapDecoQuad> = held_map
            .and_then(|(id, _, _)| session.maps.get(id))
            .map(|data| {
                data.decorations
                    .iter()
                    .filter(|d| {
                        state
                            .map_decoration_sprites
                            .get(d.type_id as usize)
                            .is_some_and(|s| s.is_some())
                    })
                    .filter_map(|d| {
                        Some(rewo_gpu::hand::MapDecoQuad {
                            x: d.x,
                            y: d.y,
                            rot: d.rot,
                            uv: map_decoration_uv(d.type_id)?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        // M213 — the banner labels, glyphs from the cells packed above.
        let labels: Vec<rewo_gpu::hand::MapLabelQuad> = match (state.font.as_ref(), state.map_uv) {
            (Some(font), Some(uv)) => map_labels
                .iter()
                .map(|(x, y, text)| {
                    let mut pen = 0.0f32;
                    let mut glyphs = Vec::new();
                    for b in text.bytes() {
                        if b != b' ' {
                            if let Some(k) = state.map_glyphs.iter().position(|&g| g == b) {
                                glyphs.push((map_glyph_uv(k), pen));
                            }
                        }
                        pen += f32::from(font.3[b as usize]);
                    }
                    rewo_gpu::hand::MapLabelQuad {
                        x: *x,
                        y: *y,
                        width: pen,
                        glyphs,
                        background: uv.label_background,
                    }
                })
                .collect(),
            _ => Vec::new(),
        };
        verts.extend(rewo_gpu::hand::build_map_vertices(
            view,
            m,
            &decos,
            &labels,
            arm_geo.as_ref(),
        ));
        drawn = held_map.map(|(id, _, _)| (id, m.data_present, decos.len()));
    }
    // Wall-clock phase, exactly as the GUI glint uses — vanilla reads
    // `Util.getMillis()` for both.
    let millis = state.started.elapsed().as_secs_f64() * 1000.0;
    let glint = rewo_gpu::hand::build_glint_vertices(view, &hands, millis);
    let vp = hand_view_proj(aspect).to_cols_array_2d();
    if let Err(e) = wr.set_hand_with_glint(gpu, &verts, &glint, vp) {
        log::warn!("live: hand upload failed: {e}");
        return None;
    }
    drawn
}

/// State the hotbar icons need across frames: the atlas currently uploaded and
/// what it was built for.
pub struct GuiItemState {
    /// The texture set the resident atlas holds, so a hotbar change that needs
    /// no new textures costs nothing.
    pub(super) resident: Vec<u16>,
    /// Where each resident texture landed, kept alongside so a frame that needs
    /// no new textures does not repack a megabyte of atlas to look them up.
    pub(super) uv: std::collections::HashMap<u16, [f32; 4]>,
    pub(super) lights: rewo_gpu::gui_item::ItemLights,
    /// The app's own copy of the baked items. `WorldRenderer` owns one too, but
    /// building the icons needs `&items` while uploading them needs `&mut wr`,
    /// and one copy cannot be both.
    pub(super) held: rewo_gpu::held::HeldItems,
    /// When this session started, for the glint's wall-clock phase (M43).
    pub(super) started: std::time::Instant,
    /// `misc/enchanted_glint_item.png` as `(rgba, w, h)`; `None` draws none.
    pub(super) glint: Option<(Vec<u8>, u32, u32)>,
}

impl GuiItemState {
    pub fn new(baked: &assets::BakedAssets) -> Self {
        Self {
            resident: Vec::new(),
            uv: std::collections::HashMap::new(),
            lights: rewo_gpu::gui_item::ItemLights::default(),
            held: to_gpu_held_items(&baked.held_items),
            started: std::time::Instant::now(),
            glint: baked.glint.as_ref().map(|i| (i.rgba.clone(), i.w, i.h)),
        }
    }
}

/// Place, shade and upload this frame's hotbar icons.
///
/// Rebuilds the atlas only when the hotbar needs a texture it does not hold —
/// switching between two swords you already carry costs one vertex upload.
pub(super) fn apply_hotbar_icons(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    session: &PlaySession,
    items: &rewo_data::items::Items,
    state: &mut GuiItemState,
    extent: (f32, f32),
) {
    let held = &state.held;
    let names = hotbar_models(&session.inventory, items, &session.trim_materials);
    let models: Vec<String> = names.iter().flatten().map(|n| n.to_string()).collect();
    let wanted = gui_atlas_wanted(held, &models);

    // Repack only when the hotbar needs a texture the resident atlas does not
    // hold. Switching between two swords you already carry costs one vertex
    // upload; packing every frame would rebuild a megabyte for nothing.
    if wanted != state.resident || !wr.gui_items_ready() {
        let atlas = pack_gui_atlas(held, &wanted);
        if let Err(e) = wr.init_gui_items(gpu, &atlas.rgba, GUI_ATLAS_W, GUI_ATLAS_H) {
            log::warn!("live: gui-item atlas upload failed: {e}");
            return;
        }
        // The glint rides on the item pass, and the item pass is rebuilt
        // whenever its atlas is repacked — so the glint is rebuilt here too.
        if let Some(g) = state.glint.as_ref() {
            if let Err(e) = wr.init_gui_glint(gpu, &g.0, g.1, g.2) {
                log::warn!("live: glint upload failed: {e}");
            }
        }
        // The UVs come from the same packing the atlas was built from, so the
        // two cannot disagree.
        state.uv = atlas.uv;
        state.resident = wanted;
    }
    let slots = rewo_gpu::hud::hotbar_slot_rects(182.0, 22.0, extent.0, extent.1);
    let gui: Vec<rewo_gpu::gui_item::GuiItem> = names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            n.as_ref().map(|name| rewo_gpu::gui_item::GuiItem {
                model: name.clone(),
                x: slots[i].0,
                y: slots[i].1,
                size: slots[i].2,
                glint: session.inventory.hotbar(i).is_some_and(|s| s.enchanted),
            })
        })
        .collect();
    upload_gui_icons(wr, gpu, state, &gui);
    // The nine hotbar stacks' durability bars, in the same rects the icons
    // were placed from.
    let stacks: Vec<_> = (0..rewo_world::inventory::HOTBAR_SIZE)
        .map(|i| (session.inventory.hotbar(i), slots[i]))
        .collect();
    // `!has(UNBREAKABLE)`. No item's *prototype* carries the component in
    // 26.2 — it is only ever patched on — so the patch flag is the whole
    // answer here.
    wr.set_item_bars(item_bars(&stacks, items, |s| {
        session.inventory.text_of(s).is_some_and(|t| t.unbreakable)
    }));
}

/// Place, shade and upload an arbitrary list of icons (M35).
///
/// The screen's 46 slots and the hotbar's nine go through exactly this, which
/// is why the pass never learns which it is drawing.
pub(super) fn apply_gui_icons(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    state: &mut GuiItemState,
    icons: &[rewo_gpu::gui_item::GuiItem],
) {
    let names: Vec<String> = icons.iter().map(|i| i.model.clone()).collect();
    let wanted = gui_atlas_wanted(&state.held, &names);
    if wanted != state.resident || !wr.gui_items_ready() {
        let atlas = pack_gui_atlas(&state.held, &wanted);
        if let Err(e) = wr.init_gui_items(gpu, &atlas.rgba, GUI_ATLAS_W, GUI_ATLAS_H) {
            log::warn!("live: gui-item atlas upload failed: {e}");
            return;
        }
        // The glint rides on the item pass, and the item pass is rebuilt
        // whenever its atlas is repacked — so the glint is rebuilt here too.
        if let Some(g) = state.glint.as_ref() {
            if let Err(e) = wr.init_gui_glint(gpu, &g.0, g.1, g.2) {
                log::warn!("live: glint upload failed: {e}");
            }
        }
        state.uv = atlas.uv;
        state.resident = wanted;
    }
    upload_gui_icons(wr, gpu, state, icons);
}

pub(super) fn upload_gui_icons(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    state: &GuiItemState,
    icons: &[rewo_gpu::gui_item::GuiItem],
) {
    let verts = rewo_gpu::gui_item::build_vertices(&state.held, icons, &state.lights, &|t| {
        state.uv.get(&t).copied()
    });
    // The glint's phase is wall-clock, not the game tick: vanilla reads
    // `Util.getMillis()` directly, so it keeps scrolling on a paused screen
    // and does not stutter with the tick rate (M43).
    let millis = state.started.elapsed().as_secs_f64() * 1000.0;
    let glint = rewo_gpu::gui_item::build_glint_vertices(&state.held, icons, millis);
    if let Err(e) = wr.set_gui_items_with_glint(gpu, &verts, &glint) {
        log::warn!("live: gui-item upload failed: {e}");
    }
}
