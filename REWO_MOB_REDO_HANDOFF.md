# REWO — Mob Model Redo: ✅ COMPLETED 2026-07-22

**This brief is done.** The redo it specified shipped in full; this file is
kept as a record. The living documentation is:

- **The port itself**: [`crates/rewo-gpu/src/mobs.rs`](crates/rewo-gpu/src/mobs.rs)
  — a verbatim port of vanilla `ModelPart.Cube`/`Polygon` (its module doc
  carries the coordinate contract + the ground-truth face/UV table this
  handoff's §3 used to hold), plus all 21 mob meshes transcribed from the
  26.2 decompile.
- **The verification gate**: `rewo mobshot --check` — the face-labeled
  debug-texture pass §6 demanded, fully automated (facelabel textures +
  perspective ray-cast prediction, occlusion-exact; 63/63 mob-views green).
  `rewo mobshot --out sheet.png` renders the real-texture contact sheet.
- **The record**: `REWO_PLAN.md` §15, entry
  *"2026-07-22 — the mob redo shipped"* — what was built, the 21-mob roster,
  the gates that ran (unit tests pinning vanilla UV corners, demo PNG
  byte-identity, bench flat, 0 VUIDs, live summon shots).

Outcome highlights, for anyone reading the history:

1. `box_uv_faces` and all builders on it were **deleted**, exactly as §2
   prescribed — no incremental patching.
2. Beyond the three UV bugs this brief diagnosed, the faithful transform
   port surfaced a fourth: the old `(−x, 24−y, −z)` model→world conversion
   had the **X sign wrong** (vanilla composes to `(mx, 24.016−my, −mz)` at
   yaw 0), so every mob was additionally left/right-mirrored.
3. The 26.2 **cow is not the generic quadruped** — it has its own mesh
   (8×8×6 head + muzzle + horns, 12×18×10 body + udder, legs at ±4). The
   first pass's "cow" was a pre-1.21.2 body plan against a 26.2 texture, so
   even a correct unwrap would have misaligned it.
4. The mob set grew from 6 to **21** while rolling everything onto the new
   builder (husk, drowned, skeleton, stray, wither skeleton, creeper,
   spider, cave spider, enderman, chicken, wolf, squid, glow squid, rabbit,
   villager joined player/zombie/slime/cow/pig/sheep).
5. The lesson this file existed to encode — **verify the property, not a
   proxy** — is now enforced by machinery: `rewo mobshot --check` is listed
   in REWO_PLAN §0.0's verification toolkit and must run after any mob/UV
   change.
