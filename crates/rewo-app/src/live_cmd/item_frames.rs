//! Item frames (M194) — `ItemFrameRenderer.submit` as entity-pass draws.
//!
//! A frame is its block model (`block/item_frame` and kin, baked as
//! `rewo:be/…` models) plus its item through the FIXED display context. Both
//! are static quads at a pose, so they ride the block-entity emitter with the
//! poses `rewo_gpu::item_frame` derives from the decompile, rather than a new
//! draw path. The entity itself has no body: `collect_entities` skips it.

use super::*;

/// Whether a type id is one of the two frame types `ItemFrameRenderer`
/// draws.
pub(crate) fn frame_kind(etypes: &EntityTypes, type_id: i32) -> Option<FrameKind> {
    if Some(type_id) == etypes.id_of("minecraft:item_frame") {
        Some(FrameKind { glow: false })
    } else if Some(type_id) == etypes.id_of("minecraft:glow_item_frame") {
        Some(FrameKind { glow: true })
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameKind {
    /// `entity.is(EntityTypes.GLOW_ITEM_FRAME)`.
    pub glow: bool,
}

/// `LightCoordsUtil.pack(15, 15)` (`15728880`), the light a glow frame's
/// item is drawn at.
const FULL_BRIGHT: (u8, u8) = (15, 15);

/// `15728850` (`15728880 - BRIGHT_MAP_LIGHT_ADJUSTMENT`), a glow frame's map:
/// sky 15, block coordinate 210 = level 13.125, taken at the level below
/// because Rewo's lightmap sample takes whole levels (a recorded deviation of
/// an eighth of a level).
const GLOW_MAP_LIGHT: (u8, u8) = (13, 15);

/// `ItemFrameRenderer.getBlockLightLevel`: a glow frame's block light is at
/// least `GLOW_FRAME_BRIGHTNESS` (5).
pub(crate) const GLOW_FRAME_BRIGHTNESS: u8 = 5;

/// One frame's resolved inputs — what `extractRenderState` reads.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FrameInputs {
    pub pos: [f32; 3],
    pub facing: u8,
    pub rotation: u8,
    pub invisible: bool,
    pub glow: bool,
    /// The framed item's registry name; `None` for an empty frame.
    pub item: Option<String>,
    /// `ItemStack.hasFoil()` for the framed stack (M198).
    pub foil: bool,
    /// `state.mapId != null`: the item is a map AND the level holds its data —
    /// its id, its pixel version, and the decorations a frame shows.
    pub map: Option<FramedMap>,
    /// `(block, sky)` light at the entity.
    pub light: (u8, u8),
}

/// A framed map with data.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FramedMap {
    pub id: i32,
    pub version: u64,
    /// `MapRenderer.render(..., showOnlyFrame = true, ...)`: only the
    /// decorations whose type is `showOnItemFrame`, in the map's order.
    pub decorations: Vec<rewo_gpu::entities::FrameMapDecoration>,
    /// Their names (M204): a named banner's label, flattened.
    pub labels: Vec<rewo_gpu::entities::MapLabel>,
}

/// A [`rewo_gpu::entities::FrameMapDraw`] that owns its decorations.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OwnedFrameMap {
    pub pos: [f32; 3],
    pub transform: [[f32; 4]; 3],
    pub map_id: i32,
    pub version: u64,
    pub light: [f32; 3],
    pub decorations: Vec<rewo_gpu::entities::FrameMapDecoration>,
    pub labels: Vec<rewo_gpu::entities::MapLabel>,
}

impl OwnedFrameMap {
    pub fn as_draw(&self) -> rewo_gpu::entities::FrameMapDraw<'_> {
        rewo_gpu::entities::FrameMapDraw {
            pos: self.pos,
            transform: self.transform,
            map_id: self.map_id,
            light: self.light,
            decorations: &self.decorations,
            labels: &self.labels,
        }
    }
}

/// Every frame's draws for one frame.
#[derive(Default)]
pub(crate) struct ItemFrameDraws {
    /// Frame models and framed items, through the block-entity emitter.
    pub models: Vec<OwnedBlockEntityDraw>,
    pub maps: Vec<OwnedFrameMap>,
}

/// `MapDecorationType.showOnItemFrame` for a raw type id (M193's table).
pub(crate) fn shows_on_frame(type_id: i32) -> bool {
    usize::try_from(type_id)
        .ok()
        .and_then(|i| rewo_data::map_decoration_table::TYPES.get(i))
        .is_some_and(|t| t.2)
}

/// The draws one frame contributes: the frame model unless invisible, then
/// a map with data, else the item.
pub(crate) fn frame_draws(f: &FrameInputs, lightmap: &LightmapState, out: &mut ItemFrameDraws) {
    use rewo_gpu::item_frame;
    let (block, sky) = f.light;
    let frame_block = if f.glow {
        block.max(GLOW_FRAME_BRIGHTNESS)
    } else {
        block
    };
    let lit = |(b, s): (u8, u8)| {
        let mut rgb = sample(b, s, lightmap);
        for c in &mut rgb {
            if !c.is_finite() {
                *c = 0.0;
            }
        }
        rgb
    };
    let draw = |model: String, transform, light, fixed_display, glint| OwnedBlockEntityDraw {
        pos: f.pos,
        model,
        transform,
        light,
        part_transforms: [rewo_data::be_transform::IDENTITY; rewo_gpu::entities::MAX_PARTS],
        part_pivots: [[0.0; 3]; rewo_gpu::entities::MAX_PARTS],
        tint: [1.0; 3],
        fixed_display,
        glint,
        white: 0.0,
    };
    if !f.invisible {
        if let Some(t) = item_frame::frame_transform(f.facing) {
            out.models.push(draw(
                rewo_data::block_entity_models::item_frame_model(f.glow, f.map.is_some())
                    .to_string(),
                t,
                lit((frame_block, sky)),
                false,
                false,
            ));
        }
    }
    if let Some(map) = &f.map {
        if let Some(pose) = item_frame::map_pose(f.facing, f.rotation, f.invisible) {
            // `getLightCoords(isGlowFrame, 15728850, lightCoords)`.
            let light = if f.glow {
                GLOW_MAP_LIGHT
            } else {
                (frame_block, sky)
            };
            out.maps.push(OwnedFrameMap {
                pos: f.pos,
                transform: item_frame::affine(pose),
                map_id: map.id,
                version: map.version,
                light: lit(light),
                decorations: map.decorations.clone(),
                labels: map.labels.clone(),
            });
        }
    } else if let Some(item) = &f.item {
        if let Some(t) = item_frame::item_transform(f.facing, f.rotation, f.invisible) {
            // `getLightCoords(isGlowFrame, 15728880, lightCoords)`.
            let light = if f.glow {
                FULL_BRIGHT
            } else {
                (frame_block, sky)
            };
            out.models
                .push(draw(item.clone(), t, lit(light), true, f.foil));
        }
    }
}

/// Every item frame in the level, resolved for [`frame_draws`].
pub(crate) fn collect_frame_inputs(
    world: &rewo_world::World,
    maps: &rewo_world::maps::MapStore,
    // M204 — the language table a banner name's component is flattened with.
    lang: Option<&rewo_data::lang::Language>,
    etypes: &EntityTypes,
    item_names: &rewo_data::items::Items,
    alpha: f32,
) -> Vec<FrameInputs> {
    let ents = &world.entities;
    let mut out = Vec::new();
    for (id, e) in ents.iter() {
        let Some(kind) = frame_kind(etypes, e.type_id) else {
            continue;
        };
        let Some((facing, rotation, item)) = ents.item_frame(id) else {
            continue;
        };
        let p = e.render_pos(alpha);
        let map = item.and_then(|i| i.map_id).and_then(|id| {
            let data = maps.get(id)?;
            Some(FramedMap {
                id,
                version: data.version,
                decorations: data
                    .decorations
                    .iter()
                    .filter(|d| shows_on_frame(d.type_id))
                    .map(|d| rewo_gpu::entities::FrameMapDecoration {
                        type_id: d.type_id,
                        x: d.x,
                        y: d.y,
                        rot: d.rot,
                    })
                    .collect(),
                labels: data
                    .decorations
                    .iter()
                    .filter(|d| shows_on_frame(d.type_id))
                    .filter_map(|d| {
                        let text = rewo_world::chat_style::flatten(d.name.as_ref()?, lang);
                        Some(rewo_gpu::entities::MapLabel {
                            x: d.x,
                            y: d.y,
                            text,
                        })
                    })
                    .collect(),
            })
        });
        out.push(FrameInputs {
            pos: [p[0] as f32, p[1] as f32, p[2] as f32],
            facing,
            rotation,
            invisible: ents.is_invisible(id),
            glow: kind.glow,
            item: item
                .and_then(|i| item_names.name(i.item_id))
                .map(str::to_string),
            foil: item.is_some_and(|i| i.foil),
            map,
            light: world.light_at(
                p[0].floor() as i32,
                p[1].floor() as i32,
                p[2].floor() as i32,
            ),
        });
    }
    // Stable order (the table is a hash map), so a still frame's vertex soup
    // is the same from run to run.
    out.sort_by(|a, b| {
        a.pos[0]
            .total_cmp(&b.pos[0])
            .then(a.pos[1].total_cmp(&b.pos[1]))
            .then(a.pos[2].total_cmp(&b.pos[2]))
            .then(a.facing.cmp(&b.facing))
    });
    out
}

/// The draws for every frame in the level.
pub(crate) fn collect_item_frames(
    world: &rewo_world::World,
    maps: &rewo_world::maps::MapStore,
    lang: Option<&rewo_data::lang::Language>,
    etypes: &EntityTypes,
    item_names: &rewo_data::items::Items,
    lightmap: &LightmapState,
    alpha: f32,
) -> ItemFrameDraws {
    let mut out = ItemFrameDraws::default();
    for f in collect_frame_inputs(world, maps, lang, etypes, item_names, alpha) {
        frame_draws(&f, lightmap, &mut out);
    }
    out
}

/// Upload this frame's framed maps' pixels (the ones whose version moved)
/// before the draws are built — the entity pass samples them from its map
/// band.
pub(crate) fn prepare_frame_maps(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    frames: &ItemFrameDraws,
    maps: &rewo_world::maps::MapStore,
) {
    let wants: Vec<(i32, u64)> = frames.maps.iter().map(|m| (m.map_id, m.version)).collect();
    if let Err(e) = wr.prepare_frame_maps(gpu, &wants, &mut |id| maps.get(id).map(|m| m.rgba())) {
        log::warn!("live: framed map upload failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(glow: bool, invisible: bool, map: Option<FramedMap>) -> FrameInputs {
        FrameInputs {
            pos: [1.0, 2.0, 3.0],
            facing: 3,
            rotation: 0,
            invisible,
            glow,
            item: Some("minecraft:stick".into()),
            foil: false,
            map,
            light: (7, 9),
        }
    }

    fn map() -> FramedMap {
        FramedMap {
            id: 4,
            version: 2,
            decorations: Vec::new(),
            labels: Vec::new(),
        }
    }

    /// `submit`: the frame model unless invisible, then the map when it has
    /// data, else the item at the FIXED context.
    #[test]
    fn a_frame_draws_its_model_then_a_map_or_its_item() {
        let lm = LightmapState::default();
        let mut out = ItemFrameDraws::default();
        frame_draws(&inputs(false, false, None), &lm, &mut out);
        let names: Vec<&str> = out.models.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(names, ["rewo:be/item_frame", "minecraft:stick"]);
        assert_eq!(
            out.models
                .iter()
                .map(|m| m.fixed_display)
                .collect::<Vec<_>>(),
            [false, true]
        );
        assert!(out.maps.is_empty());

        let mut out = ItemFrameDraws::default();
        frame_draws(&inputs(true, false, Some(map())), &lm, &mut out);
        let names: Vec<&str> = out.models.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(
            names,
            ["rewo:be/glow_item_frame_map"],
            "no item beside a map"
        );
        assert_eq!((out.maps[0].map_id, out.maps[0].version), (4, 2));
        assert_eq!(out.maps[0].pos, [1.0, 2.0, 3.0]);

        let mut out = ItemFrameDraws::default();
        frame_draws(&inputs(false, true, None), &lm, &mut out);
        let names: Vec<&str> = out.models.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(
            names,
            ["minecraft:stick"],
            "an invisible frame has no model"
        );
    }

    /// M198: a foil stack's glint goes on the framed item, never on the frame.
    #[test]
    fn a_foil_item_glints_and_its_frame_does_not() {
        let lm = LightmapState::default();
        let mut f = inputs(false, false, None);
        f.foil = true;
        let mut out = ItemFrameDraws::default();
        frame_draws(&f, &lm, &mut out);
        let glints: Vec<(&str, bool)> = out
            .models
            .iter()
            .map(|m| (m.model.as_str(), m.glint))
            .collect();
        assert_eq!(
            glints,
            [("rewo:be/item_frame", false), ("minecraft:stick", true)]
        );
    }

    /// `MapDecorationType.showOnItemFrame`: player arrows and the off-map
    /// markers stay off a framed map; frame markers and banners show.
    #[test]
    fn only_frame_decorations_show_on_a_framed_map() {
        assert!(!shows_on_frame(0), "player");
        assert!(shows_on_frame(1), "frame");
        assert!(!shows_on_frame(6), "player_off_map");
        assert!(shows_on_frame(10), "banner_white");
        assert!(!shows_on_frame(-1) && !shows_on_frame(10_000));
    }

    /// The glow frame's three light constants: block >= 5 for the frame,
    /// 15728880 for the item, 15728850 for a map.
    #[test]
    fn a_glow_frame_lights_its_contents() {
        let lm = LightmapState::default();
        let s = |b, k| sample(b, k, &lm);
        let mut out = ItemFrameDraws::default();
        frame_draws(&inputs(true, false, None), &lm, &mut out);
        assert_eq!(out.models[0].light, s(7, 9), "block 7 is already >= 5");
        assert_eq!(out.models[1].light, s(15, 15));
        let mut dim = inputs(true, false, Some(map()));
        dim.light = (2, 9);
        let mut out = ItemFrameDraws::default();
        frame_draws(&dim, &lm, &mut out);
        assert_eq!(out.models[0].light, s(5, 9));
        assert_eq!(out.maps[0].light, s(GLOW_MAP_LIGHT.0, GLOW_MAP_LIGHT.1));
    }
}
