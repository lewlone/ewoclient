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

**At `SEGMENTS = 1` with infinite stiffness, the wavy cape must reproduce the
vanilla cape exactly.** Vanilla stays the default; wavy is opt-in.

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
| `SEGMENTS` | `12` | enough to read as cloth; `16/12` keeps slab height rational |
| `GRAVITY` | `0.008` | model units per tick², downward |
| `DAMPING` | `0.92` | velocity retained per tick |
| `RELAX_PASSES` | `4` | Gauss–Seidel distance-constraint iterations |
| `REST_LEN` | `16.0 / SEGMENTS` | equal to the slab height, so rest state is straight |
| `TORSO_RADIUS` | `2.5` | push-out cylinder about the body's vertical axis |
| `MAX_JOINT_RADIUS` | `24.0` | divergence backstop; a joint beyond this is clamped |
| tick rate | **20 Hz**, fixed | the session tick, never the frame rate |

### The rules

1. **Verlet, fixed step.** `x' = x + (x − x_prev) · DAMPING + a · dt²`, then
   `RELAX_PASSES` of distance-constraint relaxation, then the torso push-out,
   then the radius clamp. Same order every tick.
2. **Forcing reuses vanilla's already-gated inputs.** The acceleration is
   gravity plus the lagging-anchor delta vector `(deltaX, deltaY, deltaZ)` the
   vanilla cape already computes. **Nothing new is invented to make it move** —
   which keeps the wave anchored to behaviour that is separately verified.
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

---

## What the gate asserts

`rewo capeshot --check`, sharing the vanilla cape's gate. Not "does it look
like cloth" — **does it reduce, conserve, and stay bounded?** Each witness needs
a mutation partner.

| witness | property | mutation that must fail it |
|---|---|---|
| **reduction** | `SEGMENTS = 1`, infinite stiffness → vertices equal the vanilla cape's to 1e-5 | any nonzero gravity |
| settling | zero motion settles, and the settled state is idempotent to 1e-6 | `DAMPING = 1.0` → perpetual oscillation |
| constraints | every link within 1e-4 of `REST_LEN` after the relax passes | `RELAX_PASSES = 0` |
| determinism | two runs, identical inputs → bit-identical joint positions | seed anything from the wall clock |
| pinning | joint 0 equals the vanilla attachment point every tick | let joint 0 simulate |
| push-out | after a scripted 180° turn, no joint inside the torso AABB | disable the push-out |
| the constants | every simulation constant equals **this table's** value | change one in code only |
| stability | 600 adversarial ticks — teleports, the >10-block snap, fixed pseudo-random motion — no NaN, no joint beyond `MAX_JOINT_RADIUS` | drop the snap handling → the chain explodes |
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
