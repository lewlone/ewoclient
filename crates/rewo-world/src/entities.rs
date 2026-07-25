//! Entity table — id → transform, with vanilla-style interpolation and a
//! UUID → player-name map (Player Info) for nametags.
//!
//! Movement model (decompiled `Entity.lerpTo` semantics): packets set an
//! authoritative **target** — absolute from add/teleport/position-sync,
//! delta-accumulated from the move packets (the client-side mirror of
//! `VecDeltaCodec`: deltas apply to the last transmitted position, so
//! quantization can't drift). Each 20 Hz tick the rendered position steps
//! `(target - cur) / steps_left` toward it (3-step lerp, converging exactly
//! on the third tick); frames blend `prev → cur` by the partial-tick alpha.

use std::collections::HashMap;

/// Vanilla's interpolation step count for tracked entities.
const LERP_STEPS: u32 = 3;

/// A model-visible entity-event animation (`ClientboundEntityEventPacket`).
///
/// Vanilla routes each event byte through the concrete entity class'
/// `handleEntityEvent`, where an id like 4 means "attack" on a warden and
/// something entirely different (or nothing) on any other entity — so the
/// *(kind, id)* pair, not the id alone, names the animation. Only the three
/// model-visible ones are represented here; every other event byte is an
/// unmodelled status (hurt flash, particles, sound) that this client ignores.
///
/// Each is a one-shot `AnimationState`: the wire carries no time, so the
/// client stamps the receipt tick and the renderer measures elapsed from it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntityEvent {
    /// Warden id 4 — `attackAnimationState.start(tickCount)` (also stops the
    /// roar; the render layer drops the roar while this plays).
    WardenAttack,
    /// Warden id 62 — `sonicBoomAnimationState.start(tickCount)`.
    WardenSonicBoom,
    /// Armadillo id 64 — restarts the peek from tick 0 even while the
    /// metadata-driven SCARED/balled state (which normally holds the peek at
    /// its end pose) persists.
    ArmadilloPeek,
}

impl EntityEvent {
    /// Dense count for the fixed-size per-entity store.
    pub const COUNT: usize = 3;

    /// Slot index into [`EntityEventStarts`].
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// Per-entity receipt ticks of the active model-visible events. `None` = the
/// event has never fired for this entity (so its rig contributes nothing).
type EntityEventStarts = [Option<i64>; EntityEvent::COUNT];

#[derive(Clone, Copy, Debug)]
pub struct EntityState {
    pub uuid: u128,
    pub type_id: i32,
    /// Authoritative synced target position (see module docs).
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    /// Head yaw (degrees) — the server points it at nearby players, so the
    /// model can turn its head toward the viewer. Defaults to the body yaw.
    pub head_yaw: f32,
    lerp_steps: u32,
    cur: [f64; 3],
    prev: [f64; 3],
    /// Walk-cycle phase (vanilla `animationPosition`) — advances by
    /// `limb_amount` each tick, so a still entity's limbs freeze.
    limb_swing: f32,
    /// Smoothed horizontal speed 0..1 (vanilla `animationSpeed`) — the
    /// swing amplitude. The server never sends limb angles; both are
    /// derived here from the entity's own motion, exactly as vanilla does.
    limb_amount: f32,
}

impl EntityState {
    pub fn new(uuid: u128, type_id: i32, x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Self {
        Self {
            uuid,
            type_id,
            x,
            y,
            z,
            yaw,
            pitch,
            head_yaw: yaw,
            lerp_steps: 0,
            cur: [x, y, z],
            prev: [x, y, z],
            limb_swing: 0.0,
            limb_amount: 0.0,
        }
    }

    pub fn set_head_yaw(&mut self, yaw: f32) {
        self.head_yaw = yaw;
    }

    /// Absolute target (teleport / position sync): start a fresh 3-tick lerp.
    pub fn set_target(&mut self, x: f64, y: f64, z: f64) {
        self.x = x;
        self.y = y;
        self.z = z;
        self.lerp_steps = LERP_STEPS;
    }

    /// Relative move (the short-delta packets): accumulate onto the synced
    /// target, never onto the rendered position.
    pub fn nudge(&mut self, dx: f64, dy: f64, dz: f64) {
        self.set_target(self.x + dx, self.y + dy, self.z + dz);
    }

    pub fn set_rot(&mut self, yaw: f32, pitch: f32) {
        self.yaw = yaw;
        self.pitch = pitch;
    }

    fn tick(&mut self) {
        let before = self.cur;
        self.prev = self.cur;
        if self.lerp_steps > 0 {
            let n = self.lerp_steps as f64;
            self.cur[0] += (self.x - self.cur[0]) / n;
            self.cur[1] += (self.y - self.cur[1]) / n;
            self.cur[2] += (self.z - self.cur[2]) / n;
            self.lerp_steps -= 1;
        }
        // Walk animation from this tick's horizontal displacement (vanilla
        // LivingEntity.aiStep): target = min(1, dist·4), smoothed by 0.4,
        // phase advances by the smoothed amount.
        let dx = self.cur[0] - before[0];
        let dz = self.cur[2] - before[2];
        let target = ((dx * dx + dz * dz).sqrt() as f32 * 4.0).min(1.0);
        self.limb_amount += (target - self.limb_amount) * 0.4;
        self.limb_swing += self.limb_amount;
    }

    /// Frame position: last tick's `prev` blended toward `cur` by the
    /// partial-tick alpha (0..1).
    pub fn render_pos(&self, alpha: f32) -> [f64; 3] {
        let a = alpha.clamp(0.0, 1.0) as f64;
        [
            self.prev[0] + (self.cur[0] - self.prev[0]) * a,
            self.prev[1] + (self.cur[1] - self.prev[1]) * a,
            self.prev[2] + (self.cur[2] - self.prev[2]) * a,
        ]
    }

    /// Walk-cycle phase + amplitude for the model's limb swing.
    pub fn limb(&self) -> (f32, f32) {
        (self.limb_swing, self.limb_amount)
    }
}

#[derive(Default)]
pub struct EntityTable {
    map: HashMap<i32, EntityState>,
    /// Player Info: profile UUID → name. Populated before the player's
    /// `add_entity` arrives; survives entity unload (list membership, not
    /// entity lifetime).
    names: HashMap<u128, String>,
    /// Custom names from entity metadata, keyed by entity id (a named mob
    /// shows this above its model). Cleared on entity removal.
    custom_names: HashMap<i32, String>,
    /// Entity pose ordinals from metadata index 6 (warden roar, frog
    /// croak, breeze shoot…). Cleared on entity removal.
    poses: HashMap<i32, u8>,
    /// Mob-specific gesture states (sniffer/armadillo enum ordinals at
    /// metadata index 17). Cleared on entity removal.
    gesture_states: HashMap<i32, u8>,
    /// Slime / magma-cube size (metadata index 16). Cleared on removal.
    sizes: HashMap<i32, i32>,
    /// Baby flag (index 16 BOOLEAN, ageable / zombie mobs). Cleared on removal.
    babies: std::collections::HashSet<i32>,
    /// Model-visible entity-event receipt ticks (warden attack/sonic boom,
    /// armadillo peek). Cleared on removal AND on (re-)add, so a reused entity
    /// id can never inherit a previous occupant's animation timing — vanilla's
    /// `AnimationState`s die with the entity.
    events: HashMap<i32, EntityEventStarts>,
}

impl EntityTable {
    pub fn add(&mut self, id: i32, state: EntityState) {
        // A fresh entity at this id inherits no stale event timing — the
        // packet stream sends `remove_entities` before reusing an id, but a
        // dropped removal must not leave one warden's attack clock running on
        // its replacement.
        self.events.remove(&id);
        self.map.insert(id, state);
    }

    pub fn remove(&mut self, id: i32) {
        self.map.remove(&id);
        self.custom_names.remove(&id);
        self.poses.remove(&id);
        self.gesture_states.remove(&id);
        self.sizes.remove(&id);
        self.babies.remove(&id);
        self.events.remove(&id);
    }

    /// Stamp a model-visible entity event's receipt tick — an unconditional
    /// restart (vanilla `AnimationState.start(tick)`), so repeated events
    /// re-clock the rig from zero. Only called for a looked-up entity of the
    /// matching kind; the caller enforces that.
    pub fn start_event(&mut self, id: i32, event: EntityEvent, tick: i64) {
        self.events.entry(id).or_insert([None; EntityEvent::COUNT])[event.index()] = Some(tick);
    }

    /// The receipt tick of a model-visible event on an entity, or `None` if it
    /// has never fired. The renderer measures `(now − start) · 0.05 s` from it.
    pub fn event_start(&self, id: i32, event: EntityEvent) -> Option<i64> {
        self.events.get(&id).and_then(|e| e[event.index()])
    }

    /// Set / clear an entity's metadata custom name.
    pub fn set_custom_name(&mut self, id: i32, name: Option<String>) {
        match name {
            Some(n) => {
                self.custom_names.insert(id, n);
            }
            None => {
                self.custom_names.remove(&id);
            }
        }
    }

    pub fn custom_name(&self, id: i32) -> Option<&str> {
        self.custom_names.get(&id).map(|s| s.as_str())
    }

    /// Entity pose ordinal from metadata (index 6) — STANDING=0 default.
    pub fn set_pose(&mut self, id: i32, pose: u8) {
        self.poses.insert(id, pose);
    }

    pub fn pose(&self, id: i32) -> u8 {
        self.poses.get(&id).copied().unwrap_or(0)
    }

    /// Mob gesture state (sniffer/armadillo/… enum ordinal at index 17).
    pub fn set_gesture_state(&mut self, id: i32, state: u8) {
        self.gesture_states.insert(id, state);
    }

    pub fn gesture_state(&self, id: i32) -> u8 {
        self.gesture_states.get(&id).copied().unwrap_or(0)
    }

    /// Slime / magma-cube size (index 16). Vanilla default is 1; callers
    /// choose their own fallback for entities that never sent it.
    pub fn set_size(&mut self, id: i32, size: i32) {
        self.sizes.insert(id, size);
    }

    pub fn size(&self, id: i32) -> Option<i32> {
        self.sizes.get(&id).copied()
    }

    /// Baby flag (index 16 BOOLEAN). `set` toggles membership.
    pub fn set_baby(&mut self, id: i32, baby: bool) {
        if baby {
            self.babies.insert(id);
        } else {
            self.babies.remove(&id);
        }
    }

    pub fn is_baby(&self, id: i32) -> bool {
        self.babies.contains(&id)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn get(&self, id: i32) -> Option<&EntityState> {
        self.map.get(&id)
    }

    pub fn get_mut(&mut self, id: i32) -> Option<&mut EntityState> {
        self.map.get_mut(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (i32, &EntityState)> {
        self.map.iter().map(|(id, e)| (*id, e))
    }

    /// Advance every entity's interpolation one 20 Hz tick.
    pub fn tick_lerp(&mut self) {
        for e in self.map.values_mut() {
            e.tick();
        }
    }

    pub fn set_name(&mut self, uuid: u128, name: String) {
        self.names.insert(uuid, name);
    }

    pub fn remove_name(&mut self, uuid: u128) {
        self.names.remove(&uuid);
    }

    pub fn name_of(&self, uuid: u128) -> Option<&str> {
        self.names.get(&uuid).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_step_lerp_converges_exactly() {
        let mut e = EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
        e.set_target(3.0, 0.0, 0.0);
        e.tick(); // (3-0)/3 → 1
        assert_eq!(e.render_pos(1.0)[0], 1.0);
        assert_eq!(e.render_pos(0.5)[0], 0.5, "partial tick blends prev→cur");
        e.tick(); // (3-1)/2 → 2
        e.tick(); // (3-2)/1 → 3 exact
        assert_eq!(e.render_pos(1.0)[0], 3.0);
        e.tick(); // no steps left — stays put
        assert_eq!(e.render_pos(0.0)[0], 3.0);
    }

    #[test]
    fn deltas_accumulate_on_the_target_not_the_render_pos() {
        let mut e = EntityState::new(0, 0, 10.0, 0.0, 0.0, 0.0, 0.0);
        e.nudge(0.5, 0.0, 0.0);
        e.nudge(0.5, 0.0, 0.0); // mid-lerp — target must not lose the first
        assert_eq!(e.x, 11.0);
        for _ in 0..3 {
            e.tick();
        }
        assert_eq!(e.render_pos(1.0)[0], 11.0);
    }

    #[test]
    fn still_entity_has_no_limb_swing_but_walking_builds_it_up() {
        let mut e = EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for _ in 0..10 {
            e.tick(); // no target set → cur never moves
        }
        let (_, amt_still) = e.limb();
        assert!(amt_still < 1e-6, "still entity: amount {amt_still}");
        // Now walk ~0.2 blk/tick and let the smoother ramp.
        for _ in 0..20 {
            e.nudge(0.2, 0.0, 0.0);
            e.tick();
        }
        let (swing, amt) = e.limb();
        assert!(amt > 0.5, "sustained walk drives amount up: {amt}");
        assert!(swing > 0.0, "phase advanced: {swing}");
    }

    #[test]
    fn events_restart_and_clear_on_lifecycle() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), None);
        // A stamp records the receipt tick.
        t.start_event(1, EntityEvent::WardenAttack, 100);
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), Some(100));
        // Independent slots: sonic boom does not disturb attack.
        t.start_event(1, EntityEvent::WardenSonicBoom, 105);
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), Some(100));
        assert_eq!(t.event_start(1, EntityEvent::WardenSonicBoom), Some(105));
        // A repeated event is an unconditional restart (new tick).
        t.start_event(1, EntityEvent::WardenAttack, 140);
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), Some(140));
        // Removal clears every slot.
        t.remove(1);
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), None);
        assert_eq!(t.event_start(1, EntityEvent::WardenSonicBoom), None);
    }

    #[test]
    fn readding_an_id_drops_stale_event_timing() {
        let mut t = EntityTable::default();
        t.add(7, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.start_event(7, EntityEvent::ArmadilloPeek, 50);
        // Re-adding the same id (a dropped despawn, then a new occupant) must
        // not inherit the previous entity's peek clock.
        t.add(7, EntityState::new(1, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        assert_eq!(t.event_start(7, EntityEvent::ArmadilloPeek), None);
    }

    #[test]
    fn names_are_keyed_by_uuid_independent_of_entities() {
        let mut t = EntityTable::default();
        t.set_name(7, "Vwyla".into());
        t.add(1, EntityState::new(7, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        assert_eq!(t.name_of(t.get(1).unwrap().uuid), Some("Vwyla"));
        t.remove(1);
        assert_eq!(t.name_of(7), Some("Vwyla"), "name outlives the entity");
        t.remove_name(7);
        assert_eq!(t.name_of(7), None);
    }
}
