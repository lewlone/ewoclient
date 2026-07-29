# Rewo wavy capes — the specification

**Status: approved design, not yet implemented.** Second Rewo feature with no
vanilla behaviour to transcribe, after `REWO_HEALTH_BAR_SPEC.md`. Read that
file's preamble first; the reasoning is the same and is not repeated here.

---

## Why this file exists, and why it is not a port

The requested feature is a cape that behaves like cloth rather than a rigid
board — the behaviour popularised by the Fabric mod **Wavey Capes**.

**That mod's source may not be consulted.** `REWO_FEATURE_SURVEY.md` §2 is
binding: Rewo reimplements rather than bundles, so a reference mod's licence
governs whether its source may be read at all. Wavey Capes is
`LicenseRef-tr7zw-Protective-License` — verified against Modrinth's API and the
repository's own LICENSE — which is:

1. the **exact LicenseRef** §2 already lists as reference-unsafe for
   EntityCulling, by the same author; and
2. **NonCommercial** in substance (*"may not be used to get a) a commercial
   advantage, or b) monetary compensation"*), which §2 separately rules out as
   not OSI-open because it discriminates by field of endeavour — the same
   ground on which Jade and WTHIT are excluded.

Either is sufficient on its own. **No Wavey Capes source, decompiled jar,
algorithm description or configuration documentation was read, quoted or
paraphrased in producing this design.**

That costs nothing, because **a behaviour is not protected — only its
expression is.** Simulating cloth as a constrained particle chain is textbook
and decades older than the mod: Verlet integration (Verlet, 1967), mass-spring
cloth (Provot, 1995), position-based distance constraints (Jakobsen, GDC 2001).
Everything below is derived from those plus **vanilla's own cape state**, which
Rewo may transcribe freely and which the vanilla cape milestone already gates.

§2's disclaimer applies here too: this is not legal advice.

**So the numbers below are chosen, not derived** — same as the health-bar spec,
and stated in the same place for the same reason.

---

## The split — where vanilla stops

**Below the line — the vanilla cape.** Geometry, the lagging cloak anchor
(`moveCloak`'s `d * 0.25` and its 10-block snap), `capeFlap` / `capeLean` /
`capeLean2` with their clamps, the `Rx·Rz·Ry` composition, the four suppression
gates, metadata index 16 bit 0. All transcribed, all gated normally by
`capeshot`. **This spec does not govern any of it.**

**Above the line — the wave.** Only what follows.

### The reduction rule, which is the whole safety net

**At `SEGMENTS = 1` the wavy cape must reproduce the vanilla cape exactly** —
by *bypassing* the simulation, not by stiffening it. Vanilla stays the default;
wavy is opt-in.

**Correction, before implementation.** The first draft of this rule said "with
infinite stiffness", which is **false**. Infinite stiffness fixes a link's
*length*, not its *orientation*: a two-joint chain with a rigid rod is a
pendulum, and under gravity it hangs straight down, where the vanilla cape sits
at `Rx(6 + capeLean/2 + capeFlap)`. Stiffness alone can never produce that
angle, so the reduction would have failed for a correct implementation — the
worst kind of specification bug, because the obvious fix is to break the code
until the spec passes.

The rule that is actually true, and is what the gate asserts: **at
`SEGMENTS = 1` the single segment takes its orientation from the vanilla
rotation and the simulation contributes nothing.** That makes the reduction an
identity by construction rather than an emergent coincidence, which is what a
safety net needs to be.

This is the strongest witness available for a feature with no oracle, because
it grades the new code against the **already-gated old code** rather than
against a restatement of itself. It is also what stops the vanilla milestone
regressing quietly behind a feature flag.

---

## The model

The cape becomes **N stacked slabs**, each `10 × (16/N) × 1` model units, and
the UV band a straight subdivision of the same 64×32 box-UV the single cube
uses. There are **N+1 joints**, one per slab boundary.

Joint 0 is **pinned** to the vanilla attachment point — the position and
orientation the transcribed cape already computes. Everything below it
simulates.

### The numbers

| name | value | note |
|---|---:|---|
| `SEGMENTS` | `16` | enough to read as cloth, and `16/16` makes `REST_LEN` exactly `1.0` — an exactly-representable binary fraction, which the bit-determinism witness wants and `16/12 = 4/3` would not have given |
| `GRAVITY` | `0.008` | model units per tick², **world** down — see rule 7 |
| `ANCHOR_ACCEL` | `GRAVITY * 100 * PI/180` ≈ `0.013963` | **the anchor delta's scale, added M61.** Not a free parameter: it is what makes the chain's equilibrium tilt match vanilla's own `capeLean` response of 100°/block in the small-angle regime |
| `DAMPING` | `0.92` | velocity retained per tick |
| `RELAX_PASSES` | `4` | Gauss–Seidel distance-constraint iterations |
| `REST_LEN` | `16.0 / SEGMENTS` | equal to the slab height, so rest state is straight |
| `TORSO_RADIUS` | `2.5` | push-out cylinder about the body's vertical axis |
| `MAX_JOINT_RADIUS` | `24.0` | divergence backstop; a joint beyond this is clamped. **Unreachable while the constraints hold** — the chain's total length is `REST_LEN * SEGMENTS = 16`, so no joint can exceed 16 from the anchor unless the solver has already failed. See the stability witness: it must *construct* a divergence rather than assert an absence |
| tick rate | **20 Hz**, fixed | the session tick, never the frame rate |

### The rules

1. **Verlet, fixed step.** `x' = x + (x − x_prev) · DAMPING + a · dt²`, then
   `RELAX_PASSES` of distance-constraint relaxation, then the torso push-out,
   then the radius clamp. Same order every tick.
2. **Forcing reuses vanilla's already-gated inputs.** The acceleration is
   gravity plus `ANCHOR_ACCEL ×` the lagging-anchor delta vector
   `(deltaX, deltaY, deltaZ)` the vanilla cape already computes. **Nothing new
   is invented to make it move** — which keeps the wave anchored to behaviour
   that is separately verified.

   **Correction (M61): the scale factor was missing, and its absence was not
   cosmetic.** The chain's equilibrium tilt is `atan(|a_horizontal| / GRAVITY)`,
   so feeding the raw delta gives **80.9°** at a walking drift of 0.05 where
   vanilla's own `capeLean` gives **5°** — any motion at all pinned the cape
   near horizontal. The dimensionally-coherent alternative (×16, blocks →
   model units) is strictly worse: 14 link-lengths per tick snaps the chain
   rigidly onto the acceleration in one tick and it never waves.

   `ANCHOR_ACCEL` is derived, not tuned. Vanilla maps one block of lag to 100°
   of lean; in the small-angle regime `θ ≈ a_h / GRAVITY`, so
   `a_h = GRAVITY · 100 · π/180 · delta`. Checked: 4.99° against vanilla's 5.00
   at drift. At larger deltas the `atan` compresses where vanilla's linear
   clamp does not (34.9° vs 40° at a walk) — that divergence is the physics
   being right and vanilla being a linear approximation, and it is intended.
3. **Fixed step, fixed iteration count, no RNG.** The simulation must be
   assertable to the bit. M37 established that the same is true of vanilla's
   particles — `Particle.tick()` contains no randomness at all — and it is what
   makes a gate possible here.
4. **Render interpolates, simulation does not.** Frames lerp between the
   previous and current joint positions, exactly as `getInterpolatedCloakX`
   does. A frame must never advance the simulation.
5. **Torso push-out.** A joint inside `TORSO_RADIUS` of the body axis is pushed
   radially out to it. This is the one place a naive chain visibly fails —
   without it the cape passes through the player on a fast turn.
6. **Divergence is clamped, not tolerated.** A joint beyond `MAX_JOINT_RADIUS`
   of the anchor is clamped back. A teleport or a >10-block anchor snap must not
   be able to explode the chain.

7. **`GRAVITY` is world-down, and that choice is load-bearing (M61).** Reading
   it as the cape's *local* down would make the rest state exactly the vanilla
   drape and would be robust to the scale question above — but it destroys the
   reduction rule's mutation, because a single simulated segment would then
   settle onto the vanilla angle instead of hanging straight down, and the
   witness would pass for a bypassed *and* a simulated implementation alike. The
   strong witness wins. The price is a rest-state IoU of ~0.975 rather than
   1.000, the residue being 6° of tilt lying nearly along the view axis.

8. **Constraint solving walks from the pin, holding the upper joint (M61).**
   Symmetric Gauss–Seidel does **not** meet the 1e-4 link tolerance in
   `RELAX_PASSES` — it measures 2.9e-2, because gravity's uniform per-tick shift
   breaks link 0 against the pin every tick and four sweeps only halve the error
   four times. Holding the upper joint is exact in **one** pass (1.1e-15);
   passes 2–4 are then no-ops and are still run. Stated because a reasonable
   implementer writes the symmetric solver first and then cannot satisfy the
   tolerance.

9. **The push-out is a cylinder, so it cannot exclude the torso *box*.** The
   spec's earlier "no joint inside the torso AABB" was unachievable: the torso is
   8 wide and `TORSO_RADIUS` is 2.5, so a joint at x 3.5, z 0 is outside the
   cylinder and inside the box. The gate asserts what the rule actually creates —
   a minimum radial distance — not the stronger claim.

10. **The collision response is not re-projected.** Rule 1's order relaxes and
    *then* collides, so a joint shoved off the torso leaves its links stretched
    until the next tick. A steady walk never fires the push-out and a 30°/tick
    turn stretches **0.230** model units — a fifth of a slab — and that is the
    figure to quote. An earlier draft cited 3.44; that number was an artefact of
    the missing `ANCHOR_ACCEL` (the unscaled acceleration whipped the chain
    across the torso). With the scale correct, a cloak gap blows the cape *away*
    from the body and never fires the push-out at all — only a turn does.
    Recorded as a known consequence of the stated order rather than discovered
    later.

---

## What the gate asserts

`rewo capeshot --check`, sharing the vanilla cape's gate. Not "does it look
like cloth" — **does it reduce, conserve, and stay bounded?** Each witness needs
a mutation partner.

| witness | property | mutation that must fail it |
|---|---|---|
| **reduction** | `SEGMENTS = 1` → vertices equal the vanilla cape's **bit-for-bit**, because the segment takes the vanilla rotation and the simulation is bypassed | let the single segment simulate — it becomes a pendulum and hangs straight down instead of at `Rx(6 + capeLean/2 + capeFlap)` |
| settling | zero motion settles, and the settled state is idempotent to 1e-6 | `DAMPING = 1.0` → perpetual oscillation |
| constraints | every link within 1e-4 of `REST_LEN` after the relax passes | `RELAX_PASSES = 0` |
| determinism | two runs, identical inputs → bit-identical joint positions | seed anything from the wall clock |
| pinning | joint 0 equals the vanilla attachment point every tick | let joint 0 simulate |
| push-out | after a scripted 180° turn, no joint inside the torso AABB | disable the push-out |
| the constants | every simulation constant equals **this table's** value | change one in code only |
| stability | 600 adversarial ticks — teleports, the >10-block snap, fixed pseudo-random motion — no NaN, and every joint within the chain's own 16-unit reach | drop the snap handling → the chain explodes |
| backstop **engages** | inject a divergence the constraints cannot absorb (a single huge impulse), and assert the clamp catches it and the chain recovers to within `REST_LEN` tolerance | remove the clamp → the joint escapes and never returns. **Without this row the backstop is untestable**: `MAX_JOINT_RADIUS` is unreachable in normal operation, so "no joint beyond it" passes whether or not the clamp exists — the same vacuity the health-bar spec's upper clamp turned out to have |
| visual | silhouette differs from vanilla under motion, identical at rest | — (marker texture; the empty frame asserted to contain none of it) |

The marker-texture discipline is M38's: a detector must not be able to count
something other than its subject. That milestone hit **three** detector errors
in one go, every one of them "count non-X against a background that is also X".

Two rules carried in from §0.0, both earned:

- **Do not verify by diffing two live frames.** M50's control run differed in
  41,284 pixels against a 16,329-pixel signal; M37 retracted a frame-diff
  witness for the same reason.
- **The gate must drive the real simulation**, not a parallel copy — M45 and
  M41 both shipped gates that had quietly stopped testing their subject.

---

## Deliberately excluded

- **Wind, and any weather coupling.** More inputs, no oracle, and the wave
  reads fine without it.
- **Per-player configuration.** One set of constants, in this file.
- **Elytra interaction.** Vanilla suppresses the cape entirely when the chest
  item has a WINGS layer; that gate belongs to the vanilla milestone, and the
  elytra-wears-your-cape path (`usePlayerTexture`) is its own feature.
- **Self-collision.** A cape can pass through itself. Real cloth solvers need
  it; a 12-segment ribbon does not earn the cost.
