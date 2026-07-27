# M35 — particles, and the verification approach they needed first

*Written 2026-07-27. This file is the milestone record for M35, in the prose
depth REWO_PLAN §15 entries use. It lives at the repo root rather than in
REWO_PLAN.md because three sessions were working in parallel and all three
appending to §15 would conflict on every commit; fold it into §15 at
integration.*

---

## Why this milestone had a precondition

REWO_PLAN §16 listed particles under "deliberately not next", with a reason
rather than a shrug:

> **particles** (every gate here is geometry-based; it needs a verification
> approach invented first — don't pick it casually)

That is the right worry. Every gate in this project works the same way: put the
renderer in a known state, then assert a *value* — a pixel, a transform, a
count — against a number derived independently. `mobshot` ray-casts the same
geometry it rendered. `portalshot` computes the frame arithmetically. `light-check`
diffs 884,736 cells against the server's own engine.

Particles resist that shape. They are spawned in bulk by a random process and
then integrated over time, so there is no single frame whose contents are a
stated fact. "Render some smoke and see if it looks like smoke" is exactly the
proxy the [[feedback_verify_property_not_proxy]] lesson was written about — the
Rewo mobs once shipped shape-right and UV-scrambled with "verified" attached.

So the first deliverable here was not code. It was an argument.

## The argument

**Vanilla's particle system is not actually stochastic. It is a deterministic
function of a seed.** Three facts, read out of
`%APPDATA%/EwoClient/rewo/26.2/decompiled/net/minecraft/client/particle/`, in
increasing order of usefulness:

1. **`Particle.tick()` contains no randomness at all.** It is pure `f64`
   arithmetic over `(pos, vel, gravity, friction, on_ground, age, lifetime)`.
   Given an initial state, the whole trajectory is a fixed sequence of numbers.

   This is *not* universally true of subclasses, and the exception is modelled
   rather than rounded away: `WaterDropParticle` overrides `tick` entirely — it
   counts the lifetime **down** rather than the age up, applies gravity
   undivided where the base applies `0.04 * gravity`, uses a hard-coded `0.98`
   friction, and calls `nextFloat()` when it lands. Splash is built on it.

2. **Every generator is a `LegacyRandomSource`**, which is bit-for-bit
   `java.util.Random`'s 48-bit LCG (multiplier 25214903917, increment 11), and
   ports to Rust exactly. The project had already done this once —
   `rewo-gpu/src/mobs.rs` ports it for the ghast's seeded tentacle lengths — but
   that copy is private to its module and has only `next_int`.

3. **Therefore a seeded particle system is exactly predictable.** Spawn offset,
   velocity, lifetime, colour, quad size and sprite index all become assertable
   numbers.

The gate that falls out of this does not ask "does this look like fire". It
asks whether particle *k* at tick *n* is at exactly this position — the same
kind of claim every other gate here makes. "Stochastic" stops being an obstacle
the moment the seed is an input rather than an accident.

## Two anchors, so the argument is not circular

A seeded simulation graded against my own expectations proves nothing. Two
independent anchors hold it down, and they retire *different* failure modes.

**Anchor 1 — a real JVM, for the generator.** Minecraft's `BitRandomSource`
reimplements `next(bits)`, `nextInt(bound)` and `nextFloat()` with formulas
identical to the JDK's. So `java.util.Random`'s own output is genuinely
independent ground truth for those three — not another transcription of mine.
Known-answer vectors for six seeds are embedded in `particles.rs` and asserted
**bit-for-bit**.

**Anchor 2 — vanilla's own source text, for the physics.** This is the one that
matters. A Java harness (`Oracle.java`) whose class bodies are **copied
verbatim** from the decompile emits per-tick trajectory vectors, checked in at
`crates/rewo-world/src/particles_oracle_26_2.txt`. The KAT vectors prove the
LCG is right; only vanilla's own statements, compiled and run, can prove I read
the *constructors* right. A second implementation written from the same
misreading would have agreed with the first.

The harness runs in an **empty world**, so `collideBoundingBox` is the identity
and no collision code executes on either side — that isolates the constructor
and tick arithmetic, which is what these vectors grade. Collision gets its own
witness with a hand-computed stop position.

### It caught a bug on the first run

Four of six kinds failed immediately, all on `yd`, all with a low-bits-only
mismatch. The cause:

```rust
p.yd = p.yd / dd * speed * w(0.4) + 0.1;   // wrong
p.yd = p.yd / dd * speed * w(0.4) + w(0.1); // right
```

Vanilla writes `+ 0.1F`. Widened to a double, that float is
`0.10000000149011612`, not `0.1`. The error is ~1.5e-9 — invisible in any
screenshot, invisible in any "spawn some and look" check — and it shifted every
subsequent tick of every particle built on the 6-argument base constructor.

Note *which* tests caught it. The RNG KATs passed; the LCG was fine. `splash`
passed, because it overwrites `yd` outright a few lines later. `poof` passed,
because it uses the 4-argument base and never runs that line. Only the four
kinds routing through the 6-argument base failed. This is a good advertisement
for the oracle: no plausible visual check finds this, and no test written from
my own understanding of the code would either.

The module now routes every float literal through a `w(v: f32) -> f64` helper
so the widening is visible in the source, and the fixed line carries a comment
naming the bug.

## Where bit-exactness is not available — and why that is correct

`nextGaussian` (the Marsaglia polar method, which drives `level_particles`'
spawn scatter) evaluates `sqrt(-2 * log(r²) / r²)`.

- `sqrt` is IEEE-754 correctly-rounded. Measured 0 ULP divergence over 2M
  samples, as the standard requires.
- `log` is **not**. Vanilla calls `Math.log`, which the JLS specifies only to
  within 1 ULP and which HotSpot implements as an intrinsic.

Measured on Temurin 25:

| comparison | disagreement rate | worst |
|---|---|---|
| `Math.log` vs `StrictMath.log`, inputs in (0,1) | ~7% | 1 ULP |
| MC `nextGaussian` vs `java.util.Random.nextGaussian` | ~3% (1560/48000) | 3 ULP |
| Rust `f64::ln` vs JVM `Math.log`, 30,000-draw sweep | 0.073% (22 draws) | **2 ULP** |

The middle row is the interesting one. `java.util.Random.nextGaussian` uses
`StrictMath.log`; Minecraft's uses `Math.log`. **Vanilla's particle spawn
scatter is therefore not bit-reproducible even between two JVMs.** A gate
demanding bit-equality there would assert something *stronger than vanilla
itself guarantees*, and would be over-fitted to one JIT build.

So the gaussian is graded to a stated ULP bound (8 — a 4× margin over the
measured worst case) and everything else is graded to the bit. The tolerance is
scoped to exactly one primitive and justified, not a blanket "close enough".
For scale: 3 ULP at a gaussian magnitude of ~1 is ~7e-16 blocks, about ten
orders of magnitude below one pixel.

### A wrong theory, tested rather than shipped

MC declares `DOUBLE_MULTIPLIER` as the **float** literal `1.110223E-16F` where
the JDK uses `0x1.0p-53`. That looks like a real divergence, and I wrote it up
as one before checking.

It is not. 2⁻⁵³ is a power of two and therefore exactly representable as an
`f32`, so `1.110223E-16F` rounds to precisely 2⁻⁵³ and the widened value is
bit-identical to the JDK's. Confirmed on the JVM (`nextDouble` matches for every
tested seed) and pinned by
`double_multiplier_is_exactly_two_pow_minus_53`. Written in Rust as the same
float-then-widen so the provenance stays legible.

## The one deliberate divergence

Vanilla constructs each particle's generator with `RandomSource.create()` →
`RandomSupport.generateUniqueSeed()`, a nanotime-and-counter mix. Those seeds
are *arbitrary*: no particular value is more correct than another.

Rewo derives each particle's seed from a system-level master generator instead,
so a run is reproducible. This is **not an approximation** of vanilla's
behaviour — it draws from the same distribution, and any seed is an equally
valid vanilla outcome. It picks a *nameable* one, which is what makes the gate
exist at all.

## What shipped

### `crates/rewo-world/src/particles.rs` — the simulation

- `LegacyRandom`: the LCG plus `MarsagliaPolarGaussian`, with
  `next`/`next_int`/`next_float`/`next_double`/`next_long`/`next_gaussian`.
- `Particle`: the base tick, `move` with axis-separated collision, and six
  transcribed types — **Terrain** (block-break shards), **Smoke**, **Flame**,
  **Splash**, **Crit**, **Poof**. Each constructor annotates its RNG draw
  *count*, because draw order is load-bearing: an extra or missing draw does not
  change one field, it shifts everything after it.
- `ParticleSystem`: the pool, `handleParticleEvent`'s fan-out, and
  `addDestroyBlockEffect`'s shard grid.

Collision takes the same `shapes: &dyn Fn(i32,i32,i32) -> &[[f32;6]]` closure
`rewo_world::physics` uses, so it is testable against a synthetic world with no
`World` dependency.

### `crates/rewo-data/src/particle_types.rs` — the registry

`registries.json` → particle-type names, mirroring `entity_types.rs`. Resolving
by name means a version bump that renumbers the registry fails loud rather than
quietly spawning flames where the server asked for smoke. `block_id` is called
out separately because `minecraft:block` is the one supported kind whose options
carry a payload.

### `crates/rewo-net` — the wire

`route_level_particles` / `route_level_event`, ids resolved by name at the end
of `Ids`, one dispatch branch, and a `particle_events` queue on `PlaySession`
the renderer drains.

### `crates/rewo-gpu/src/particles.rs` + `shaders/particle.{vert,frag}`

Camera-facing billboards. Vanilla renders particles through
`core/particle.vsh/fsh` — *the same shader its weather uses* — so this pass is
deliberately close to `weather.rs`.

Billboarding is `FacingCameraMode.LOOKAT_XYZ`. Vanilla sets the quad's rotation
to the camera's rotation quaternion; Rewo takes the right/up basis out of the
view matrix, since for a rotation matrix the **rows** are the world axes. Same
transform, one representation instead of two that could drift.

Positions are emitted in **world space**, not camera-relative — the M33 weather
trap, recorded in the module header so the next pass does not rediscover it.

## The gate: `rewo particleshot --check`

Serverless, CPU-only, fail-closed on both a failing witness and a witness that
silently stopped running. **34 witnesses, 0 failures.**

| group | what it grades |
|---|---|
| `w1`–`w10` | the wire — hand-assembled packet bodies through the production decoders |
| `f1`–`f13` | the fan-out — spawn loop, shard grid, scatter, sprite sets |
| `s1`–`s11` | the simulation — seeded trajectories vs the verbatim-source oracle |

Bodies are **hand-assembled from the decompiled write methods**, not produced by
a Rewo encoder: if the same code wrote and read the packet, a transposed field
would round-trip happily.

Witnesses worth naming:

- **`w5`** — `count` is a plain big-endian `i32`, *not* a VarInt. A VarInt read
  consumes one byte and then misreads the particle type after it.
- **`w8`** — all 47 truncated prefixes decode to `None` without panicking.
- **`f4`** — the `count == 0` inversion. With a zero count the three `*_dist`
  fields stop being a scatter radius and become a **direction**, and `max_speed`
  stops being a spread and becomes that direction's **magnitude**. One particle
  spawns. This is the single most misreadable thing in the packet.
- **`f1`/`f2`** — a seeded system is byte-identical across runs, *and* a
  different seed actually differs. The second exists because the first would
  also pass on a constant.
- **`s9`** — `FlameParticle.move` bypasses collision **by design**, so a "fix"
  routing it through the collider would be the regression.
- **`s10`** — `CritParticle` calls `this.tick()` inside its own constructor, so
  a crit is already age 1 with one colour-decay applied before its first frame.

### Mutation-tested

A witness that has never failed has not been shown to work. Four mutations, all
caught, all with legible signatures:

| mutation | result |
|---|---|
| restore the `+ 0.1F` float-widening bug | 4 trajectory witnesses fail — and `splash`/`poof` correctly still pass, since neither runs that line |
| read `count` as a VarInt | 9 fail, starting at `w5` |
| transpose `x_dist`/`y_dist` in the decoder | `w4` and `f4` fail |
| coarsen destroy-block density 0.25 → 0.5 | `f7`/`f8`/`f10` fail (8 shards, not 64) |
| declare `Splash` single-frame again | `f12` fails |

## A second defect, found by reading the authoritative data

After the gate was green, checking the jar's own
`assets/minecraft/particles/*.json` — rather than inferring sprite sets from
the texture directory — turned up a real bug in what had already shipped.

`splash.json` lists **four** textures (`splash_0..3`), and
`SplashParticle.Provider` calls `this.sprite.get(random)`. I had declared
Splash single-frame, so `spawn_one` never made that draw.

The trajectory witnesses still passed, because they grade the *particle's own*
generator and the test hands it in directly. What was wrong was the **engine**
stream position. Vanilla's providers split cleanly:

| kind | sprite source | engine draw |
|---|---|---|
| Flame, Crit | `sprite.get(random)`, 1 texture | **yes** — `nextInt(1)` still consumes one |
| Splash | `sprite.get(random)`, 4 textures | **yes** |
| Smoke, Poof | `sprites.first()`, animated by age | no |
| Terrain | the block model | no |

The pick happens as a constructor *argument*, so it lands before any of the
particle's own draws — get it wrong and every later particle in a burst shifts.
The trap is that `nextInt(1)` looks like a no-op worth skipping; it is not,
because it still advances the LCG.

Fixed, and pinned by two new witnesses: `f12` grades the frame counts and the
`get(random)`-vs-`get(age, lifetime)` split against the JSONs, and `f13` proves
the draw is real by showing that an engine stream advanced by one lands the
same two particles somewhere else. Mutating `Splash` back to one frame fails
`f12`.

The lesson generalises: the texture directory is a *proxy* for the sprite set,
and the particle JSON is the property.

## Honest scope limits

Stated so they read as decisions rather than oversights.

- **Not wired into `rewo live` yet.** The simulation, the wire decode, the
  Vulkan pass and its `WorldRenderer` hooks (`init_particles` / `set_particles`
  / the draw call between the translucent and weather passes) all exist and
  compile. What is missing is the **particle atlas bake** — vanilla's particle
  sprites live as individual PNGs under `textures/particle/`, and terrain shards
  sample the *block* atlas rather than the particle one, which is a second
  sampling path. That is the next step, not a hidden gap: nothing currently
  calls `init_particles`, so the pass is inert and no frame changes.
- **Six kinds, not 125.** Block, smoke, flame, splash, crit, poof. Unsupported
  types are dropped rather than guessed at (`w7`), which is safe because packets
  are length-framed — abandoning a body part-way never disturbs the stream.
  Most of the other 119 carry option payloads whose codecs are untranscribed.
- **No translucent sorting.** Particles blend, depth-test, and do not
  depth-write. Vanilla sorts its translucent particle layers.
- **Splash's fluid-height removal is half-implemented.** Vanilla removes a water
  drop when it sinks below the max of the block's collision height *and its
  fluid height*; Rewo has no fluid-height query on the collision seam, so only
  the collision half applies. A splash over still water lives marginally longer
  than vanilla's. Commented at the call site.
- **Terrain shards use a flat 0.6 grey.** Vanilla multiplies it by the block's
  tint source. The render pass already owns the biome colormap, so this belongs
  there when the atlas lands.
- **Rendering is not graded by the gate.** A particle quad is a camera-facing
  billboard whose geometry the existing passes cover; what was novel and
  unverified about particles was the simulation. The pass has 4 CPU-side unit
  tests (basis extraction, quad centring/sizing, light-word packing) but no
  pixel oracle.

## Verification run

- `cargo test` on all six rewo crates: **524 lib tests**, 0 failures (was 500;
  +20 in `rewo-world`, +4 in `rewo-gpu`).
- `rewo particleshot --check`: **34/34**, exit 0.
- Full gate sweep — mobshot 243/243, blockentityshot 172, swingshot 97,
  hurtshot 38, weathershot 35, particleshot 34, eventshot 28, itemshot 28,
  danceshot 24, portalshot 12, plus skyshot, lightmapshot, tintshot, meshshot,
  dimensioncheck: **all 15 exit 0 with 0 VUIDs**.
- Canonical demo PNG SHA-256 unchanged:
  `2cc56b4acbfb92cb91398c27e5c4735885abff9331f66b7dc83bdbc002246635`.
- `git diff --check` clean.

## A note on the branch

This work was based on `36fd402` (the M33b tip). Partway through, another
session took the branch name `codex/rewo-m35-particles` for the
inventory-decode workstream, which evicted this worktree back to an M11-era
commit; the work was transplanted onto a correctly-based branch named
`codex/rewo-m35-particles-sim`. The two commits cherry-picked cleanly apart from
"add my field alongside theirs" conflicts in `rewo-data/src/lib.rs` and
`rewo-net/src/lib.rs`.

Repairing that also re-triggered §0.0 **gotcha 9** — `rewo-data/src/lib.rs` is
one of the mixed-CRLF files, and a scripted rewrite normalised all 95 CRLF
terminators, turning a 3-line change into a 195-line diff. Repaired with the
documented procedure (realign against the base, re-emit its exact bytes for
equal lines, LF for new ones). Worth restating: **that file is named in gotcha 9
and it still caught me**, because the trap is not the editing, it is the
`open(p).read()` that silently universal-newlines on the way in.
