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
- Nothing needs a decision from you yet.

## What changed

| Commit | What |
|---|---|
| `8b3cdb5` (merge) | `claude/item-frames` (M194 slices 1-2 and its plan) merged onto `main`'s tip. |
| `a7be406` | **M194** slices 3-6: the FIXED display transform on every item model, the four frame block models, the framed-map band in the entity atlas, the collector and draw builder (`live_cmd/item_frames.rs`), no capsule for frames, and the `itemframeshot` gate (21 witnesses). |
| `185fcf6` | render-check **r69** (M192's held map, injected into the off-hand), **r70** (an injected frame), **r71** (a frame the real server summons: metadata slots 8-10 calibrated against 26.2); `capeshot`'s two atlas-layout rows updated for the new band; §15 entry. |

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
