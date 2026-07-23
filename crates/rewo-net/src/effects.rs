//! Client-side visual-effect tracking for the camera lightmap (M13).
//!
//! Only two mob effects change what the lightmap looks like: **night vision**
//! (lifts the ambient floor) and **darkness** (a pulsing subtractive dim). This
//! module tracks just those, for just the local player, to the exact fidelity
//! the lightmap extractor reads — nothing more. It is the client half of the
//! effect lifecycle, ported from the decompile:
//!
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java`
//!   `handleUpdateMobEffect` / `handleRemoveMobEffect` — build a
//!   `MobEffectInstance`, `skipBlending()` when the blend flag is clear, then
//!   `forceAddEffect` (a wholesale `put` that `copyBlendState`s the previous
//!   instance). Removal is `removeEffectNoUpdate` — an immediate map remove.
//! - `net/minecraft/world/effect/MobEffectInstance.java` — `tickClient`,
//!   `endsWithin`, and the inner `BlendState` (`tick` / `setImmediate` /
//!   `getFactor`).
//! - `net/minecraft/world/effect/MobEffects.java` — darkness
//!   `setBlendDuration(22)` (in/out/out-advance all 22); night vision sets no
//!   blend duration, so all three are 0.
//! - `net/minecraft/world/entity/LivingEntity.java` `tickEffects` (the client
//!   branch just calls `tickClient` on each active effect) +
//!   `getEffectBlendFactor`.
//!
//! **Deliberate minimal cuts** (documented + tested):
//! - No hidden-effect stack. The client update path (`forceAddEffect`) never
//!   calls `MobEffectInstance.update()`, so the amplifier/duration takeover +
//!   `hiddenEffect` layering is unreachable here. An update packet replaces the
//!   instance wholesale and copies the previous blend state — exactly what the
//!   client does.
//! - `BlendState.getFactor`'s `isRemoved` short-circuit is dropped: the local
//!   player is never removed mid-session, so it can't fire.
//! - Only night vision + darkness are modelled; every other effect id is
//!   ignored (they don't touch the camera lightmap).

use rewo_proto::reader::PacketReader;

/// Per-effect blend timing (`MobEffect.setBlendDuration`). Night vision leaves
/// these at 0; darkness sets all three to 22.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BlendSpec {
    /// `getBlendInDurationTicks` — ramp-up ticks when the effect is active.
    in_ticks: i32,
    /// `getBlendOutDurationTicks` — ramp-down ticks when it fades.
    out_ticks: i32,
    /// `getBlendOutAdvanceTicks` — how many ticks before expiry the fade-out
    /// begins (`hasEffect = !endsWithin(out_advance)`).
    out_advance: i32,
}

/// Night vision: no `setBlendDuration` call → all defaults 0 (instant blend).
const NIGHT_VISION_BLEND: BlendSpec = BlendSpec {
    in_ticks: 0,
    out_ticks: 0,
    out_advance: 0,
};
/// Darkness: `setBlendDuration(22)` → 22/22/22.
const DARKNESS_BLEND: BlendSpec = BlendSpec {
    in_ticks: 22,
    out_ticks: 22,
    out_advance: 22,
};

/// `MobEffectInstance.BlendState` — the two-frame factor the lightmap lerps
/// between for a smooth darkness fade.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BlendState {
    factor: f32,
    factor_previous_frame: f32,
}

impl BlendState {
    const fn zero() -> Self {
        Self {
            factor: 0.0,
            factor_previous_frame: 0.0,
        }
    }
}

/// The minimal `MobEffectInstance` needed by the lightmap: a duration, its
/// blend timing, and the running `BlendState`. The wire amplifier is parsed
/// (`UpdateEffect.amplifier`, for byte-exact packet layout) but not stored —
/// nothing the camera lightmap reads depends on the effect's amplifier.
#[derive(Clone, Copy, Debug, PartialEq)]
struct EffectInstance {
    /// `-1` is infinite; otherwise a tick countdown.
    duration: i32,
    spec: BlendSpec,
    blend: BlendState,
}

impl EffectInstance {
    fn new(duration: i32, spec: BlendSpec) -> Self {
        Self {
            duration,
            spec,
            blend: BlendState::zero(),
        }
    }

    /// `isInfiniteDuration` — the `-1` sentinel.
    fn is_infinite(&self) -> bool {
        self.duration == -1
    }

    /// `endsWithin(ticks) = !isInfiniteDuration && duration <= ticks`.
    fn ends_within(&self, ticks: i32) -> bool {
        !self.is_infinite() && self.duration <= ticks
    }

    /// `hasRemainingDuration = isInfiniteDuration || duration > 0`.
    fn has_remaining(&self) -> bool {
        self.is_infinite() || self.duration > 0
    }

    /// `BlendState.hasEffect(instance) = !endsWithin(getBlendOutAdvanceTicks)`.
    fn blend_has_effect(&self) -> bool {
        !self.ends_within(self.spec.out_advance)
    }

    /// `tickDownDuration` via `mapDuration(d -> d - 1)`: infinite and `0` stay
    /// put, otherwise decrement.
    fn tick_down_duration(&mut self) {
        if !self.is_infinite() && self.duration != 0 {
            self.duration -= 1;
        }
    }

    /// `tickClient`: decrement (when it has remaining duration), then tick the
    /// blend. (No `downgradeToHiddenEffect` — we carry no hidden stack.)
    fn tick_client(&mut self) {
        if self.has_remaining() {
            self.tick_down_duration();
        }
        self.blend_tick();
    }

    /// `BlendState.tick`: advance `factor` toward `hasEffect ? 1 : 0` by at most
    /// `1/blendDuration` per tick (or snap when `blendDuration == 0`).
    fn blend_tick(&mut self) {
        self.blend.factor_previous_frame = self.blend.factor;
        let has_effect = self.blend_has_effect();
        let target = if has_effect { 1.0 } else { 0.0 };
        if self.blend.factor != target {
            let blend_duration = if has_effect {
                self.spec.in_ticks
            } else {
                self.spec.out_ticks
            };
            if blend_duration == 0 {
                self.blend.factor = target;
            } else {
                let max_delta = 1.0 / blend_duration as f32;
                let delta = (target - self.blend.factor).clamp(-max_delta, max_delta);
                self.blend.factor += delta;
            }
        }
    }

    /// `skipBlending` → `BlendState.setImmediate`: snap both frames to the
    /// current `hasEffect` target.
    fn skip_blending(&mut self) {
        let f = if self.blend_has_effect() { 1.0 } else { 0.0 };
        self.blend.factor = f;
        self.blend.factor_previous_frame = f;
    }

    /// `copyBlendState` — carry the previous instance's blend across a
    /// wholesale `forceAddEffect` replacement.
    fn copy_blend_state(&mut self, other: &EffectInstance) {
        self.blend = other.blend;
    }

    /// `getBlendFactor / getFactor` = `Mth.lerp(partial, prev, factor)`.
    /// `Mth.lerp(delta, start, end)` is `start + delta * (end - start)` with
    /// **no clamp on `delta`**, so a `partial` outside `[0, 1]` extrapolates —
    /// reproduced verbatim here, not clamped. (The `isRemoved` short-circuit is
    /// dropped — see the module cut.)
    fn blend_factor(&self, partial: f32) -> f32 {
        self.blend.factor_previous_frame
            + (self.blend.factor - self.blend.factor_previous_frame) * partial
    }
}

/// A decoded clientbound `update_mob_effect` (protocol 776). See
/// `ClientboundUpdateMobEffectPacket`: VarInt entity, raw holder-registry id,
/// VarInt amplifier, VarInt duration, then a flags byte (bit 8 = blend).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpdateEffect {
    pub entity_id: i32,
    pub effect_id: i32,
    pub amplifier: i32,
    pub duration: i32,
    pub flags: u8,
}

impl UpdateEffect {
    /// `FLAG_BLEND = 8` — whether the client should blend rather than snap.
    pub fn should_blend(&self) -> bool {
        self.flags & 8 != 0
    }
}

/// A decoded clientbound `remove_mob_effect`: VarInt entity + raw holder id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoveEffect {
    pub entity_id: i32,
    pub effect_id: i32,
}

/// Parse an `update_mob_effect` body. Returns `None` on truncation/garbage; the
/// caller's stream state is untouched because each packet is its own buffer.
pub fn parse_update(body: &[u8]) -> Option<UpdateEffect> {
    let mut r = PacketReader::new(body);
    Some(UpdateEffect {
        entity_id: r.varint().ok()?,
        effect_id: r.varint().ok()?,
        amplifier: r.varint().ok()?,
        duration: r.varint().ok()?,
        flags: r.u8().ok()?,
    })
}

/// Parse a `remove_mob_effect` body. `None` on truncation/garbage.
pub fn parse_remove(body: &[u8]) -> Option<RemoveEffect> {
    let mut r = PacketReader::new(body);
    Some(RemoveEffect {
        entity_id: r.varint().ok()?,
        effect_id: r.varint().ok()?,
    })
}

/// A read-only snapshot of the two lightmap-relevant effects at a partial tick,
/// for the app/renderer to combine with `rewo_world::lightmap`. Carries the raw
/// *inputs* (night-vision duration, darkness blend factor, player tick count);
/// the app applies `lightmap::night_vision_intensity` /
/// `lightmap::darkness_lightmap`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisualEffectSnapshot {
    /// Current night-vision duration (`-1` infinite), or `None` when absent.
    /// This is the input to `lightmap::night_vision_intensity`.
    pub night_vision_duration: Option<i32>,
    /// `getEffectBlendFactor(DARKNESS, partial)` — `0.0` when absent. The input
    /// to `lightmap::darkness_lightmap`.
    pub darkness_blend_factor: f32,
    /// The player entity's tick count (`Entity.tickCount`, incremented by
    /// `ClientLevel.tickNonPassenger`) — the darkness pulse's phase.
    pub tick_count: i32,
}

/// Tracks night vision + darkness for the local player, driven by the two
/// effect packets and one client tick per 20 Hz step.
#[derive(Clone, Debug)]
pub struct VisualEffects {
    night_vision: Option<EffectInstance>,
    darkness: Option<EffectInstance>,
    /// Raw `minecraft:mob_effect` registry ids, captured from `registry_data`
    /// (never assumed from bootstrap order). `None` until config syncs them.
    night_vision_id: Option<i32>,
    darkness_id: Option<i32>,
    /// The local player's entity id (first `int` of the login packet). Only
    /// effects targeting it drive the camera lightmap.
    player_id: Option<i32>,
    /// `Entity.tickCount` for the player (incremented by
    /// `ClientLevel.tickNonPassenger`) — the darkness pulse's phase.
    tick_count: i32,
}

impl VisualEffects {
    /// Construct with the `mob_effect` registry ids resolved during
    /// configuration (either may be `None` on a server that never syncs them).
    pub fn new(night_vision_id: Option<i32>, darkness_id: Option<i32>) -> Self {
        Self {
            night_vision: None,
            darkness: None,
            night_vision_id,
            darkness_id,
            player_id: None,
            tick_count: 0,
        }
    }

    /// Record the local player's entity id (from the login packet). Once set,
    /// [`tick`](Self::tick) begins advancing and effect packets targeting this
    /// id are tracked. A later call **replaces** the id outright (a re-login /
    /// respawn hands a fresh entity id); it does NOT clear `tick_count` or the
    /// tracked effects. In practice the login packet arrives once before any
    /// effect packet, so this is only ever the initial set — but the replace
    /// semantics are the safe default and never drop a legitimately-tracked
    /// effect on the ground.
    pub fn set_player_id(&mut self, id: i32) {
        self.player_id = Some(id);
    }

    /// Which tracked slot (if any) a raw registry id maps to.
    fn slot_of(&self, effect_id: i32) -> Option<Slot> {
        if self.night_vision_id == Some(effect_id) {
            Some(Slot::NightVision)
        } else if self.darkness_id == Some(effect_id) {
            Some(Slot::Darkness)
        } else {
            None
        }
    }

    /// Parse + apply an `update_mob_effect` body. Ignores other entities, other
    /// effects, and malformed packets; never panics.
    pub fn apply_update(&mut self, body: &[u8]) {
        if let Some(u) = parse_update(body) {
            self.on_update(u);
        }
    }

    /// Parse + apply a `remove_mob_effect` body.
    pub fn apply_remove(&mut self, body: &[u8]) {
        if let Some(rem) = parse_remove(body) {
            self.on_remove(rem);
        }
    }

    /// `handleUpdateMobEffect` for the local player: build the instance,
    /// `skipBlending` when the blend flag is clear, then `forceAddEffect` —
    /// replace wholesale, carrying the previous blend state.
    pub fn on_update(&mut self, u: UpdateEffect) {
        if self.player_id != Some(u.entity_id) {
            return;
        }
        let Some(slot) = self.slot_of(u.effect_id) else {
            return;
        };
        let spec = match slot {
            Slot::NightVision => NIGHT_VISION_BLEND,
            Slot::Darkness => DARKNESS_BLEND,
        };
        let mut instance = EffectInstance::new(u.duration, spec);
        if !u.should_blend() {
            instance.skip_blending();
        }
        let previous = self.slot_mut(slot).take();
        if let Some(prev) = previous {
            instance.copy_blend_state(&prev);
        }
        *self.slot_mut(slot) = Some(instance);
    }

    /// `handleRemoveMobEffect` → `removeEffectNoUpdate`: drop the instance
    /// immediately.
    pub fn on_remove(&mut self, rem: RemoveEffect) {
        if self.player_id != Some(rem.entity_id) {
            return;
        }
        if let Some(slot) = self.slot_of(rem.effect_id) {
            *self.slot_mut(slot) = None;
        }
    }

    fn slot_mut(&mut self, slot: Slot) -> &mut Option<EffectInstance> {
        match slot {
            Slot::NightVision => &mut self.night_vision,
            Slot::Darkness => &mut self.darkness,
        }
    }

    /// One client tick: bump the player tick count, then `tickClient` each
    /// active effect. Vanilla runs these in `ClientLevel.tickNonPassenger`,
    /// which increments `entity.tickCount` *before* calling `entity.tick()`;
    /// the tick then reaches the effects later, via `LivingEntity.baseTick`
    /// → `tickEffects` (client branch just calls `tickClient`). We keep that
    /// order (count first, then effects) so the darkness pulse reads the
    /// already-incremented count exactly as the client does. Effects whose
    /// duration runs out simply stop being renewed and blend out; an expired
    /// instance persists until a remove packet, matching the client (the
    /// server sends the remove).
    ///
    /// **Returns early until the local player exists.** Vanilla has no local
    /// entity to `ClientLevel.tickNonPassenger` before login creates it — the
    /// player is what `tickNonPassenger` increments and whose effects
    /// `tickClient` runs. Until `set_player_id`, `tick_count` must not advance
    /// and no effect can exist (`on_update`/`on_remove` require a matching
    /// `player_id`), so there is nothing to tick and nothing to reset.
    pub fn tick(&mut self) {
        if self.player_id.is_none() {
            return;
        }
        self.tick_count = self.tick_count.wrapping_add(1);
        if let Some(nv) = &mut self.night_vision {
            nv.tick_client();
        }
        if let Some(d) = &mut self.darkness {
            d.tick_client();
        }
    }

    /// Night-vision duration input (`-1` infinite), or `None` when absent.
    pub fn night_vision_duration(&self) -> Option<i32> {
        self.night_vision.map(|e| e.duration)
    }

    /// `getEffectBlendFactor(DARKNESS, partial)` — `0.0` when darkness is
    /// absent.
    pub fn darkness_blend_factor(&self, partial: f32) -> f32 {
        self.darkness.map_or(0.0, |e| e.blend_factor(partial))
    }

    /// The player's tick count — the darkness pulse's phase.
    pub fn tick_count(&self) -> i32 {
        self.tick_count
    }

    /// A combined read-only snapshot at `partial`.
    pub fn snapshot(&self, partial: f32) -> VisualEffectSnapshot {
        VisualEffectSnapshot {
            night_vision_duration: self.night_vision_duration(),
            darkness_blend_factor: self.darkness_blend_factor(partial),
            tick_count: self.tick_count,
        }
    }
}

#[derive(Clone, Copy)]
enum Slot {
    NightVision,
    Darkness,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_proto::varint::write_varint;

    // Synthetic registry ids — the point of capturing them at runtime is to
    // NOT hard-code bootstrap ordinals, so the tests use arbitrary ids and the
    // tracker matches by value.
    const NV_ID: i32 = 5;
    const DARK_ID: i32 = 40;
    const PLAYER: i32 = 777;
    const OTHER_ENTITY: i32 = 778;

    fn tracker() -> VisualEffects {
        let mut t = VisualEffects::new(Some(NV_ID), Some(DARK_ID));
        t.set_player_id(PLAYER);
        t
    }

    fn update_body(entity: i32, effect: i32, amp: i32, dur: i32, flags: u8) -> Vec<u8> {
        let mut b = Vec::new();
        write_varint(&mut b, entity);
        write_varint(&mut b, effect);
        write_varint(&mut b, amp);
        write_varint(&mut b, dur);
        b.push(flags);
        b
    }

    fn remove_body(entity: i32, effect: i32) -> Vec<u8> {
        let mut b = Vec::new();
        write_varint(&mut b, entity);
        write_varint(&mut b, effect);
        b
    }

    #[test]
    fn parses_update_layout_including_multibyte_varints() {
        // Duration 300 needs a 2-byte varint; blend flag set.
        let b = update_body(PLAYER, NV_ID, 0, 300, 8);
        let u = parse_update(&b).expect("parse");
        assert_eq!(
            u,
            UpdateEffect {
                entity_id: PLAYER,
                effect_id: NV_ID,
                amplifier: 0,
                duration: 300,
                flags: 8,
            }
        );
        assert!(u.should_blend());
    }

    #[test]
    fn parses_remove_layout() {
        let b = remove_body(PLAYER, DARK_ID);
        assert_eq!(
            parse_remove(&b),
            Some(RemoveEffect {
                entity_id: PLAYER,
                effect_id: DARK_ID,
            })
        );
    }

    #[test]
    fn malformed_packets_return_none_and_do_not_apply() {
        // Truncated: entity present, rest missing.
        let mut b = Vec::new();
        write_varint(&mut b, PLAYER);
        assert_eq!(parse_update(&b), None);
        assert_eq!(parse_remove(&[]), None);

        // apply_* on garbage leaves state untouched.
        let mut t = tracker();
        t.apply_update(&b);
        t.apply_update(&[0xFF, 0xFF]); // never-terminating varint
        assert_eq!(t.night_vision_duration(), None);
        assert_eq!(t.darkness_blend_factor(0.0), 0.0);
    }

    #[test]
    fn only_the_local_player_drives_effects() {
        let mut t = tracker();
        // Same effect on another entity is ignored.
        t.apply_update(&update_body(OTHER_ENTITY, NV_ID, 0, 400, 8));
        assert_eq!(t.night_vision_duration(), None);
        // On the player, it lands.
        t.apply_update(&update_body(PLAYER, NV_ID, 0, 400, 8));
        assert_eq!(t.night_vision_duration(), Some(400));
    }

    #[test]
    fn unknown_effect_ids_are_ignored() {
        let mut t = tracker();
        t.apply_update(&update_body(PLAYER, 999, 0, 400, 8));
        assert_eq!(t.night_vision_duration(), None);
        assert_eq!(t.darkness_blend_factor(0.0), 0.0);
    }

    #[test]
    fn missing_registry_ids_match_nothing() {
        // A server that synced no mob_effect registry → no id matches.
        let mut t = VisualEffects::new(None, None);
        t.set_player_id(PLAYER);
        t.apply_update(&update_body(PLAYER, NV_ID, 0, 400, 8));
        assert_eq!(t.night_vision_duration(), None);
    }

    #[test]
    fn infinite_duration_never_decrements() {
        let mut t = tracker();
        t.on_update(UpdateEffect {
            entity_id: PLAYER,
            effect_id: NV_ID,
            amplifier: 0,
            duration: -1,
            flags: 8,
        });
        for _ in 0..100 {
            t.tick();
        }
        assert_eq!(t.night_vision_duration(), Some(-1));
    }

    #[test]
    fn finite_duration_decrements_each_tick() {
        let mut t = tracker();
        t.on_update(UpdateEffect {
            entity_id: PLAYER,
            effect_id: NV_ID,
            amplifier: 0,
            duration: 10,
            flags: 8,
        });
        t.tick();
        assert_eq!(t.night_vision_duration(), Some(9));
        for _ in 0..9 {
            t.tick();
        }
        // Reaches 0 and stays (tick_down leaves 0 in place; the server sends
        // the remove packet to clear it).
        assert_eq!(t.night_vision_duration(), Some(0));
        t.tick();
        assert_eq!(t.night_vision_duration(), Some(0));
    }

    #[test]
    fn night_vision_200_boundary_inputs_reach_the_snapshot() {
        // The tracker exposes the raw duration; the 200-tick boundary lives in
        // `lightmap::night_vision_scale`. Pin the duration crossing 200.
        let mut t = tracker();
        t.on_update(UpdateEffect {
            entity_id: PLAYER,
            effect_id: NV_ID,
            amplifier: 0,
            duration: 202,
            flags: 8,
        });
        t.tick(); // 201
        assert_eq!(t.night_vision_duration(), Some(201));
        t.tick(); // 200
        assert_eq!(t.snapshot(0.0).night_vision_duration, Some(200));
    }

    #[test]
    fn darkness_blends_in_over_22_ticks() {
        let mut t = tracker();
        // Long darkness, blend flag set → starts from factor 0.
        t.on_update(UpdateEffect {
            entity_id: PLAYER,
            effect_id: DARK_ID,
            amplifier: 0,
            duration: 400,
            flags: 8,
        });
        assert_eq!(t.darkness_blend_factor(0.0), 0.0);
        // `blend_factor` lerps from the previous frame, so the just-updated
        // factor is read at partial 1.0. Each tick advances by 1/22 toward 1.0.
        t.tick();
        assert!((t.darkness_blend_factor(1.0) - 1.0 / 22.0).abs() < 1e-6);
        for _ in 0..21 {
            t.tick();
        }
        // After 22 ticks it has reached 1.0 (the clamp snaps the last step).
        assert!((t.darkness_blend_factor(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn darkness_blends_out_over_22_ticks_before_expiry() {
        let mut t = tracker();
        // Duration 22 with blend: hasEffect = !endsWithin(22) = false at 22, so
        // it is already fading out (out-advance = 22).
        t.on_update(UpdateEffect {
            entity_id: PLAYER,
            effect_id: DARK_ID,
            amplifier: 0,
            duration: 22,
            flags: 8,
        });
        // First tick: duration → 21, hasEffect = !endsWithin(22) = false (21 <=
        // 22), target 0, factor stays 0. It never blends in — this is the
        // fade-out advance window in action.
        t.tick();
        assert_eq!(t.darkness_blend_factor(0.0), 0.0);
    }

    #[test]
    fn non_blended_update_snaps_immediately() {
        let mut t = tracker();
        // Blend flag CLEAR → skipBlending → factor is the immediate target.
        t.on_update(UpdateEffect {
            entity_id: PLAYER,
            effect_id: DARK_ID,
            amplifier: 0,
            duration: 400,
            flags: 0,
        });
        // hasEffect = true (long duration) → immediate factor 1.0, no ramp.
        assert!((t.darkness_blend_factor(0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn partial_tick_lerps_between_frames() {
        let mut t = tracker();
        t.on_update(UpdateEffect {
            entity_id: PLAYER,
            effect_id: DARK_ID,
            amplifier: 0,
            duration: 400,
            flags: 8,
        });
        t.tick(); // prev 0.0, factor 1/22
        let full = 1.0 / 22.0;
        assert!((t.darkness_blend_factor(0.0) - 0.0).abs() < 1e-6);
        assert!((t.darkness_blend_factor(1.0) - full).abs() < 1e-6);
        assert!((t.darkness_blend_factor(0.5) - full * 0.5).abs() < 1e-6);
    }

    #[test]
    fn update_replacement_copies_blend_state() {
        let mut t = tracker();
        // Blend darkness partway in.
        t.on_update(UpdateEffect {
            entity_id: PLAYER,
            effect_id: DARK_ID,
            amplifier: 0,
            duration: 400,
            flags: 8,
        });
        for _ in 0..5 {
            t.tick();
        }
        let mid = t.darkness_blend_factor(0.0);
        assert!(mid > 0.0 && mid < 1.0);
        // A resent packet (server renews duration) must NOT reset the blend.
        t.on_update(UpdateEffect {
            entity_id: PLAYER,
            effect_id: DARK_ID,
            amplifier: 0,
            duration: 600,
            flags: 8,
        });
        assert!((t.darkness_blend_factor(0.0) - mid).abs() < 1e-6);
    }

    #[test]
    fn blend_factor_extrapolates_outside_unit_partial() {
        // `Mth.lerp` does not clamp its delta, so a partial outside [0, 1]
        // extrapolates. After one tick: prev 0.0, factor 1/22, so
        // blend_factor(p) = p * (1/22).
        let mut t = tracker();
        t.on_update(UpdateEffect {
            entity_id: PLAYER,
            effect_id: DARK_ID,
            amplifier: 0,
            duration: 400,
            flags: 8,
        });
        t.tick();
        let step = 1.0 / 22.0;
        // partial > 1 overshoots the current frame's factor.
        assert!((t.darkness_blend_factor(2.0) - 2.0 * step).abs() < 1e-6);
        // partial < 0 undershoots below the previous frame (goes negative).
        assert!((t.darkness_blend_factor(-1.0) + step).abs() < 1e-6);
        assert!(t.darkness_blend_factor(-1.0) < 0.0);
    }

    #[test]
    fn non_blended_replacement_still_retains_prior_blend_state() {
        // The surprising `ClientPacketListener.handleUpdateMobEffect` →
        // `forceAddEffect` ordering: the new instance is `skipBlending()`'d
        // FIRST (flags clear → snap to target), then `forceAddEffect` copies
        // the PREVIOUS instance's blend state over it. So a non-blended
        // replacement of an already-partially-blended effect does NOT snap —
        // the prior partial blend wins. Pin that exact behaviour.
        let mut t = tracker();
        // Blend darkness partway in (5 ticks → factor ~5/22).
        t.on_update(UpdateEffect {
            entity_id: PLAYER,
            effect_id: DARK_ID,
            amplifier: 0,
            duration: 400,
            flags: 8,
        });
        for _ in 0..5 {
            t.tick();
        }
        let prev0 = t.darkness_blend_factor(0.0);
        let prev1 = t.darkness_blend_factor(1.0);
        assert!(prev1 > 0.0 && prev1 < 1.0, "must be mid-blend, got {prev1}");
        // A non-blended (flags = 0) update. A naive skipBlending would snap the
        // factor to the immediate target 1.0; the copyBlendState afterwards
        // overwrites that snap with the previous partial state.
        t.on_update(UpdateEffect {
            entity_id: PLAYER,
            effect_id: DARK_ID,
            amplifier: 0,
            duration: 600,
            flags: 0,
        });
        // The blend state is unchanged — skipBlending was overwritten.
        assert!((t.darkness_blend_factor(0.0) - prev0).abs() < 1e-6);
        assert!((t.darkness_blend_factor(1.0) - prev1).abs() < 1e-6);
        // Explicitly: it did NOT snap to the immediate target.
        assert!(
            (t.darkness_blend_factor(1.0) - 1.0).abs() > 1e-3,
            "must not have snapped to 1.0"
        );
    }

    #[test]
    fn removal_is_immediate() {
        let mut t = tracker();
        t.on_update(UpdateEffect {
            entity_id: PLAYER,
            effect_id: NV_ID,
            amplifier: 0,
            duration: 400,
            flags: 8,
        });
        assert_eq!(t.night_vision_duration(), Some(400));
        t.apply_remove(&remove_body(PLAYER, NV_ID));
        assert_eq!(t.night_vision_duration(), None);
        // Removing another entity's effect does nothing.
        t.on_update(UpdateEffect {
            entity_id: PLAYER,
            effect_id: DARK_ID,
            amplifier: 0,
            duration: 400,
            flags: 8,
        });
        t.apply_remove(&remove_body(OTHER_ENTITY, DARK_ID));
        assert!(t.darkness_blend_factor(0.0) >= 0.0);
        assert!(t.snapshot(0.0).darkness_blend_factor >= 0.0);
    }

    #[test]
    fn tick_count_advances_and_feeds_the_snapshot() {
        let mut t = tracker();
        for _ in 0..7 {
            t.tick();
        }
        assert_eq!(t.tick_count(), 7);
        assert_eq!(t.snapshot(0.0).tick_count, 7);
    }

    #[test]
    fn tick_is_a_noop_until_the_player_exists() {
        // Vanilla has no local entity to tick before login creates it, so
        // ticks before `set_player_id` must leave the count at 0. (Note the
        // `tracker()` helper already sets the id; here we build one without.)
        let mut t = VisualEffects::new(Some(NV_ID), Some(DARK_ID));
        for _ in 0..5 {
            t.tick();
        }
        assert_eq!(t.tick_count(), 0, "tick_count must not advance pre-login");
        // Once the player id lands, ticking resumes from 0 → 1.
        t.set_player_id(PLAYER);
        t.tick();
        assert_eq!(t.tick_count(), 1);
    }
}
