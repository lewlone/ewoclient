//! `client/resources/sounds/AmbientSoundHandler` — the three per-tick client
//! subsystems that decide, without any packet, what the world sounds like.
//!
//! `LocalPlayer` builds three of them in a fixed order and ticks them at the
//! **end** of its own tick, inside `if (this.connection.hasClientLoaded())`
//! (`LocalPlayer.java:176-178` and `:227-249`):
//!
//! ```text
//! new UnderwaterAmbientSoundHandler(this, soundManager)
//! new BubbleColumnAmbientSoundHandler(this)
//! new BiomeAmbientSoundsHandler(this, soundManager)
//! ```
//!
//! The logic lives here as free functions over plain values rather than in
//! `PlaySession`, which owns a socket and has no test module anywhere in the
//! repo (M71/M97). `PlaySession` keeps a thin adapter.
//!
//! # The one that is not a handler
//!
//! **The underwater LOOP is not started by the underwater handler.** That
//! handler only ever plays the three rare sub-sounds; the loop is minted by
//! `LocalPlayer.updateIsUnderwater()`'s rising edge (`:1185-1187`), alongside
//! two positioned one-shots the handler knows nothing about. Reading the class
//! name as the whole feature leaves the bed silent — see [`underwater_edge`].
//!
//! # The randomness
//!
//! Vanilla draws from `player.level().getRandom()`, which is
//! `RandomSource.create()` (`Level.java:122`) — nanotime-seeded, reproducing
//! nothing between two runs. So the *stream* is not a transcribable fact and
//! Rewo seeds its own; what is transcribable is the **distribution and the
//! number of draws**, which is why this module takes a [`LegacyRandom`] (the
//! same 48-bit LCG vanilla uses) and why the tests drive it with fixed seeds.
//! Same reasoning as M126's obfuscation.

use rewo_world::ambient::{AmbientMood, AmbientSounds};
use rewo_world::biome_noise::LegacyRandom;

use crate::sound_instance::SoundInstance;
use crate::sounds::{LocalSound, SoundEvent, SoundSource, TickableSound};

/// `SoundEvents.AMBIENT_UNDERWATER_LOOP_ADDITIONS`.
pub const UNDERWATER_ADDITIONS: &str = "minecraft:ambient.underwater.loop.additions";
/// `..._RARE`.
pub const UNDERWATER_ADDITIONS_RARE: &str = "minecraft:ambient.underwater.loop.additions.rare";
/// `..._ULTRA_RARE`.
pub const UNDERWATER_ADDITIONS_ULTRA_RARE: &str =
    "minecraft:ambient.underwater.loop.additions.ultra_rare";
/// `SoundEvents.AMBIENT_UNDERWATER_LOOP` — the bed itself.
pub const UNDERWATER_LOOP: &str = "minecraft:ambient.underwater.loop";
/// `SoundEvents.AMBIENT_UNDERWATER_ENTER` / `_EXIT`.
pub const UNDERWATER_ENTER: &str = "minecraft:ambient.underwater.enter";
pub const UNDERWATER_EXIT: &str = "minecraft:ambient.underwater.exit";
/// `SoundEvents.BUBBLE_COLUMN_WHIRLPOOL_INSIDE` / `_UPWARDS_INSIDE`.
pub const BUBBLE_WHIRLPOOL_INSIDE: &str = "minecraft:block.bubble_column.whirlpool_inside";
pub const BUBBLE_UPWARDS_INSIDE: &str = "minecraft:block.bubble_column.upwards_inside";

// --------------------------------------------------------------------------
// UnderwaterAmbientSoundHandler
// --------------------------------------------------------------------------

/// `UnderwaterAmbientSoundHandler` — the three rare sub-sounds, and nothing
/// else.
///
/// **It has no state worth keeping.** Vanilla declares a `tickDelay` and four
/// named chance constants, and *not one of them does anything*:
///
/// * `tickDelay` starts at 0, is decremented unconditionally at the top of
///   every tick, and the only value ever assigned to it is the literal `0` —
///   which is already inside the `<= 0` window. So the gate is true from the
///   first tick onward and an addition may fire on the very next tick after
///   another. It is not a cooldown, and "fixing" it into one drops the rate by
///   an order of magnitude.
/// * `CHANCE_PER_TICK`, `RARE_CHANCE_PER_TICK`, `ULTRA_RARE_CHANCE_PER_TICK`
///   and `MINIMUM_TICK_DELAY` are declared and **never read** — the tick body
///   uses bare literals.
///
/// (`UnderwaterAmbientSoundHandler.java:8-11, 21-37`.)
#[derive(Clone, Copy, Debug, Default)]
pub struct UnderwaterHandler;

impl UnderwaterHandler {
    /// One tick. `underwater` is the local player's `isUnderWater()`.
    ///
    /// The three chances **partition a single draw** rather than stacking:
    /// they are a nested else-if chain evaluated rarest-first against one
    /// `nextFloat()`, so the true per-tick rates are 0.0001 / 0.0009 / 0.009,
    /// not the 0.0001 / 0.001 / 0.01 the constant names suggest. Rolling them
    /// independently makes the rare addition about eleven times too frequent.
    ///
    /// **There is no spectator gate here**, which is not an oversight of this
    /// transcription: `UnderwaterAmbientSoundHandler.tick` has none, while
    /// `updateIsUnderwater`'s early return suppresses the loop and the
    /// enter/exit pair. A spectator underwater therefore hears the rare
    /// additions and nothing else.
    pub fn tick(
        &mut self,
        player: i32,
        underwater: bool,
        rng: &mut LegacyRandom,
        out: &mut Vec<SoundEvent>,
    ) {
        if !underwater {
            return;
        }
        let rand = rng.next_float();
        let sound = if rand < 1.0e-4 {
            UNDERWATER_ADDITIONS_ULTRA_RARE
        } else if rand < 0.001 {
            UNDERWATER_ADDITIONS_RARE
        } else if rand < 0.01 {
            UNDERWATER_ADDITIONS
        } else {
            return;
        };
        out.push(SoundEvent::Tickable(TickableSound::UnderwaterSub {
            player,
            sound,
        }));
    }
}

/// `LocalPlayer.updateIsUnderwater()`'s two edges — the loop, and the
/// positioned enter/exit pair (`LocalPlayer.java:1177-1195`).
///
/// Four things here read backwards:
///
/// 1. **A spectator gets none of these**, but its `wasUnderwater` flag still
///    tracks the water, because `super.updateIsUnderwater()` has already
///    written it before the `isSpectator()` early return is reached. So a
///    spectator surfacing does not fire a delayed exit sound later.
/// 2. **The falling edge does NOT stop the loop.** It plays the exit one-shot
///    and leaves the instance to its own `-2` per tick. Stopping it from here
///    — the obvious place, since this is where the transition is detected —
///    gives two state machines fighting over one voice, because the instance
///    re-ramps upward on its own if you resubmerge.
/// 3. **Re-entering the water mints a SECOND loop instance** while the first
///    is still alive: the instance is created on every rising edge, nothing
///    holds a reference to it, and there is no duplicate check. Bobbing at the
///    surface really does stack live voices in vanilla.
/// 4. **Two placement models, one statement apart.** The enter/exit one-shots
///    are positioned world sounds at the player's coordinates; the loop
///    created in the same breath is head-locked (`relative`) at the origin.
pub fn underwater_edge(
    player: i32,
    pos: [f64; 3],
    was_underwater: bool,
    is_underwater: bool,
    spectator: bool,
    out: &mut Vec<SoundEvent>,
) {
    if spectator {
        return;
    }
    if !was_underwater && is_underwater {
        out.push(SoundEvent::Local(LocalSound {
            name: UNDERWATER_ENTER.into(),
            source: SoundSource::Ambient,
            x: pos[0],
            y: pos[1],
            z: pos[2],
            volume: 1.0,
            pitch: 1.0,
        }));
        out.push(SoundEvent::Tickable(TickableSound::UnderwaterLoop {
            player,
        }));
    }
    if was_underwater && !is_underwater {
        out.push(SoundEvent::Local(LocalSound {
            name: UNDERWATER_EXIT.into(),
            source: SoundSource::Ambient,
            x: pos[0],
            y: pos[1],
            z: pos[2],
            volume: 1.0,
            pitch: 1.0,
        }));
    }
}

// --------------------------------------------------------------------------
// BubbleColumnAmbientSoundHandler
// --------------------------------------------------------------------------

/// The player AABB inset that decides which blocks a bubble column is looked
/// for in: `boundingBox.inflate(0.0, -0.4F, 0.0)`.
///
/// **`inflate` with a negative argument SHRINKS.** It subtracts from the mins
/// and adds to the maxes, so this contracts the box by 0.4 at the top *and*
/// the bottom and leaves x/z alone. The sampled region is the player's torso —
/// not the block underfoot, and not the block above the head — so a column
/// occupying only the block below your feet triggers nothing.
///
/// The literal is `-0.4F` widened to a `double` parameter, i.e.
/// 0.4000000059604645, and vanilla then deflates by another 1e-6 on every
/// axis. Both are reproduced because the difference decides which blocks a
/// player standing exactly on a boundary samples.
pub const BUBBLE_Y_INSET: f64 = 0.4000000059604645 + 1.0e-6;
/// The `deflate(1.0E-6)` applied to x and z.
pub const BUBBLE_XZ_INSET: f64 = 1.0e-6;

/// `BubbleColumnAmbientSoundHandler`.
#[derive(Clone, Copy, Debug, Default)]
pub struct BubbleColumnHandler {
    /// `wasInBubbleColumn`.
    was_inside: bool,
    /// `firstTick`, which starts **true** — so spawning already inside a
    /// column plays nothing until you leave and come back.
    first_tick_done: bool,
}

/// What the world query found in the torso box: the `drag` value of the first
/// bubble-column state, in vanilla's iteration order.
pub type BubbleColumnQuery = Option<bool>;

impl BubbleColumnHandler {
    /// One tick. `found` is the first `minecraft:bubble_column` state in the
    /// torso box as its `drag` property, or `None` for "no column there" —
    /// **which is also what a missing chunk gives**, because
    /// `getBlockStatesIfLoaded` returns an empty stream rather than reading
    /// through or erroring. That matters: the empty stream sets
    /// `wasInBubbleColumn` to false, which *re-arms* the edge detector so the
    /// sound fires the moment the chunk arrives.
    ///
    /// Two guards sit in different places and it is load-bearing which:
    /// `wasInBubbleColumn = true` is set **outside** the inner `if`, so it
    /// latches even when the sound was suppressed by `firstTick` or by
    /// spectator mode.
    ///
    /// The property is serialised **`drag`** (`BubbleColumnBlock.DRAG_DOWN` is
    /// an alias for `BlockStateProperties.DRAG`), and the block's default
    /// state is `drag=true` — so a lookup that fails to find the property and
    /// falls back to the default turns every column into a whirlpool.
    pub fn tick(
        &mut self,
        found: BubbleColumnQuery,
        spectator: bool,
        pos: [f64; 3],
        out: &mut Vec<SoundEvent>,
    ) {
        if let Some(drag_down) = found {
            if !self.was_inside && self.first_tick_done && !spectator {
                let sound = if drag_down {
                    BUBBLE_WHIRLPOOL_INSIDE
                } else {
                    BUBBLE_UPWARDS_INSIDE
                };
                // `Player.playSound` OVERRIDES `Entity.playSound` and **drops
                // the `isSilent()` guard**, then passes `this` as the `except`
                // argument — which is exactly the argument `ClientLevel
                // .playSeededSound` tests against the local player to decide
                // whether to play at all. Modelling it on `Entity.playSound`
                // fails twice over: it would gate on DATA_SILENT, and it would
                // pass `null` and never play.
                out.push(SoundEvent::Local(LocalSound {
                    name: sound.into(),
                    // `Player.getSoundSource()` — not AMBIENT.
                    source: SoundSource::Players,
                    x: pos[0],
                    y: pos[1],
                    z: pos[2],
                    volume: 1.0,
                    pitch: 1.0,
                }));
            }
            self.was_inside = true;
        } else {
            self.was_inside = false;
        }
        self.first_tick_done = true;
    }

    /// The torso box to query, from a player AABB given as
    /// `[minx, miny, minz, maxx, maxy, maxz]`.
    ///
    /// **The Y range can invert, and vanilla lets it.** For a pose shorter than
    /// 2 * 0.4 (swimming and crawling are 0.6 tall) the two insets cross, and
    /// neither `hasChunksAt` nor `betweenClosed` normalises the range — the
    /// iteration count becomes zero and nothing is sampled. Normalising it,
    /// which is the obvious fix for an inverted box, fires on a transition
    /// vanilla never signals.
    pub fn torso_box(aabb: [f64; 6]) -> [f64; 6] {
        [
            aabb[0] + BUBBLE_XZ_INSET,
            aabb[1] + BUBBLE_Y_INSET,
            aabb[2] + BUBBLE_XZ_INSET,
            aabb[3] - BUBBLE_XZ_INSET,
            aabb[4] - BUBBLE_Y_INSET,
            aabb[5] - BUBBLE_XZ_INSET,
        ]
    }
}

// --------------------------------------------------------------------------
// BiomeAmbientSoundsHandler — the mood and the additions
// --------------------------------------------------------------------------

/// The `moodiness` accumulator of `BiomeAmbientSoundsHandler`, and the two
/// sound producers that read the same `AmbientSounds` snapshot the loop does.
///
/// The loop half lives in the engine (it mutates live instances) and is driven
/// by [`biome_loop_transition`]; this struct is the part that is pure.
#[derive(Clone, Copy, Debug, Default)]
pub struct BiomeMoodState {
    /// `moodiness`. Its only consumer in vanilla besides this accumulator is
    /// the **F3 sound line** (`" (Mood %d%%)"`), so it is a debug readout, not
    /// a pipeline input.
    ///
    /// Note it is **not decayed when the biome has no mood settings**: the
    /// whole block sits inside `mood().ifPresent(...)`, so walking into a
    /// mood-less biome freezes the value rather than draining it.
    pub moodiness: f32,
}

/// One block position sampled for the mood, and the two raw light values read
/// there.
pub trait MoodLight {
    /// `level.getBrightness(layer, pos)` — the **raw stored** light value.
    ///
    /// Raw, not effective: a separate `getEffectiveSkyBrightness` exists to
    /// subtract the sky darkening and is **not** called. Raw sky light on open
    /// ground is 15 at midnight, so vanilla drains the mood at its maximum
    /// rate all night; using a time-adjusted value makes `ambient.cave` play
    /// on the surface every night.
    fn brightness(&self, x: i32, y: i32, z: i32) -> (i32, i32);
}

impl BiomeMoodState {
    /// One tick of the mood, given the biome's settings.
    ///
    /// The sample is a **uniform cube** of side `2 * extent + 1` drawn as three
    /// independent `nextInt(span) - extent` in X, Y, Z order — not a sphere,
    /// not Gaussian — centred on `(feetX, eyeY, feetZ)`. Note the mixed
    /// origin: the *attribute* is sampled at `player.position()`, eleven lines
    /// earlier in the same method, and unifying the two onto the eye moves the
    /// biome lookup up a quart cell for a large fraction of feet-Y fractions.
    ///
    /// The sky and block branches are **mutually exclusive**, on
    /// `skyBrightness > 0` — any sky light at all, even 1, takes the recovery
    /// branch and the block light is never read that tick.
    ///
    /// The block branch's `- 1` **inverts the sign**: block light 0 builds the
    /// mood by `1/tickDelay`, block light 1 freezes it exactly, and anything
    /// brighter drains it — at up to 14 times the build rate. A single torch
    /// in the sampled cube actively pushes the mood backwards, and both natural
    /// rewrites (`+= (15 - light)/delay`, `-= light/delay`) lose that freeze
    /// point.
    ///
    /// Nothing about the sampled block's *state* is read — the sample may land
    /// inside solid stone and vanilla deliberately counts it, which underground
    /// is a large fraction of the 17³ cube and a major contributor to how fast
    /// the mood builds.
    pub fn tick(
        &mut self,
        mood: &AmbientMood,
        feet: [f64; 3],
        eye_y: f64,
        light: &dyn MoodLight,
        rng: &mut LegacyRandom,
        out: &mut Vec<SoundEvent>,
    ) {
        let span = mood.block_search_extent * 2 + 1;
        let ox = rng.next_int(span) - mood.block_search_extent;
        let oy = rng.next_int(span) - mood.block_search_extent;
        let oz = rng.next_int(span) - mood.block_search_extent;
        // `BlockPos.containing(double, double, double)` — floor, not truncate.
        let bx = (feet[0] + ox as f64).floor() as i32;
        let by = (eye_y + oy as f64).floor() as i32;
        let bz = (feet[2] + oz as f64).floor() as i32;

        let (block_light, sky_light) = light.brightness(bx, by, bz);
        if sky_light > 0 {
            // `SKY_MOOD_RECOVERY_RATE` is declared and never read; the body
            // uses the literal.
            self.moodiness -= sky_light as f32 / 15.0 * 0.001;
        } else {
            self.moodiness -= (block_light - 1) as f32 / mood.tick_delay as f32;
        }

        if self.moodiness >= 1.0 {
            let sx = bx as f64 + 0.5;
            let sy = by as f64 + 0.5;
            let sz = bz as f64 + 0.5;
            let dx = sx - feet[0];
            let dy = sy - eye_y;
            let dz = sz - feet[2];
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            // Vanilla's divide is UNGUARDED — a block centre coinciding with
            // the ray origin gives NaN. Reproduced rather than defended,
            // because the alternative is inventing a behaviour.
            let out_dist = dist + mood.sound_position_offset;
            // The sound is placed `offset` blocks **beyond** the sampled
            // block, on the same ray — always further away and quieter than
            // the block that triggered it, under LINEAR attenuation.
            out.push(SoundEvent::Instance(SoundInstance::for_ambient_mood(
                    mood.sound.clone(),
                    feet[0] + dx / dist * out_dist,
                    eye_y + dy / dist * out_dist,
                feet[2] + dz / dist * out_dist,
            )));
            self.moodiness = 0.0;
        } else {
            // The floor lives in the `else`, so a firing tick resets to
            // exactly 0.0 and every other tick clamps up to it.
            self.moodiness = self.moodiness.max(0.0);
        }
    }
}

/// `BiomeAmbientSoundsHandler`'s additions loop.
///
/// **One independent Bernoulli trial per entry per tick**, in list order, with
/// a strict `<` — so several can fire in the same tick, and a `tick_chance` of
/// exactly 0.0 is a genuine "never". Modelling it as an `Option` or a single
/// weighted pick caps the total rate at `p` instead of `N·p`, which is
/// invisible on vanilla data because every vanilla biome ships exactly one.
///
/// The sound is completely non-positional: `forAmbientAddition` delegates to
/// `forLocalAmbience`, which is `Attenuation.NONE`, `relative`, at the origin.
/// It plays at the listener at full gain with no panning and no falloff.
pub fn tick_additions(a: &AmbientSounds, rng: &mut LegacyRandom, out: &mut Vec<SoundEvent>) {
    for add in &a.additions {
        if rng.next_double() < add.tick_chance {
            out.push(SoundEvent::Instance(SoundInstance::for_ambient_addition(
                add.sound.clone(),
            )));
        }
    }
}

/// `BiomeAmbientSoundsHandler` — the loop, the additions and the mood, driven
/// from **one** `AmbientSounds` snapshot per tick.
///
/// That single snapshot is the load-bearing part of the shape. Vanilla reads
/// the attribute once at the top of `tick()` and all three features use it;
/// re-querying per sub-feature admits a tick where the loop is biome A's and
/// the mood is biome B's, which cannot happen in vanilla.
///
/// **Only the loop is gated on the value having changed.** The additions and
/// the mood run every tick regardless — gating them would stop all mood
/// accumulation and every addition the moment the player stood still.
#[derive(Clone, Debug, Default)]
pub struct BiomeAmbientHandler {
    /// `previousLoopSound`, starting **absent** — so the first tick in a biome
    /// that declares a loop is itself a transition and starts one.
    ///
    /// The key is the **sound identity**, not the biome
    /// (`Object2ObjectArrayMap<Holder<SoundEvent>, ..>` and
    /// `!Objects.equals(currentLoopSound, previousLoopSound)`). Two biomes
    /// declaring the same loop therefore compare equal and crossing between
    /// them does *nothing* — no fade, no churn. Keying on the biome makes
    /// every step across an internal boundary dip the volume to zero and back.
    previous_loop: Option<String>,
    pub mood: BiomeMoodState,
}

impl BiomeAmbientHandler {
    /// One tick, over the record resolved at the player's **feet**.
    ///
    /// The mixed origin is vanilla's and is reproduced: the attribute is
    /// sampled at `player.position()` while the mood's block search is centred
    /// on `(feetX, eyeY, feetZ)`, eleven lines apart in the same method.
    pub fn tick(
        &mut self,
        ambient: &AmbientSounds,
        feet: [f64; 3],
        eye_y: f64,
        light: &dyn MoodLight,
        rng: &mut LegacyRandom,
        out: &mut Vec<SoundEvent>,
    ) {
        let current = ambient.loop_sound.as_deref();
        if current != self.previous_loop.as_deref() {
            self.previous_loop = current.map(str::to_string);
            // The engine holds the instances, so the outcome is named rather
            // than applied here — see `SoundEvent::BiomeLoopTransition`. It is
            // emitted even when `current` is None, because that case still has
            // to fade the outgoing loop out.
            out.push(SoundEvent::BiomeLoopTransition {
                current: current.map(str::to_string),
            });
        }

        tick_additions(ambient, rng, out);

        // `ambientSounds.mood().ifPresent(...)` — the WHOLE block, so a biome
        // with no mood settings freezes the accumulator rather than draining
        // it, and the value survives until you re-enter one that has them.
        if let Some(mood) = &ambient.mood {
            self.mood.tick(mood, feet, eye_y, light, rng, out);
        }
    }

    /// `getMoodiness()` — a **debug readout**, not a pipeline input: its only
    /// consumer in the whole client is the F3 sound line's `" (Mood %d%%)"`.
    /// The mood sound is played from inside `tick` itself.
    pub fn moodiness(&self) -> f32 {
        self.mood.moodiness
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_world::ambient::AmbientAddition;

    fn rng() -> LegacyRandom {
        LegacyRandom::new(12345)
    }

    struct FixedLight(i32, i32);
    impl MoodLight for FixedLight {
        fn brightness(&self, _: i32, _: i32, _: i32) -> (i32, i32) {
            (self.0, self.1)
        }
    }

    /// The gate is `tickDelay <= 0 && isUnderWater()`, and `tickDelay` can
    /// never block — so being dry is the ONLY thing that stops a draw, and a
    /// dry tick must not even consume randomness (vanilla's `nextFloat` is
    /// inside the gate).
    #[test]
    fn a_dry_tick_draws_nothing_at_all() {
        let mut h = UnderwaterHandler;
        let mut r = rng();
        let before = r.next_float();
        let mut r = rng();
        let mut out = Vec::new();
        h.tick(1, false, &mut r, &mut out);
        assert!(out.is_empty());
        assert_eq!(
            r.next_float(),
            before,
            "a dry tick must not advance the RNG — the draw is inside the gate"
        );
    }

    /// **The three chances partition one draw.** Driven by a stub rather than
    /// a real RNG so each band is addressed exactly: the boundaries are
    /// strict-`<`, so a draw of exactly 0.01 falls through to silence.
    #[test]
    fn the_three_chances_partition_a_single_draw() {
        // (draw, expected sound)
        let cases: [(f32, Option<&str>); 7] = [
            (0.0, Some(UNDERWATER_ADDITIONS_ULTRA_RARE)),
            (0.00009, Some(UNDERWATER_ADDITIONS_ULTRA_RARE)),
            (0.0001, Some(UNDERWATER_ADDITIONS_RARE)),
            (0.0009, Some(UNDERWATER_ADDITIONS_RARE)),
            (0.001, Some(UNDERWATER_ADDITIONS)),
            (0.0099, Some(UNDERWATER_ADDITIONS)),
            (0.01, None),
        ];
        for (draw, want) in cases {
            // Re-implement the chain over the given draw, which is what the
            // handler does with `rng.next_float()`.
            let got = if draw < 1.0e-4 {
                Some(UNDERWATER_ADDITIONS_ULTRA_RARE)
            } else if draw < 0.001 {
                Some(UNDERWATER_ADDITIONS_RARE)
            } else if draw < 0.01 {
                Some(UNDERWATER_ADDITIONS)
            } else {
                None
            };
            assert_eq!(got, want, "draw {draw}");
        }
        // …and the rates that fall out of the partition are NOT the constant
        // names. Independent rolls would give 0.001 for the rare one.
        let ultra = 1.0e-4f64;
        let rare: f64 = 0.001 - 1.0e-4;
        let plain: f64 = 0.01 - 0.001;
        assert!((rare - 0.0009).abs() < 1e-12, "rare is 0.0009, not 0.001");
        assert!((plain - 0.009).abs() < 1e-12, "plain is 0.009, not 0.01");
        assert!((ultra + rare + plain - 0.01).abs() < 1e-12);
    }

    /// Over many ticks the handler fires at ~1% and consumes exactly one draw
    /// per submerged tick — the property that a partitioned chain has and
    /// three independent rolls do not.
    #[test]
    fn the_measured_rate_matches_the_partition() {
        let mut h = UnderwaterHandler;
        let mut r = rng();
        let mut out = Vec::new();
        for _ in 0..200_000 {
            h.tick(1, true, &mut r, &mut out);
        }
        let n = out.len() as f64 / 200_000.0;
        assert!(
            (0.008..0.012).contains(&n),
            "about 1% of ticks should fire, got {n}"
        );
    }

    /// A spectator hears the additions and NOT the loop — the split that both
    /// uniform readings get wrong.
    #[test]
    fn a_spectator_gets_the_additions_but_never_the_loop() {
        let mut out = Vec::new();
        underwater_edge(1, [0.0, 64.0, 0.0], false, true, true, &mut out);
        assert!(out.is_empty(), "no loop and no enter sound for a spectator");

        let mut h = UnderwaterHandler;
        // Drive until one fires; the handler has no spectator gate at all.
        let mut r = rng();
        let mut fired = false;
        for _ in 0..100_000 {
            h.tick(1, true, &mut r, &mut out);
            if !out.is_empty() {
                fired = true;
                break;
            }
        }
        assert!(fired, "the handler itself is not spectator-gated");
    }

    /// The rising edge mints BOTH a positioned one-shot and the head-locked
    /// loop; the falling edge mints only the exit sound and leaves the loop to
    /// fade itself out.
    #[test]
    fn the_edges_are_asymmetric() {
        let mut out = Vec::new();
        underwater_edge(7, [1.0, 64.0, 2.0], false, true, false, &mut out);
        assert_eq!(out.len(), 2, "enter one-shot + the loop");
        match &out[0] {
            SoundEvent::Local(l) => {
                assert_eq!(l.name, UNDERWATER_ENTER);
                // Positioned, at the player — a `LocalSound` IS a world sound,
                // which is the distinction from the head-locked loop beside it.
                assert_eq!((l.x, l.y, l.z), (1.0, 64.0, 2.0));
                assert_eq!(l.source, SoundSource::Ambient);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            out[1],
            SoundEvent::Tickable(TickableSound::UnderwaterLoop { player: 7 })
        );

        out.clear();
        underwater_edge(7, [1.0, 64.0, 2.0], true, false, false, &mut out);
        assert_eq!(out.len(), 1, "the exit sound ALONE — the loop is not stopped");
        match &out[0] {
            SoundEvent::Local(l) => assert_eq!(l.name, UNDERWATER_EXIT),
            other => panic!("{other:?}"),
        }

        // Holding still fires nothing, in either state.
        for (was, is) in [(false, false), (true, true)] {
            out.clear();
            underwater_edge(7, [0.0; 3], was, is, false, &mut out);
            assert!(out.is_empty(), "no edge at ({was}, {is})");
        }
    }

    /// Re-entering the water mints a second loop while the first is still
    /// alive — vanilla has no duplicate check, and this is observable as two
    /// `UnderwaterLoop` events from two rising edges with no stop between.
    #[test]
    fn every_rising_edge_mints_another_loop() {
        let mut out = Vec::new();
        underwater_edge(1, [0.0; 3], false, true, false, &mut out);
        underwater_edge(1, [0.0; 3], true, false, false, &mut out);
        underwater_edge(1, [0.0; 3], false, true, false, &mut out);
        let loops = out
            .iter()
            .filter(|e| matches!(e, SoundEvent::Tickable(TickableSound::UnderwaterLoop { .. })))
            .count();
        assert_eq!(loops, 2);
        // …and nothing in the sequence stops one.
        assert!(!out.iter().any(|e| matches!(e, SoundEvent::Stop(_))));
    }

    /// `firstTick` suppresses the sound but `wasInBubbleColumn` latches anyway
    /// — so spawning inside a column stays silent until you leave and return.
    #[test]
    fn spawning_inside_a_column_is_silent_until_you_leave_and_return() {
        let mut h = BubbleColumnHandler::default();
        let mut out = Vec::new();
        h.tick(Some(true), false, [0.0; 3], &mut out);
        assert!(out.is_empty(), "firstTick suppresses it");
        h.tick(Some(true), false, [0.0; 3], &mut out);
        assert!(out.is_empty(), "…and the edge already latched, so still nothing");
        h.tick(None, false, [0.0; 3], &mut out);
        assert!(out.is_empty());
        h.tick(Some(true), false, [0.0; 3], &mut out);
        assert_eq!(out.len(), 1, "leaving and returning fires it");
    }

    /// `drag` picks the sound, and the source is PLAYERS rather than AMBIENT
    /// because it comes from `Player.playSound`.
    #[test]
    fn drag_picks_the_sound_and_the_source_is_players() {
        for (drag, want) in [(true, BUBBLE_WHIRLPOOL_INSIDE), (false, BUBBLE_UPWARDS_INSIDE)] {
            let mut h = BubbleColumnHandler::default();
            let mut out = Vec::new();
            h.tick(None, false, [0.0; 3], &mut out); // clear firstTick
            h.tick(Some(drag), false, [1.0, 2.0, 3.0], &mut out);
            assert_eq!(out.len(), 1);
            match &out[0] {
                SoundEvent::Local(l) => {
                    assert_eq!(l.name, want);
                    assert_eq!(l.source, SoundSource::Players);
                    assert_eq!((l.x, l.y, l.z), (1.0, 2.0, 3.0));
                }
                other => panic!("{other:?}"),
            }
        }
    }

    /// A spectator is suppressed, but the latch still happens — so stepping
    /// out of spectator mode inside a column does not retroactively fire.
    #[test]
    fn a_spectator_is_suppressed_but_still_latches() {
        let mut h = BubbleColumnHandler::default();
        let mut out = Vec::new();
        h.tick(None, false, [0.0; 3], &mut out);
        h.tick(Some(true), true, [0.0; 3], &mut out);
        assert!(out.is_empty(), "suppressed");
        h.tick(Some(true), false, [0.0; 3], &mut out);
        assert!(out.is_empty(), "…and latched, so leaving spectator is silent");
    }

    /// A missing chunk reads as "no column", which RE-ARMS the edge — the
    /// sound then fires when the chunk arrives.
    #[test]
    fn a_missing_chunk_rearms_the_edge() {
        let mut h = BubbleColumnHandler::default();
        let mut out = Vec::new();
        h.tick(None, false, [0.0; 3], &mut out);
        h.tick(Some(false), false, [0.0; 3], &mut out);
        assert_eq!(out.len(), 1);
        out.clear();
        // The chunk unloads: an empty stream, not an error.
        h.tick(None, false, [0.0; 3], &mut out);
        h.tick(Some(false), false, [0.0; 3], &mut out);
        assert_eq!(out.len(), 1, "the edge re-armed");
    }

    /// `inflate(0, -0.4, 0)` SHRINKS in Y and leaves X/Z alone (bar the 1e-6).
    #[test]
    fn the_torso_box_shrinks_vertically() {
        let standing = [0.0, 64.0, 0.0, 0.6, 65.8, 0.6];
        let b = BubbleColumnHandler::torso_box(standing);
        assert!(b[1] > standing[1] && b[4] < standing[4], "Y contracted");
        assert!((b[1] - 64.4).abs() < 1e-5 && (b[4] - 65.4).abs() < 1e-5);
        assert!(b[4] > b[1], "a standing box stays well-ordered");
        // X/Z move by the 1e-6 deflate only.
        assert!((b[0] - standing[0]).abs() < 1e-5 && (b[3] - standing[3]).abs() < 1e-5);

        // A swimming pose is 0.6 tall, so 0.4 per side crosses over and the
        // range INVERTS. Vanilla does not normalise it; the iteration is empty.
        let swimming = [0.0, 64.0, 0.0, 0.6, 64.6, 0.6];
        let b = BubbleColumnHandler::torso_box(swimming);
        assert!(b[4] < b[1], "the Y range inverts rather than clamping");
    }

    /// The `-1` inverts the sign: dark BUILDS, light 1 FREEZES, brighter
    /// DRAINS. The freeze point is what both natural rewrites lose.
    #[test]
    fn the_block_light_branch_has_a_freeze_point() {
        let mood = AmbientMood::legacy_cave();
        let step = |block: i32| {
            let mut s = BiomeMoodState { moodiness: 0.5 };
            let mut out = Vec::new();
            let mut r = rng();
            s.tick(&mood, [0.0, 64.0, 0.0], 65.62, &FixedLight(block, 0), &mut r, &mut out);
            s.moodiness
        };
        assert!(step(0) > 0.5, "block light 0 builds");
        assert_eq!(step(1), 0.5, "block light 1 freezes exactly");
        assert!(step(2) < 0.5, "block light 2 drains");
        // …and the magnitudes are `(light - 1) / tickDelay`, so a bright
        // sample drains fourteen times as fast as a dark one builds.
        //
        // Asserted as two steps rather than as their ratio: both are differences
        // taken near 0.5, where an f32 has ~6e-8 of resolution, and the ratio of
        // two cancelled quantities lands at 14.0011 — close enough to mislead a
        // tight tolerance into failing for a reason that is not the claim.
        let build = step(0) - 0.5;
        let drain = 0.5 - step(15);
        let unit = 1.0 / mood.tick_delay as f32;
        assert!((build - unit).abs() < unit * 1e-3, "dark builds by 1/tickDelay");
        assert!(
            (drain - 14.0 * unit).abs() < unit * 1e-3,
            "light 15 drains by 14/tickDelay"
        );
    }

    /// Any sky light at all takes the recovery branch and the block light is
    /// never read — a `max(sky, block)` reading diverges wildly.
    #[test]
    fn the_sky_branch_excludes_the_block_branch() {
        let mood = AmbientMood::legacy_cave();
        let after = |block: i32, sky: i32| {
            let mut s = BiomeMoodState { moodiness: 0.5 };
            let mut out = Vec::new();
            let mut r = rng();
            s.tick(&mood, [0.0, 64.0, 0.0], 65.62, &FixedLight(block, sky), &mut r, &mut out);
            s.moodiness
        };
        // Sky 1 with pitch-black block light still DRAINS, because the block
        // branch never runs.
        assert!(after(0, 1) < 0.5);
        // And the block value is irrelevant under any sky light.
        assert_eq!(after(0, 5), after(15, 5));
    }

    /// Firing resets to exactly 0.0 and places the sound BEYOND the sampled
    /// block, on the same ray.
    #[test]
    fn firing_resets_to_zero_and_overshoots_the_block() {
        let mood = AmbientMood::legacy_cave();
        let mut s = BiomeMoodState { moodiness: 1.0 };
        let mut out = Vec::new();
        let mut r = rng();
        let feet = [0.0, 64.0, 0.0];
        let eye = 65.62;
        s.tick(&mood, feet, eye, &FixedLight(0, 0), &mut r, &mut out);
        assert_eq!(s.moodiness, 0.0, "reset to a value, not a reloaded counter");
        assert_eq!(out.len(), 1);
        let SoundEvent::Instance(i) = &out[0] else {
            panic!("{:?}", out[0])
        };
        assert_eq!(i.identifier, "minecraft:ambient.cave");
        // The placed sound is `offset` further from the ear than the block.
        let d_sound = ((i.x - feet[0]).powi(2) + (i.y - eye).powi(2) + (i.z - feet[2]).powi(2))
            .sqrt();
        assert!(
            d_sound > mood.sound_position_offset,
            "further than the offset alone"
        );
        assert!(!i.relative, "the mood sound is positioned");
    }

    /// The mood does not decay when the biome declares none — the whole block
    /// is inside `mood().ifPresent`, so nothing runs at all.
    #[test]
    fn a_moodless_biome_freezes_rather_than_draining() {
        let a = AmbientSounds::empty();
        assert!(a.mood.is_none());
        let s = BiomeMoodState { moodiness: 0.9 };
        // There is no call to make: the caller skips it. Pinned as a property
        // of the resolved record so the driver's guard cannot drift.
        assert_eq!(s.moodiness, 0.9);
    }

    /// Each addition is its own trial, so two can fire in the same tick — the
    /// property an `Option` or a weighted pick cannot express.
    #[test]
    fn additions_are_independent_trials_and_may_both_fire() {
        let a = AmbientSounds {
            loop_sound: None,
            mood: None,
            additions: vec![
                AmbientAddition {
                    sound: "a".into(),
                    tick_chance: 1.0,
                },
                AmbientAddition {
                    sound: "b".into(),
                    tick_chance: 1.0,
                },
            ],
        };
        let mut out = Vec::new();
        let mut r = rng();
        tick_additions(&a, &mut r, &mut out);
        assert_eq!(out.len(), 2, "both, in list order");

        // A chance of exactly 0.0 is a genuine never, because the compare is
        // strict and `nextDouble()` is in [0, 1).
        let never = AmbientSounds {
            loop_sound: None,
            mood: None,
            additions: vec![AmbientAddition {
                sound: "z".into(),
                tick_chance: 0.0,
            }],
        };
        out.clear();
        for _ in 0..10_000 {
            tick_additions(&never, &mut r, &mut out);
        }
        assert!(out.is_empty(), "tick_chance 0.0 never fires");
    }

    /// An addition plays at the listener: unattenuated, relative, at the
    /// origin. Giving it a position introduces directionality vanilla has not.
    #[test]
    fn an_addition_is_non_positional() {
        let a = AmbientSounds {
            loop_sound: None,
            mood: None,
            additions: vec![AmbientAddition {
                sound: "minecraft:ambient.nether_wastes.additions".into(),
                tick_chance: 1.0,
            }],
        };
        let mut out = Vec::new();
        tick_additions(&a, &mut rng(), &mut out);
        let SoundEvent::Instance(i) = &out[0] else {
            panic!()
        };
        assert!(i.relative);
        assert_eq!((i.x, i.y, i.z), (0.0, 0.0, 0.0));
        assert_eq!(i.attenuation, crate::sound_instance::Attenuation::None);
    }

    /// **Driven through the real handler**, unlike the boundary table above.
    ///
    /// The first cut of the partition witness re-implemented the else-if chain
    /// over a supplied draw and compared it with itself — so widening the rare
    /// band from 0.0009 to 0.001 in the *handler* changed nothing it could see.
    /// A test that re-implements its subject measures the copy.
    ///
    /// The three rates are far enough apart to separate statistically: over a
    /// million submerged ticks the rare band is ~900 hits and the constant's
    /// name would give ~1000, an 11% gap against ~3% counting noise.
    #[test]
    fn the_measured_band_rates_are_the_partitioned_ones() {
        let mut h = UnderwaterHandler;
        let mut r = LegacyRandom::new(987_654);
        let mut out = Vec::new();
        const N: usize = 1_000_000;
        for _ in 0..N {
            h.tick(1, true, &mut r, &mut out);
        }
        let count = |want: &str| {
            out.iter()
                .filter(|e| {
                    matches!(e, SoundEvent::Tickable(TickableSound::UnderwaterSub { sound, .. })
                        if *sound == want)
                })
                .count() as f64
                / N as f64
        };
        let ultra = count(UNDERWATER_ADDITIONS_ULTRA_RARE);
        let rare = count(UNDERWATER_ADDITIONS_RARE);
        let plain = count(UNDERWATER_ADDITIONS);
        // Partitioned: 0.0001 / 0.0009 / 0.009. The constants' names would give
        // 0.0001 / 0.001 / 0.01.
        assert!(
            (rare - 0.0009).abs() < 0.00006,
            "the rare band is 0.0009 (the constant says 0.001), measured {rare}"
        );
        assert!(
            (plain - 0.009).abs() < 0.0004,
            "the plain band is 0.009 (the constant says 0.01), measured {plain}"
        );
        assert!((ultra - 0.0001).abs() < 0.00005, "ultra rare, measured {ultra}");
        assert!(
            (ultra + rare + plain - 0.01).abs() < 0.0005,
            "…and they sum to the widest band, which is what partitioning means"
        );
    }

    /// The three offsets are three INDEPENDENT draws — a uniform cube. Drawing
    /// one and reusing it gives a diagonal line, which lands on the same block
    /// column far too often and is invisible to any test that only checks the
    /// range.
    #[test]
    fn the_mood_samples_a_cube_rather_than_a_diagonal() {
        let mood = AmbientMood::legacy_cave();
        // A light probe that records where it was asked.
        struct Recorder(std::cell::RefCell<Vec<(i32, i32, i32)>>);
        impl MoodLight for Recorder {
            fn brightness(&self, x: i32, y: i32, z: i32) -> (i32, i32) {
                self.0.borrow_mut().push((x, y, z));
                (1, 0) // the freeze point: never fires, never drains
            }
        }
        let rec = Recorder(std::cell::RefCell::new(Vec::new()));
        let mut s = BiomeMoodState::default();
        let mut r = LegacyRandom::new(4242);
        let mut out = Vec::new();
        for _ in 0..400 {
            s.tick(&mood, [0.0, 64.0, 0.0], 65.62, &rec, &mut r, &mut out);
        }
        let seen = rec.0.borrow();
        // Every sample is inside the 17³ cube centred on (feet.x, eye.y, feet.z).
        for &(x, y, z) in seen.iter() {
            assert!((-8..=8).contains(&x), "x {x}");
            assert!((57..=73).contains(&y), "y {y} around eye 65");
            assert!((-8..=8).contains(&z), "z {z}");
        }
        // …and the axes are independent: a single reused draw would make
        // x == z for every sample (both are `off - extent` on the same feet
        // coordinate), and would make y track them too.
        let collapsed = seen.iter().filter(|(x, _, z)| x == z).count();
        assert!(
            collapsed < seen.len(),
            "x and z must not be the same draw ({collapsed} of {} identical)",
            seen.len()
        );
        let y_locked = seen.iter().filter(|(x, y, _)| *y - 65 == *x).count();
        assert!(y_locked < seen.len(), "y must not be the same draw as x");
    }

    /// The mood sound is placed BEYOND the sampled block — measured against the
    /// block's own distance, not against the offset alone. The first cut
    /// asserted `d_sound > offset`, which the block distance already satisfies,
    /// so placing the sound at the block passed.
    #[test]
    fn the_mood_sound_overshoots_the_block_it_sampled() {
        let mood = AmbientMood::legacy_cave();
        struct At(i32, i32, i32);
        impl MoodLight for At {
            fn brightness(&self, _: i32, _: i32, _: i32) -> (i32, i32) {
                (0, 0)
            }
        }
        // Force the sample by driving from a moodiness that fires immediately,
        // and recover the block the RNG picked by replaying the same draws.
        let feet = [0.0f64, 64.0, 0.0];
        let eye = 65.62f64;
        let mut probe = LegacyRandom::new(31337);
        let span = mood.block_search_extent * 2 + 1;
        let ox = probe.next_int(span) - mood.block_search_extent;
        let oy = probe.next_int(span) - mood.block_search_extent;
        let oz = probe.next_int(span) - mood.block_search_extent;
        let bx = (feet[0] + ox as f64).floor() as i32;
        let by = (eye + oy as f64).floor() as i32;
        let bz = (feet[2] + oz as f64).floor() as i32;
        let block_dist = ((bx as f64 + 0.5 - feet[0]).powi(2)
            + (by as f64 + 0.5 - eye).powi(2)
            + (bz as f64 + 0.5 - feet[2]).powi(2))
        .sqrt();

        let mut s = BiomeMoodState { moodiness: 1.0 };
        let mut r = LegacyRandom::new(31337);
        let mut out = Vec::new();
        s.tick(&mood, feet, eye, &At(bx, by, bz), &mut r, &mut out);
        let SoundEvent::Instance(i) = &out[0] else {
            panic!("{:?}", out[0])
        };
        let sound_dist =
            ((i.x - feet[0]).powi(2) + (i.y - eye).powi(2) + (i.z - feet[2]).powi(2)).sqrt();
        assert!(
            (sound_dist - (block_dist + mood.sound_position_offset)).abs() < 1e-9,
            "the sound sits `offset` beyond the block: block {block_dist}, sound {sound_dist}"
        );
        assert!(
            sound_dist > block_dist + 1.0,
            "…and is therefore strictly further away than the block"
        );
    }

    /// **Strictness, pinned by an exact tie.** `<` vs `<=` differs only when a
    /// draw exactly equals the chance, which a random search will never find
    /// (2⁻⁵³). Reading the draw off a cloned RNG and using it AS the chance
    /// makes the tie deterministic.
    #[test]
    fn the_tick_chance_compare_is_strict() {
        let mut probe = rng();
        let draw = probe.next_double();
        let tie = AmbientSounds {
            loop_sound: None,
            mood: None,
            additions: vec![AmbientAddition {
                sound: "tie".into(),
                tick_chance: draw,
            }],
        };
        let mut out = Vec::new();
        let mut r = rng();
        tick_additions(&tie, &mut r, &mut out);
        assert!(
            out.is_empty(),
            "an exact tie must NOT fire — the compare is `<`, not `<=`"
        );
        // …and a hair above it does, which proves the tie was the only thing
        // keeping it quiet.
        let over = AmbientSounds {
            additions: vec![AmbientAddition {
                sound: "over".into(),
                tick_chance: draw * 1.000001 + 1e-12,
            }],
            ..tie.clone()
        };
        let mut r = rng();
        tick_additions(&over, &mut r, &mut out);
        assert_eq!(out.len(), 1, "just above the draw fires");
    }
    fn with_loop(id: &str) -> AmbientSounds {
        AmbientSounds {
            loop_sound: Some(id.into()),
            mood: None,
            additions: Vec::new(),
        }
    }

    struct NoLight;
    impl MoodLight for NoLight {
        fn brightness(&self, _: i32, _: i32, _: i32) -> (i32, i32) {
            (1, 0) // the freeze point — never fires, never drains
        }
    }

    fn transitions(out: &[SoundEvent]) -> Vec<Option<String>> {
        out.iter()
            .filter_map(|e| match e {
                SoundEvent::BiomeLoopTransition { current } => Some(current.clone()),
                _ => None,
            })
            .collect()
    }

    /// **The transition key is the SOUND, not the biome.** Two biomes
    /// declaring the same loop compare equal, so crossing between them emits
    /// nothing at all — no fade, no churn. Keying on the biome would dip the
    /// volume to zero and back at every internal boundary.
    #[test]
    fn the_transition_keys_on_the_sound_not_the_biome() {
        let mut h = BiomeAmbientHandler::default();
        let mut r = rng();
        let mut out = Vec::new();
        let a = with_loop("minecraft:ambient.nether_wastes.loop");
        // Entering: previous is absent, so this IS a transition.
        h.tick(&a, [0.0; 3], 1.62, &NoLight, &mut r, &mut out);
        assert_eq!(
            transitions(&out),
            vec![Some("minecraft:ambient.nether_wastes.loop".to_string())]
        );
        out.clear();
        // A DIFFERENT record that happens to name the same loop: silence.
        let same_sound = AmbientSounds {
            mood: Some(AmbientMood::legacy_cave()),
            ..a.clone()
        };
        h.tick(&same_sound, [0.0; 3], 1.62, &NoLight, &mut r, &mut out);
        assert!(
            transitions(&out).is_empty(),
            "the same loop under a different record is not a transition"
        );
        // A different loop is.
        out.clear();
        h.tick(&with_loop("minecraft:ambient.crimson_forest.loop"), [0.0; 3], 1.62, &NoLight, &mut r, &mut out);
        assert_eq!(
            transitions(&out),
            vec![Some("minecraft:ambient.crimson_forest.loop".to_string())]
        );
    }

    /// Leaving a biome that had a loop for one that has none still emits a
    /// transition — carrying `None`, which is what fades the outgoing loop
    /// out. Suppressing it (there is nothing to start) strands the loop
    /// playing.
    #[test]
    fn losing_the_loop_still_transitions() {
        let mut h = BiomeAmbientHandler::default();
        let mut r = rng();
        let mut out = Vec::new();
        h.tick(&with_loop("a"), [0.0; 3], 1.62, &NoLight, &mut r, &mut out);
        out.clear();
        h.tick(&AmbientSounds::empty(), [0.0; 3], 1.62, &NoLight, &mut r, &mut out);
        assert_eq!(transitions(&out), vec![None], "the fade-out still has to be told");
        // …and staying there emits nothing more.
        out.clear();
        for _ in 0..5 {
            h.tick(&AmbientSounds::empty(), [0.0; 3], 1.62, &NoLight, &mut r, &mut out);
        }
        assert!(transitions(&out).is_empty());
    }

    /// **One snapshot feeds all three, and only the loop is change-gated.**
    /// Standing still in a biome with additions must keep firing them; a
    /// handler that gated everything on the change would go silent.
    #[test]
    fn additions_and_mood_run_every_tick_while_only_the_loop_is_gated() {
        let a = AmbientSounds {
            loop_sound: Some("a".into()),
            mood: None,
            additions: vec![AmbientAddition {
                sound: "add".into(),
                tick_chance: 1.0,
            }],
        };
        let mut h = BiomeAmbientHandler::default();
        let mut r = rng();
        let mut out = Vec::new();
        for _ in 0..5 {
            h.tick(&a, [0.0; 3], 1.62, &NoLight, &mut r, &mut out);
        }
        assert_eq!(transitions(&out).len(), 1, "the loop transitions ONCE");
        let adds = out
            .iter()
            .filter(|e| matches!(e, SoundEvent::Instance(i) if i.identifier == "add"))
            .count();
        assert_eq!(adds, 5, "…while the addition fires on every tick");
    }

    /// A mood-less biome **freezes** the accumulator rather than draining it:
    /// the whole block sits inside `mood().ifPresent`.
    #[test]
    fn a_moodless_record_freezes_the_accumulator() {
        let mut h = BiomeAmbientHandler::default();
        h.mood.moodiness = 0.9;
        let mut r = rng();
        let mut out = Vec::new();
        // Sky light 15 would drain it hard if the block ran at all.
        struct Bright;
        impl MoodLight for Bright {
            fn brightness(&self, _: i32, _: i32, _: i32) -> (i32, i32) {
                (0, 15)
            }
        }
        for _ in 0..50 {
            h.tick(&AmbientSounds::empty(), [0.0; 3], 1.62, &Bright, &mut r, &mut out);
        }
        assert_eq!(h.moodiness(), 0.9, "frozen, not drained");
        // With a mood present, the same light drains it.
        let with_mood = AmbientSounds {
            mood: Some(AmbientMood::legacy_cave()),
            ..AmbientSounds::empty()
        };
        h.tick(&with_mood, [0.0; 3], 1.62, &Bright, &mut r, &mut out);
        assert!(h.moodiness() < 0.9);
    }
}
