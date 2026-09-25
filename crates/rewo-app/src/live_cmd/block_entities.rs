use super::*;

pub(super) fn collect_block_entities(
    world: &rewo_world::World,
    chests: &rewo_data::chest_states::ChestStates,
    lightmap: &LightmapState,
    alpha: f32,
    game_time: i64,
    // The camera's right and up axes. An active conduit's EYE is a billboard
    // — the one input in this whole path that is a property of the VIEW rather
    // than of the block (M30).
    cam: ([f32; 3], [f32; 3]),
) -> Vec<OwnedBlockEntityDraw> {
    use rewo_data::chest_states::BlockEntityAnim;
    let mut out = Vec::new();
    for (pos, be) in world.block_entities.iter() {
        let state = world.block_state_at(pos.x, pos.y, pos.z);
        let Some(draw) = chests.draw_for(state) else {
            continue;
        };
        // A pot's wobble is a BLOCK-level rotation composed after its facing,
        // not a part animation — the whole pot rocks (M29). It turns about
        // `(0.5, 0, 0.5)`, the block's FLOOR centre, where the facing turns
        // about `(0.5, 0.5, 0.5)`: a pot rocks on its base like a real one.
        let be_transform = match world
            .block_entities
            .pot_wobble(*pos, game_time, alpha)
        {
            Some((style, progress)) if draw.anim == BlockEntityAnim::DecoratedPot => {
                let st = match style {
                    rewo_world::block_entities::PotWobble::Positive => {
                        rewo_data::be_transform::WobbleStyle::Positive
                    }
                    rewo_world::block_entities::PotWobble::Negative => {
                        rewo_data::be_transform::WobbleStyle::Negative
                    }
                };
                rewo_data::be_transform::mul(
                    &rewo_data::be_transform::pot_wobble(st, progress),
                    &draw.transform,
                )
            }
            _ => draw.transform,
        };

        // A skull's animation counter, which only runs while its block state
        // is POWERED, drives a piglin head's ears and a dragon head's jaw.
        let skull_anim = world.block_entities.skull_animation(*pos, alpha);

        // A decorated pot is five draws, not one: the base, plus a side plane
        // per face with its own sherd texture. They share the pot's own
        // transform and differ by the side pose, so each rides the emitter's
        // animated-group slot rather than needing a new draw path.
        // A banner is the woodwork, then the bare cloth, then the base colour
        // as a mask, then one draw per pattern layer — each the SAME flag
        // geometry with a different greyscale sprite and a different dye.
        if let BlockEntityAnim::Banner {
            base_color,
            standing,
        } = draw.anim
        {
            use rewo_data::block_entity_models as bem;
            let light = entity_light(
                world,
                pos.x as f64 + 0.5,
                pos.y as f64 + 0.5,
                pos.z as f64 + 0.5,
                lightmap,
            );
            let flag = if standing {
                bem::BANNER_STANDING_FLAG_MODEL
            } else {
                bem::BANNER_WALL_FLAG_MODEL
            };
            let suffix = if standing { "" } else { "_wall" };
            // The sway (M29). Every cloth draw — the bare flag and each
            // pattern layer — shares ONE phase, or the layers would drift
            // apart and the pattern would slide off the flag it is painted on.
            let phase = rewo_data::be_transform::banner_phase(
                pos.x, pos.y, pos.z, game_time, alpha,
            );
            let flag_pivot = if standing {
                bem::BANNER_STANDING_FLAG_PIVOT
            } else {
                bem::BANNER_WALL_FLAG_PIVOT
            };
            let (xs, ps) = one_part(
                bem::BANNER_FLAG_PART,
                rewo_data::be_transform::banner_flag_part(phase, flag_pivot),
                flag_pivot,
            );
            let mut layer = |model: String, tint: [f32; 3]| OwnedBlockEntityDraw {
                pos: [pos.x as f32, pos.y as f32, pos.z as f32],
                model,
                transform: be_transform,
                light,
                part_transforms: xs,
                part_pivots: ps,
                tint,
            };
            // The bare cloth, untinted — its own texture carries its colour.
            out.push(layer(flag.to_string(), [1.0; 3]));
            // `Sheets.BANNER_PATTERN_BASE` masked with the banner's own dye,
            // drawn before any pattern.
            out.push(layer(
                format!("rewo:be/banner_pattern/base{suffix}"),
                dye_linear(base_color as usize),
            ));
            // Up to sixteen — `submitPatterns` stops there regardless of how
            // many the tag carries.
            for (pattern, colour) in banner_layers(be).into_iter().take(16) {
                let Some(name) = bem::banner_pattern_model(&pattern) else {
                    // An unknown pattern is skipped rather than drawn as some
                    // other one; a wrong banner is worse than a plain one.
                    continue;
                };
                out.push(layer(format!("{name}{suffix}"), dye_linear(colour)));
            }
        }
        // An ACTIVE conduit replaces its dormant shell with four draws: a
        // tumbling cage, the wind shroud twice, and a camera-facing eye whose
        // pupil opens once the frame is complete (M30).
        if draw.model == rewo_data::block_entity_models::CONDUIT.0 {
            let c = world.block_entities.conduit(*pos);
            if c.shape.active() {
                let light = entity_light(
                    world,
                    pos.x as f64 + 0.5,
                    pos.y as f64 + 0.5,
                    pos.z as f64 + 0.5,
                    lightmap,
                );
                let t = c.anim_time(alpha);
                let mut piece = |model: &str, xf: rewo_data::be_transform::Affine| {
                    out.push(OwnedBlockEntityDraw {
                        pos: [pos.x as f32, pos.y as f32, pos.z as f32],
                        model: model.to_string(),
                        transform: xf,
                        light,
                        part_transforms: [rewo_data::be_transform::IDENTITY;
                            rewo_gpu::entities::MAX_PARTS],
                        part_pivots: [[0.0; 3]; rewo_gpu::entities::MAX_PARTS],
                        tint: [1.0; 3],
                    });
                };
                piece(
                    "rewo:be/conduit_cage",
                    rewo_data::be_transform::conduit_cage(c.rotation(alpha), t),
                );
                // Phase 1 uses the vertical texture; the other two the
                // horizontal one. Both copies of the shroud share it.
                let wind = if c.phase() == 1 {
                    "rewo:be/conduit_wind_vertical"
                } else {
                    "rewo:be/conduit_wind"
                };
                piece(wind, rewo_data::be_transform::conduit_wind(c.phase(), false));
                piece(wind, rewo_data::be_transform::conduit_wind(c.phase(), true));
                piece(
                    if c.shape.hunting() {
                        "rewo:be/conduit_eye_open"
                    } else {
                        "rewo:be/conduit_eye_closed"
                    },
                    rewo_data::be_transform::conduit_eye(t, cam.0, cam.1),
                );
                // The dormant shell is NOT drawn alongside — vanilla's two
                // branches are exclusive.
                continue;
            }
        }
        if draw.anim == BlockEntityAnim::DecoratedPot {
            let sherds = pot_sherds(be);
            for (i, item) in sherds.iter().enumerate() {
                out.push(OwnedBlockEntityDraw {
                    pos: [pos.x as f32, pos.y as f32, pos.z as f32],
                    model: rewo_data::block_entity_models::pot_side_model(item.as_deref()),
                    // The wobble rocks the WHOLE pot, sides included — a base
                    // that rocked while its sherds stayed put would come apart.
                    transform: be_transform,
                    light: entity_light(
                        world,
                        pos.x as f64 + 0.5,
                        pos.y as f64 + 0.5,
                        pos.z as f64 + 0.5,
                        lightmap,
                    ),
                    part_transforms: one_part(
                        rewo_data::block_entity_models::POT_SIDE_PART,
                        rewo_data::be_transform::pot_side(i),
                        [0.0; 3],
                    )
                    .0,
                    part_pivots: [[0.0; 3]; rewo_gpu::entities::MAX_PARTS],
                    tint: [1.0; 3],
                });
            }
        }
        let parts = match draw.anim {
            // The pot's BASE has no animated group; its four sides are the
            // separate draws pushed above, each with its own side pose.
            BlockEntityAnim::None
            | BlockEntityAnim::DecoratedPot
            | BlockEntityAnim::Banner { .. } => skull_parts(&draw.model, skull_anim),
            BlockEntityAnim::ChestLid(c) => one_part(
                rewo_data::block_entity_models::CHEST_LID_PART,
                rewo_data::be_transform::chest_lid_part(
                    chest_openness(world, chests, *pos, c, alpha),
                    rewo_data::block_entity_models::CHEST_LID_PIVOT,
                ),
                rewo_data::block_entity_models::CHEST_LID_PIVOT,
            ),
            BlockEntityAnim::ShulkerLid => one_part(
                rewo_data::block_entity_models::SHULKER_LID_PART,
                rewo_data::be_transform::shulker_lid_part(
                    world.block_entities.shulker(*pos).progress(alpha),
                ),
                rewo_data::block_entity_models::SHULKER_LID_PIVOT,
            ),
        };
        out.push(OwnedBlockEntityDraw {
            pos: [pos.x as f32, pos.y as f32, pos.z as f32],
            model: draw.model,
            transform: be_transform,
            // Lit from the block's own cell — a chest fills its block, so
            // there is no neighbour to sample the way a flat model would need.
            light: entity_light(
                world,
                pos.x as f64 + 0.5,
                pos.y as f64 + 0.5,
                pos.z as f64 + 0.5,
                lightmap,
            ),
            // Each animated group builds its own transform. Both clocks are
            // driven by `block_event` and both are animated client-side, but
            // they are not the same clock and not the same motion — see
            // `rewo_world::block_entities::ShulkerAnim`.
            // Each animated group builds its own transform. The clocks are
            // genuinely different — a chest lid converges, a shulker lid runs
            // a four-state machine, a banner hashes the world clock — which is
            // why this is a match rather than one shared animator.
            part_transforms: parts.0,
            part_pivots: parts.1,
            // Only a banner's pattern layers are tinted; every other model's
            // texture already carries its colour.
            tint: [1.0; 3],
        });
    }
    out
}

/// `ChestBlock.opennessCombiner` — the openness a chest renders with.
///
/// ```text
/// acceptDouble(a, b) -> max(a.getOpenNess(t), b.getOpenNess(t))
/// acceptSingle(a)    -> a.getOpenNess(t)
/// ```
///
/// The **max over the pair** is the whole reason `DoubleBlockCombiner` appears
/// in `ChestRenderer` at all — it is not how the half-models are chosen (the
/// block's own `type` does that), it is how both halves of a double chest open
/// together when only one of them received the event. M25 recorded the
/// combiner as the blocker for drawing halves; that was wrong, and this is
/// what it actually does.
///
/// `ChestBlock.getConnectedDirection`:
/// `type == LEFT ? facing.getClockWise() : facing.getCounterClockWise()`.
pub(super) fn chest_openness(
    world: &rewo_world::World,
    chests: &rewo_data::chest_states::ChestStates,
    pos: rewo_world::block_entities::BlockEntityPos,
    chest: rewo_data::chest_states::ChestState,
    alpha: f32,
) -> f32 {
    use rewo_data::chest_states::ChestType;
    let mine = world.block_entities.lid(pos).openness(alpha);
    let dir = match chest.kind {
        ChestType::Single => return mine,
        ChestType::Left => clockwise(chest.facing),
        ChestType::Right => counter_clockwise(chest.facing),
    };
    let (dx, dz) = step(dir);
    let other = rewo_world::block_entities::BlockEntityPos {
        x: pos.x + dx,
        y: pos.y,
        z: pos.z + dz,
    };
    // Only pair with a block that really is the other half. A LEFT whose
    // neighbour is not a chest is a half-broken pair mid-update, and taking
    // its (absent) lid as 0 is the same answer `acceptNone` gives.
    let paired = chests
        .get(world.block_state_at(other.x, other.y, other.z))
        .is_some_and(|o| o.kind != ChestType::Single);
    if !paired {
        return mine;
    }
    mine.max(world.block_entities.lid(other).openness(alpha))
}

/// `Direction.getClockWise()` for the four horizontals.
pub(super) fn clockwise(f: rewo_data::chest_states::ChestFacing) -> rewo_data::chest_states::ChestFacing {
    use rewo_data::chest_states::ChestFacing as F;
    match f {
        F::North => F::East,
        F::East => F::South,
        F::South => F::West,
        F::West => F::North,
    }
}

/// `Direction.getCounterClockWise()`.
pub(super) fn counter_clockwise(
    f: rewo_data::chest_states::ChestFacing,
) -> rewo_data::chest_states::ChestFacing {
    use rewo_data::chest_states::ChestFacing as F;
    match f {
        F::North => F::West,
        F::West => F::South,
        F::South => F::East,
        F::East => F::North,
    }
}

/// `Direction`'s `(stepX, stepZ)` for the four horizontals.
pub(super) fn step(f: rewo_data::chest_states::ChestFacing) -> (i32, i32) {
    use rewo_data::chest_states::ChestFacing as F;
    match f {
        F::North => (0, -1),
        F::South => (0, 1),
        F::West => (-1, 0),
        F::East => (1, 0),
    }
}

/// One rendered sign line, owning its text (M25e).
pub(crate) struct OwnedSignLine {
    pub transform: rewo_data::be_transform::Affine,
    pub text: String,
    /// Baseline origin in font px. `x` is `-width/2` plus, for an outline
    /// copy, its offset; the caller no longer re-centres.
    pub x: f32,
    pub y: f32,
    /// Depth along the transform's third axis, in font px — negative for the
    /// outline copies so they sit behind the glyphs (M27).
    pub z: f32,
    pub color: [f32; 3],
    pub light: [f32; 3],
}


/// Every end portal and gateway in the world, as geometry for the M32 pass.
///
/// They leave `collect_block_entities` entirely: their shader samples in
/// screen space from two textures, so there is nothing for the block-entity
/// emitter — which wants one texture and a UV — to do with them.
pub(crate) fn collect_end_portals(
    world: &rewo_world::World,
    portal_states: &std::collections::HashSet<u32>,
    gateway_states: &std::collections::HashSet<u32>,
) -> Vec<rewo_gpu::end_portal::PortalDraw> {
    let mut out = Vec::new();
    for (pos, _be) in world.block_entities.iter() {
        let state = world.block_state_at(pos.x, pos.y, pos.z);
        let is_portal = portal_states.contains(&state);
        if !is_portal && !gateway_states.contains(&state) {
            continue;
        }
        // The portal's slab transform is applied here rather than in the
        // shader, because the shader's vertex stage is position-only and
        // vanilla's `TRANSFORMATION` is a poseStack push.
        let xf = if is_portal {
            rewo_data::be_transform::end_portal()
        } else {
            rewo_data::be_transform::end_gateway()
        };
        let verts = rewo_data::block_entity_models::end_portal_positions(is_portal)
            .into_iter()
            .map(|p| {
                let v = [
                    xf[0][0] * p[0] + xf[0][1] * p[1] + xf[0][2] * p[2] + xf[0][3],
                    xf[1][0] * p[0] + xf[1][1] * p[1] + xf[1][2] * p[2] + xf[1][3],
                    xf[2][0] * p[0] + xf[2][1] * p[1] + xf[2][2] * p[2] + xf[2][3],
                ];
                rewo_gpu::end_portal::PortalVertex {
                    pos: [
                        pos.x as f32 + v[0],
                        pos.y as f32 + v[1],
                        pos.z as f32 + v[2],
                    ],
                }
            })
            .collect();
        out.push(rewo_gpu::end_portal::PortalDraw {
            verts,
            layers: if is_portal {
                rewo_gpu::end_portal::PORTAL_LAYERS
            } else {
                rewo_gpu::end_portal::GATEWAY_LAYERS
            },
        });
    }
    out
}

/// A spawner's display-entity id, from `SpawnData` (M31).
///
/// ```text
/// SpawnData.CODEC:  { "entity": CompoundTag, ... }
/// getOrCreateDisplayEntity: if (entityToSpawn.getString("id").isEmpty()) return null;
/// ```
///
/// So the id lives two levels down, and an **empty or absent** one means the
/// spawner has no display entity at all rather than a default — vanilla
/// returns null and draws nothing.
pub(crate) fn spawner_entity_id(be: &rewo_world::block_entities::BlockEntity) -> Option<String> {
    let id = be
        .data
        .get("SpawnData")?
        .get("entity")?
        .get("id")
        .and_then(rewo_proto::nbt::Nbt::as_str)?;
    (!id.is_empty()).then(|| id.to_string())
}

/// Every spawner's caged mob, as mounted entity draws (M31).
///
/// Separate from `collect_block_entities` because the result is an
/// `EntityDraw`, not a block-entity one: the mob rides the **entity** pass, so
/// it gets the same models, rigs and animations every other mob does. The only
/// difference is that its position comes from a mount matrix rather than from
/// the world.
pub(crate) fn collect_spawner_mobs<'a>(
    world: &rewo_world::World,
    etypes: &rewo_data::entity_types::EntityTypes,
    spawner_states: &std::collections::HashSet<u32>,
    lightmap: &LightmapState,
    alpha: f32,
) -> Vec<OwnedSpawnerMob> {
    let mut out = Vec::new();
    for (pos, be) in world.block_entities.iter() {
        if !spawner_states.contains(&world.block_state_at(pos.x, pos.y, pos.z)) {
            continue;
        }
        let Some(name) = spawner_entity_id(be) else {
            continue;
        };
        let Some(type_id) = etypes.id_of(&name) else {
            // An entity type this version does not register: draw nothing
            // rather than substitute a mob that is not in the cage.
            continue;
        };
        let (w, h) = etypes.dimensions(type_id);
        let spin = world.block_entities.spawner(*pos);
        out.push(OwnedSpawnerMob {
            pos: [pos.x as f32, pos.y as f32, pos.z as f32],
            kind: rewo_gpu::mobs::kind_for_entity_name(&name),
            width: w,
            height: h,
            mount: rewo_data::be_transform::spawner_mob(
                rewo_data::be_transform::spawner_spin_degrees(
                    spin.old_spin,
                    spin.spin,
                    alpha,
                ),
                rewo_data::be_transform::spawner_mob_scale(w, h),
            ),
            light: entity_light(
                world,
                pos.x as f64 + 0.5,
                pos.y as f64 + 0.5,
                pos.z as f64 + 0.5,
                lightmap,
            ),
        });
    }
    out.sort_by(|a, b| {
        a.pos[0]
            .total_cmp(&b.pos[0])
            .then(a.pos[1].total_cmp(&b.pos[1]))
            .then(a.pos[2].total_cmp(&b.pos[2]))
    });
    out
}


/// Turn a collected caged mob into an `EntityDraw`.
///
/// Everything except `pos`, `kind` and `mount` is the neutral pose: a spawner's
/// display entity is a *model*, not a simulated mob — vanilla loads it once and
/// never ticks it, so it does not walk, look around, swing or take damage.
pub(crate) fn spawner_mob_draw(m: &OwnedSpawnerMob) -> rewo_gpu::entities::EntityDraw<'_> {
    rewo_gpu::entities::EntityDraw {
        pos: m.pos,
        width: m.width,
        height: m.height,
        color: [1.0; 3],
        name: None,
        // M59: no health bar in a still — the gate renders its own.
        health: None,
        kind: m.kind,
        yaw: 0.0,
        death_time: 0.0,
        head_yaw: 0.0,
        pitch: 0.0,
        limb_swing: 0.0,
        limb_amount: 0.0,
        gesture: None,
        shell: false,
        events: [None; rewo_gpu::mobs::ModelEvent::COUNT],
        allay_dance: None,
        attack: rewo_gpu::mobs::SwingPose::NONE,
        arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
        mob: Default::default(),
        hurt: false,
        held: [None, None],
        ground_item: None,
        armor: [None; 4],
        held_glint: [false; 2],
        ground_glint: false,
        ground_count: 0,
        ground_seed: 0,
        ground_age: None,
        bob_offset: 0.0,
        skin_uv: None,
        scale_mul: 1.0,
        mount: Some(m.mount),
        anim_id: 0.0,
        light: m.light,
        emissive: rewo_gpu::entities::EmissiveState::default(),
        variant: 0,
        dye: None,
        sheared: false,
        undercoat: false,
        fish_dye: None,
        cape: None,
    }
}

/// The in-flight pickup animations, as entity draws (M81).
///
/// Same shape as the spawner's caged mob and for the same reason: this is an
/// *item*, and Rewo's item geometry lives on the entity pass, so the animation
/// reuses the whole `emit_ground_item` path — bob, spin, per-copy jitter and
/// all — rather than growing a second item emitter. Vanilla splits them (a
/// particle group with a captured render state) only because its entity is
/// already deleted; Rewo's problem is the same and its answer is
/// [`rewo_world::pickup`] holding the appearance rather than the entity.
///
/// Everything except position, item and age is neutral: a collected stack does
/// not walk, look around or take damage.
pub(crate) fn collect_pickups<'a>(
    session: &PlaySession,
    item_names: &'a rewo_data::items::Items,
    lightmap: &LightmapState,
    alpha: f32,
    now: f32,
) -> Vec<rewo_gpu::entities::EntityDraw<'a>> {
    let mut out = Vec::new();
    for p in session.world.pickups.iter() {
        let Some((item, count, foil)) = p.stack else {
            // An experience orb or an arrow: vanilla adds the particle
            // regardless and renders that entity's own model, which Rewo does
            // not have. The record exists; nothing is drawn.
            continue;
        };
        let Some(name) = item_names.name(item) else {
            continue;
        };
        let pos = p.render_pos(alpha);
        let pos = [pos[0] as f32, pos[1] as f32, pos[2] as f32];
        let light = entity_light(
            &session.world,
            pos[0] as f64,
            pos[1] as f64,
            pos[2] as f64,
            lightmap,
        );
        out.push(rewo_gpu::entities::EntityDraw {
            pos,
            width: 0.25,
            height: 0.25,
            color: [1.0; 3],
            name: None,
            health: None,
            kind: rewo_gpu::entities::EntityModelKind::Capsule,
            yaw: 0.0,
            death_time: 0.0,
            head_yaw: 0.0,
            pitch: 0.0,
            limb_swing: 0.0,
            limb_amount: 0.0,
            gesture: None,
            shell: false,
            events: [None; rewo_gpu::mobs::ModelEvent::COUNT],
            allay_dance: None,
            attack: rewo_gpu::mobs::SwingPose::NONE,
            arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
            mob: Default::default(),
            hurt: false,
            held: [None, None],
            ground_item: Some(name),
            armor: [None; 4],
            held_glint: [false; 2],
            ground_glint: foil,
            ground_count: count,
            ground_seed: item,
            // The captured `ageInTicks`, reconstructed from the animation's own
            // life counter: the flight is `life + alpha` ticks old, so capture
            // was that many ticks before now. No second clock, and it cannot
            // drift from the one `emit_ground_item` would otherwise use.
            ground_age: Some(now * 20.0 - (p.life as f32 + alpha)),
            bob_offset: bob_offset_for(p.entity_id),
            skin_uv: None,
            scale_mul: 1.0,
            mount: None,
            anim_id: 0.0,
            light,
            emissive: rewo_gpu::entities::EmissiveState::default(),
            variant: 0,
            dye: None,
            sheared: false,
            undercoat: false,
            fish_dye: None,
            cape: None,
        });
    }
    out
}

/// One spawner's caged mob.
pub(crate) struct OwnedSpawnerMob {
    pub pos: [f32; 3],
    pub kind: rewo_gpu::entities::EntityModelKind,
    pub width: f32,
    pub height: f32,
    pub mount: rewo_data::be_transform::Affine,
    pub light: [f32; 3],
}

/// A dye index as a linear-space tint.
///
/// `DyeColor.getTextureDiffuseColor()` is what dyes a banner layer — **not**
/// the `textColor` a sign uses. Two of the sixteen differ enough to be obvious
/// (red is 0xB02E26 here against 0xFF0000 there), so the two tables are kept
/// apart rather than shared.
pub(crate) fn dye_linear(i: usize) -> [f32; 3] {
    let c = rewo_data::block_entity_models::DYE_DIFFUSE_COLORS
        .get(i)
        .copied()
        .unwrap_or(0xFFFFFF);
    linear_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// A banner's pattern layers, as `(pattern id, dye index)`.
///
/// The tag is `patterns`, a list of `{pattern, color}` compounds written by
/// `BannerPatternLayers.CODEC`. `color` is a dye **name**, so it is resolved
/// through the same 16-entry order the block colours use.
pub(crate) fn banner_layers(
    be: &rewo_world::block_entities::BlockEntity,
) -> Vec<(String, usize)> {
    let Some(rewo_proto::nbt::Nbt::List(items)) = be.data.get("patterns") else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|it| {
            let pattern = it.get("pattern").and_then(rewo_proto::nbt::Nbt::as_str)?;
            let colour = it
                .get("color")
                .and_then(rewo_proto::nbt::Nbt::as_str)
                .and_then(|n| {
                    rewo_data::block_entity_models::DYE_COLORS
                        .iter()
                        .position(|d| *d == n)
                })
                .unwrap_or(0);
            Some((pattern.to_string(), colour))
        })
        .collect()
}

/// A decorated pot's four sherds, in `PotDecorations`' stored order —
/// **back, left, right, front**.
///
/// The tag is `sherds`, a list of item ids written by
/// `PotDecorations.CODEC`; an absent list, a short one, or a slot holding
/// `minecraft:brick` all mean the plain side, which is what
/// `getSideSprite` falls through to. Returning `None` for those rather than the
/// literal item keeps that fall-through in one place.
pub(crate) fn pot_sherds(be: &rewo_world::block_entities::BlockEntity) -> [Option<String>; 4] {
    let mut out: [Option<String>; 4] = Default::default();
    let Some(rewo_proto::nbt::Nbt::List(items)) = be.data.get("sherds") else {
        return out;
    };
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = items
            .get(i)
            .and_then(rewo_proto::nbt::Nbt::as_str)
            .map(str::to_string);
    }
    out
}

/// `Font.prepare8xTextOutline` — the eight offsets a glowing sign's outline is
/// drawn at, in font px.
///
/// `for (xo = -1; xo <= 1; xo++) for (yo = -1; yo <= 1; yo++) if (xo|yo != 0)`,
/// each scaled by the glyph's `getShadowOffset()`, which is 1 for the default
/// font. Eight copies, not four: the diagonals are what close the outline's
/// corners.
pub(super) const OUTLINE_OFFSETS: [(f32, f32); 8] = [
    (-1.0, -1.0),
    (-1.0, 0.0),
    (-1.0, 1.0),
    (0.0, -1.0),
    (0.0, 1.0),
    (1.0, -1.0),
    (1.0, 0.0),
    (1.0, 1.0),
];

/// How far behind the glyphs an outline copy sits, in font px.
///
/// Vanilla keeps them coplanar and separates them by draw order under
/// `Font.DisplayMode.POLYGON_OFFSET`. Rewo's world text rides the entity
/// pass's ordinary depth-tested buffer, so the separation is a real one. A
/// font px is 1/96 of a block, so this is ~1/10 mm in world terms — far below
/// the depth buffer's resolution at any distance a sign is legible from, and
/// far above the coplanar z-fighting it prevents.
pub(super) const OUTLINE_DEPTH: f32 = -0.01;

/// [`collect_sign_text`] against a live session — the app's ONLY entry.
///
/// **The world and the language table come from one owner, so no call site can
/// pair one with the other's absence.** Both of the app's frame loops used to
/// spell out `(&session.world, .., session.lang.as_deref())`, and an
/// adversarial review changed both of them to `None` with `blockentityshot`
/// still 179/179: the whole sign path could be wired to nothing and the only
/// gate that grades it never asks who calls it, because `sg1`/`sg2` hand
/// `collect_sign_text` a table of their own.
///
/// That is M89's finding — *a per-call-site choice is how two things from one
/// owner come to disagree* — reached for the fourth time in this tree, after
/// the container hover, the quick-move's slot kinds and the recipe book's
/// displacement. The remedy is the same one: one accessor, and the choice
/// deleted rather than duplicated. With no `lang` parameter here the mutation
/// is a compile error at both sites rather than a silent green.
///
/// [`collect_sign_text`] keeps taking the table, because that is the seam
/// `blockentityshot` needs and a session cannot be built in a gate (M71 — it
/// owns a socket). What is left ungraded is this one line, and `r49` grades it
/// end to end in the windowed client.
pub(crate) fn collect_session_sign_text(
    session: &rewo_net::play::PlaySession,
    signs: &rewo_data::sign_states::SignStates,
    lightmap: &LightmapState,
    advance: &[u8; 256],
) -> Vec<OwnedSignLine> {
    collect_sign_text(
        &session.world,
        signs,
        lightmap,
        advance,
        session.lang.as_deref(),
    )
}

/// Every sign face in the world, as text draws.
///
/// The board itself is an ordinary block model and has been drawn since M2;
/// this is only the text. A sign whose state is not in the table, or whose
/// block entity carries no `front_text`, contributes nothing.
///
/// The line's x is `-font.width(line) / 2` — `AbstractSignRenderer` centres
/// each line independently, which is why a short line sits centred rather than
/// left-aligned under a long one.
pub(crate) fn collect_sign_text(
    world: &rewo_world::World,
    signs: &rewo_data::sign_states::SignStates,
    lightmap: &LightmapState,
    advance: &[u8; 256],
    // The language table, threaded through to `SignFace::from_nbt`. Its doc
    // records what a per-frame resolution costs and why no cache is taken yet.
    lang: Option<&rewo_data::lang::Language>,
) -> Vec<OwnedSignLine> {
    use rewo_data::sign_text;
    let mut out = Vec::new();
    for (pos, be) in world.block_entities.iter() {
        let Some(sign) = signs.get(world.block_state_at(pos.x, pos.y, pos.z)) else {
            continue;
        };
        let (front, back) = be.sign_text(lang);
        let light = entity_light(
            world,
            pos.x as f64 + 0.5,
            pos.y as f64 + 0.5,
            pos.z as f64 + 0.5,
            lightmap,
        );
        for (face, is_front) in [(front, true), (back, false)] {
            let Some(face) = face else { continue };
            if face.is_blank() {
                continue;
            }
            // `submitSignText`'s colour branch (M27). Unglowing text is the
            // dye at 40%; glowing text is the dye at *full* strength, lit
            // fullbright, with the 40% version demoted to its outline — glow
            // is not "the same colour, brighter".
            let dye = sign_text::dye_text_color(face.color.as_deref());
            // `state.drawOutline` is `isOutlineVisible`: within 16 blocks of
            // the camera. Rewo has no camera here (the collector runs before
            // the view is known), so it takes the near branch — which only
            // ever *adds* an outline, and glowing black outlines regardless.
            let style = sign_text::text_style(dye, face.glowing, true);
            let rgb = |c: u32| linear_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
            let color = rgb(style.color);
            let outline = style.outline.map(rgb);
            let light = if style.fullbright {
                // `15728880` — both light nibbles at 15. Glowing ink is
                // legible in an unlit room, which is the point of it.
                sample(15, 15, lightmap)
            } else {
                light
            };
            let base = sign.text_transform(is_front);
            // The block origin is folded in here rather than in the renderer,
            // so a sign's transform is the same shape as a block entity's.
            let m = rewo_data::be_transform::mul(
                &rewo_data::be_transform::translation(
                    pos.x as f32,
                    pos.y as f32,
                    pos.z as f32,
                ),
                &base,
            );
            for (i, line) in face.lines.iter().enumerate() {
                // `getRenderMessages` splits every line against the board and
                // keeps fragment 0 — a sign does not wrap onto the next row,
                // it truncates at a word boundary (M27).
                let line = sign_text::split_first(line, sign.max_line_width, advance);
                if line.is_empty() {
                    continue;
                }
                let y = sign.line_y(i as i32);
                // Each line is centred on its *own* width, which is why a
                // short line sits centred under a long one.
                let x = -sign_text::width(&line, advance) / 2.0;
                if let Some(outline) = outline {
                    for (dx, dy) in OUTLINE_OFFSETS {
                        out.push(OwnedSignLine {
                            transform: m,
                            x: x + dx,
                            y: y + dy,
                            z: OUTLINE_DEPTH,
                            text: line.clone(),
                            color: outline,
                            light,
                        });
                    }
                }
                out.push(OwnedSignLine {
                    transform: m,
                    x,
                    y,
                    z: 0.0,
                    text: line,
                    color,
                    light,
                });
            }
        }
    }
    // Deterministic order, so a headless render is reproducible. The outline
    // copies share a line's `y`, so `x` and `z` join the key — without them
    // eight identical-looking entries would sort arbitrarily against each
    // other and the vertex buffer would differ run to run.
    out.sort_by(|a, b| {
        a.transform[0][3]
            .total_cmp(&b.transform[0][3])
            .then(a.transform[2][3].total_cmp(&b.transform[2][3]))
            .then(a.y.total_cmp(&b.y))
            .then(a.x.total_cmp(&b.x))
            .then(a.z.total_cmp(&b.z))
    });
    out
}

/// A [`rewo_gpu::entities::BlockEntityDraw`] that owns its model name.
///
/// The half-models' names are built per frame (`…_left` / `…_right`), so they
/// cannot borrow from the state table the way the single models did.
pub(crate) struct OwnedBlockEntityDraw {
    pub pos: [f32; 3],
    pub model: String,
    pub transform: rewo_data::be_transform::Affine,
    pub light: [f32; 3],
    pub part_transforms: [rewo_data::be_transform::Affine; rewo_gpu::entities::MAX_PARTS],
    pub part_pivots: [[f32; 3]; rewo_gpu::entities::MAX_PARTS],
    /// A linear tint multiplied into the vertex colour — `[1, 1, 1]` for
    /// everything but a banner's dyed pattern layers (M28c).
    pub tint: [f32; 3],
}

impl OwnedBlockEntityDraw {
    pub fn as_draw(&self) -> rewo_gpu::entities::BlockEntityDraw<'_> {
        rewo_gpu::entities::BlockEntityDraw {
            pos: self.pos,
            model: &self.model,
            transform: self.transform,
            light: self.light,
            part_transforms: self.part_transforms,
            part_pivots: self.part_pivots,
            tint: self.tint,
        }
    }
}
