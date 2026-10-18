# Autonomous run — 2026-10-18 to 2026-10-20

An autonomous stretch while the owner was away (phone only), started by the
Control Room session alongside five other projects on the same PC. Branch
`claude/autonomous-run-1020-82937b`, fast-forwarded to the local `main` at
`e09f02f` (the end of the 2026-10-15 overnight push) with M194's unfinished
branch `claude/item-frames` merged in. Nothing was pushed and nothing was
released.

## For Enni (short)

*(Kept current through the run; the last update is the one that counts.)*

- **M194, item frames, is done**: frames draw their wood model, the item in
  them (turned in eighths) and a map in them (turned in quarters, with only
  the markers a frame shows). New offscreen gate `itemframeshot`.
- **M195**: the 25 animated block textures vanilla blends (magma, prismarine,
  sculk, command blocks, campfires...) now blend instead of cutting. New
  gate `animshot`.
- **M196 / M197**: water and lava sides use the flowing texture, and a
  flowing surface uses the flowing texture turned to the direction the
  fluid runs (rivers, waterfalls' edges). This changes the demo picture on
  purpose, in exactly the three flowing cells of its pool.
- Nothing needs a decision from you yet.

## What changed

| Commit | What |
|---|---|
| `8b3cdb5` (merge) | `claude/item-frames` (M194 slices 1-2 and its plan) merged onto `main`'s tip. |
| `a7be406` | **M194** slices 3-6: the FIXED display transform on every item model, the four frame block models, the framed-map band in the entity atlas, the collector and draw builder (`live_cmd/item_frames.rs`), no capsule for frames, and the `itemframeshot` gate (21 witnesses). |
| `185fcf6` | render-check **r69** (M192's held map, injected into the off-hand), **r70** (an injected frame), **r71** (a frame the real server summons: metadata slots 8-10 calibrated against 26.2); `capeshot`'s two atlas-layout rows updated for the new band; §15 entry. |
| `e8dbae7` | §0.0 numbers after M194. |
| `cf38cbe` | **M195** — animated textures interpolate (`render-misc [INTERP]`): the bake reads `.mcmeta` `interpolate`, `anim_tick` blends each frame into the next per mip on every tick (vanilla's `animate_sprite_interpolate.fsh` on bytes). New gate `animshot`, which reads the uploaded layer back from the GPU. |
| `807cf47` | **M196** — fluid sides draw the top-left quadrant of the 32 px flow sprite (all a side samples), as an animated 16 px layer interned after the main bake so no layer index moves. |
| `8d3ce51`, *(next)* | **M197** — flowing fluid tops: a 32 px flow texture array on terrain binding 2, picked by the layer's high bit; the mesher reuses physics' `getFlow` for the angle; demo hash rebaselined. |

## Decisions made

1. **The frame model and the framed item ride the block-entity emitter**
   rather than a new draw path: both are static quads at a pose, which is
   what `BlockEntityDraw` already is. The item's FIXED transform is applied
   by the emitter from the model it draws (`fixed_display`), the way the
   dropped-item path applies `ground`, so the app never needs the transform.
2. **A framed map gets its own band in the entity atlas** (128 rows at the
   bottom, M48's recipe, every earlier address unchanged) with seven LRU map
   slots. Past seven framed maps on screen at once the rest draw no pixels; a
   recorded deviation, since vanilla's texture manager has no cap.
3. **Recorded M194 deviations**: no enchantment glint on a framed item; the
   glow frame's map light is taken at block level 13 rather than 13.125; no
   name tag for a framed item with a custom name.
4. **r69 puts the map in the OFF-hand**, so the main-hand witnesses keep their
   sword; the map is then drawn one-handed.
5. **r71 adds one staging command** to `tools/render_check.py`: a `Fixed`
   item frame summoned on the test server. It is the only way to calibrate
   the frame's polymorphic metadata slots against a real server rather than
   the decompile's slot count.
6. **Windowed runs are allowed this time** (the 10-15 run skipped them for a
   recording batch). A note in the shared notes folder tells the other
   sessions when Rewo's GPU checks run.
7. **Work stays on this branch; local `main` is untouched.** The 10-15 run
   merged into `main`; this time `main` is checked out in the main worktree
   and the brief said to merge into this branch. To take the run: `git merge
   --no-ff claude/autonomous-run-1020-82937b` on `main`.
8. **M195's clock**: vanilla keeps per-sprite counters from load; Rewo
   derives them from the session tick (`subFrame = tick % frametime`). Every
   interpolating block texture has one uniform frametime, so the blend is the
   same; only the phase differs, which nothing can see.
9. **M197, the decision the plan had left open**: the flowing top samples
   about 23 px of a 32 px sprite, which a 16 px layer cannot hold. Chosen: a
   separate 32 px texture array for the flow sprites, picked by the layer's
   high bit, sampled in a branch on a flat input. Rejected: halving the
   sprite (half the texel density on every flowing surface) and raising the
   whole block array to 32 px (every consumer of `TEX_SIZE` changes, the
   held-item bake among them). Reversible: the bit, the binding and the bake's
   flow layers are the whole change.
10. **`getFlow` exists once**: the mesher calls physics' `fluid_flow` through
   an adapter rather than transcribing it again; only `Mth.atan2` had to be
   ported a second time (rewo-mesh cannot see rewo-gpu), with a test that
   holds the two ports to the bit.
11. **The demo hash moved on purpose** (`d7dc9e89...` to `748e5a1e...`); the
   pixel diff against the old binary is confined to the three cells that
   flow.

## Verification

- `cargo test --workspace --no-fail-fast --features rewo-app/gates`:
  4,253 passed, 0 failed (after slices 3-6; baseline at `e09f02f` was 4,244).
- `python tools/gates.py`: 49/50 after slices 3-6, the one failure being
  `capeshot`'s atlas-layout rows, which encoded the old bottom band; updated
  in `185fcf6`. At `185fcf6` (`tools/numbers.py`): **4,253 tests passed,
  0 failed; gates 50/50** (demo hash unchanged, `d7dc9e8985f0504f`); clippy
  `-D warnings` and `cargo fmt --check` clean.
- `python tools/render_check.py` (a windowed run against a fresh local
  vanilla server): **71/71**, including r68 (the 10-15 recipe-book button,
  never run before), r69, r70 and r71.

## Needs Enni

- Nothing blocking. Still open from before (listening pass, the release
  zip on a fresh machine, the HUD's Java side in a running Minecraft): see
  `docs/OVERNIGHT-2026-10-15.md` "Needs the owner".

## Follow-ups found, not done

- Foil on framed items (the block-entity emitter has no glint sink).
- Banner name labels on maps (held and framed).
