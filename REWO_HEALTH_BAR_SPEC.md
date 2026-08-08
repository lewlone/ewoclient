# Rewo health bars — the specification

**Status: SHIPPED as M59 (2026-07-29), and this file is now the oracle it is
graded against.** `rewo healthbarshot --check` — 33 witnesses — asserts the
render against the numbers written below, and **re-declares them rather than
importing the implementation's constants**, because importing them would assert
only that the implementation equals itself.

This file exists because the health bar is the first Rewo feature with **no
vanilla behaviour to transcribe**, and the project's entire verification method
assumes there is one. That is why the design was written down *first*, as a
decision, and is kept here unchanged: it is the specification the gate reads.

---

## Why this file exists at all

Every one of Rewo's gates works the same way: predict the answer from an
independent reading of the 26.2 decompile, then assert the render matches.
Fourteen milestones have shipped on that method.

It does not work here. **Vanilla renders no health bar over any entity.** It
shows the local player's hearts, a server-driven boss bar, and a horse's
inventory screen — and nothing else. A floating bar over a mob is purely a mod
convention.

That leaves two bad options and one good one:

- *Bad:* transcribe a mod. Reading a GPL-licensed mod's source to derive Rewo's
  design is a licensing decision this project has deliberately avoided —
  `REWO_FEATURE_SURVEY.md` §2 already records Sodium, EntityCulling and Xaero's
  as source-available-but-not-open and off-limits as reference.
- *Bad:* let the implementation be its own specification. Then the gate grades
  the code against a restatement of itself, which is exactly the failure mode
  §0.0 collects — M41's `t4` passed for months while the box was drawn in the
  wrong coordinate space, because the witness had been written against the
  implementation.
- **Good:** write the numbers down *first*, here, as a decision. Then the gate
  grades the render against this file, and this file is reviewable on its own
  terms.

**So: the numbers below are chosen, not derived.** Every other constant in Rewo
carries a decompile citation. These do not, and that is the point of putting
them somewhere a reader will not mistake them for transcription.

---

## The split — where vanilla stops

The milestone divides at the line where a vanilla oracle exists.

### Below the line — normal transcription, gate it as usual

| input | source | state |
|---|---|---|
| current health | `LivingEntity.DATA_HEALTH_ID`, metadata index 9, FLOAT | **shipped** (M24) |
| max health | `ClientboundUpdateAttributesPacket` (protocol 131) + `AttributeModifier.Operation` + `RangedAttribute`'s `[1.0, 1024.0]` clamp | **in progress** |
| when a floating label may appear at all | `AvatarRenderer`'s name-render distance culling, `isInvisible`, spectator suppression | to do |
| billboard basis, anchor, scale | `cam_right`/`cam_up`; `TAG_PX = 0.025`, `TAG_LIFT = 0.4` — both annotated in `entities.rs` as vanilla's | **shipped** |

These get ordinary witnesses with decompile citations. The bulk of the
milestone's verification effort belongs here, not on the appearance.

### Above the line — this file is the source of truth

Only the bar's own geometry and colour. Nothing below depends on a vanilla
behaviour, and nothing below should be described in a commit message as
"vanilla's".

---

## The render precedent — reuse, don't invent

A health bar needs **no new geometry type, texture, pipeline, or blend state**.

`entities.rs::push_tag` already draws the nametag's backing plate: a
camera-billboarded, arbitrarily-coloured, untextured rectangle at a fixed lift
above the entity's head, sampling a guaranteed-opaque white texel patched into
the font atlas for exactly this purpose. A bar is that, twice — a backing plate
and a fill.

The implementation is therefore a `push_health_bar` sibling to `push_tag`,
emitting into the **nametag (text) vertex range** — alpha-blended, no depth
write — not the solid range that world-space sign text uses.

---

## The numbers

All dimensions are in **font pixels**, the same unit `push_tag` works in, so the
bar scales with `TAG_PX` exactly as a nametag does.

| name | value | note |
|---|---:|---|
| `BAR_W` | `40.0` | roughly the width of a short name, so a bar and a tag read as one unit |
| `BAR_H` | `3.0` | thick enough to read at distance, thin enough not to compete with the name |
| `BAR_PAD` | `1.0` | the plate's margin around the fill, matching the nametag plate's 1px |
| `BAR_GAP` | `2.0` | vertical gap between the bar and the nametag above it |
| plate colour | `[0.0, 0.0, 0.0, 0.25]` | **identical to the nametag plate**, so the two never disagree |
| fill colour, healthy | `[0.85, 0.20, 0.20, 1.0]` | |
| fill colour, critical | `[0.95, 0.55, 0.15, 1.0]` | below `CRITICAL_FRAC` |
| `CRITICAL_FRAC` | `0.25` | |
| anchor | `pos.y + height + TAG_LIFT` | the nametag's anchor. **Disambiguated (M59):** `BAR_GAP` is measured from the **anchor**, not from the tag plate's bottom edge at −1 — the two readings differ by exactly `BAR_PAD`, and only the anchor reading is consistent with "at the anchor itself when no tag is present" |

### Rules

1. **`fraction = clamp(health / max_health, 0.0, 1.0)`.** Both clamps matter:
   absorption can push health above max, and a server that lowers max after the
   fact can leave a stale ratio above 1.
2. **The fill's width is `fraction * BAR_W`, exactly** — no rounding to whole
   pixels. At `fraction == 0` the fill is emitted with zero width or not at all.

   **Correction (M59):** the original text finished "at `fraction == 1` it is
   exactly `BAR_W`", which rule 3 makes **unreachable** — a bar at full health
   emits nothing, so `BAR_W` is a supremum the fill never attains. The strongest
   observable statement is the one the gate asserts instead: 19/20 is exactly
   38.0 px, and 20/20 emits nothing.

   The same applies to rule 1's **upper** clamp: `clamp(_, 0, 1)`'s ceiling and
   rule 3's `>= 1.0` hide the same set, so clamped and unclamped are
   indistinguishable from outside. It stays specified (defensive, and free), but
   it is not independently gradeable and the gate does not pretend otherwise.
   The **lower** clamp is observable and is a real witness — unclamped, −5/20
   draws a −10 px fill escaping the plate to the left.
3. **Hidden at full health.** `fraction >= 1.0` emits **no vertices at all** —
   not a full bar. This keeps a peaceful scene uncluttered and makes the
   "damaged" signal the presence of the bar rather than its width.
4. **Hidden when max health is unknown.** If no `update_attributes` has been
   received for the entity, emit nothing. **Do not fall back to 20.0.** A wrong
   denominator draws a confidently wrong bar, which is worse than no bar — and
   Rewo cannot distinguish "server never sent health" from "this mob has 1 HP"
   either, since the metadata default is `1.0`.
5. **Living entities only**, and suppressed by everything that suppresses a
   nametag: invisible, spectator, beyond the name-render distance.

   Two corrections from M59, both from the decompile:

   - **The name-render distance is an attribute, not a constant.**
     `Attributes.NAME_TAG_DISTANCE` is a `RangedAttribute(64.0, [0, 512])`, so
     M55's resolution machinery answers it — modifiers and clamp included. A
     hard-coded 64 is wrong for any server that scales it.
   - **"Spectator suppression" is backwards.** `isInvisibleTo` returns *false*
     for a spectating viewer — a spectator **sees** invisible entities. The real
     rule is `entity != getCameraEntity()`, which the local-player exclusion
     already covers.

   **M70 implemented this rule properly, and it is no longer a list.** The
   three gates M59 shipped (living, distance, invisible) were a strict subset
   of what suppresses a nametag, and the nametag path had a *different* subset
   — namely none at all — so "suppressed by everything that suppresses a
   nametag" was aspirational rather than true. Both features now call
   [`rewo_world::label`], and the sentence is true by construction.

   What that means concretely, and three findings that qualify the text above:

   - **The bar takes the *suppression* half of
     `LivingEntityRenderer.shouldShowName`, not the whole predicate.** The
     rules enumerated in this file — invisible, spectator, distance — are
     exactly that half, joined by the `isDiscrete()` sneak cut-off, the four
     `Team.Visibility` arms, `isVehicle()`, the camera entity and
     `hud.isHidden()`. What the bar deliberately does **not** take is
     `MobRenderer`'s extra `entity.shouldShowName() || (hasCustomName &&
     crosshairPick)` conjunct: that is a question about whether a *name*
     exists, and gating a bar on it would hide the bar from every un-named mob
     in the world.
   - **The name-render distance is the attribute, but the sneak cut-off is
     not.** `LivingEntityRenderer.shouldShowName` hides a sneaking entity at
     `distanceToCameraSq >= 1024.0` — a folded literal, **not**
     `Mth.square(nameTagDistance)` — so a server that raises the attribute
     does not move the 32-block sneak bound with it.
   - **One recorded divergence, and it is deliberate.** Vanilla's
     `ArmorStandRenderer.shouldShowName` is a *full* override that returns
     `isCustomNameVisible()` and skips every suppression rule. That override is
     about names; this file is the authority for bars, so an armour stand's bar
     does take the suppression rules. Letting an invisible armour stand keep a
     health bar because of a naming quirk would be the stranger answer.

   **Answerable as of M73 — this paragraph used to say it was not.** When this
   spec was written (M70) `crosshairPickEntity` needed an entity raycast Rewo
   did not have, so the pick clause was transcribed, graded both ways by
   `labelshot`, and fed a hard `false` live. **M73 built that raycast** — it is
   not a second, label-only cast but the same `LocalPlayer.pick` hitResult that
   decides which block you are mining — and the clause now resolves from it.
   Left as a correction rather than a rewrite, because the reasoning for
   suppressing was sound and is worth keeping: showing a custom name
   unconditionally is wrong for every non-visible named mob at all times, where
   the suppressed version was wrong only while one was under the crosshair.

---

## What the gate asserts

`rewo healthbarshot --check`. Not "does it look right" — **is it self-consistent,
and does it respond to its inputs the way this file says?** Each witness needs a
mutation partner, as everywhere else.

| witness | property | mutation that must fail it |
|---|---|---|
| arithmetic | `health 7 / max 20` → fill width is exactly `0.35 * BAR_W`, measured from emitted vertices | swap numerator and denominator; divide by the metadata default |
| monotonicity | 21 health values give non-decreasing widths, exactly `0` at 0 and exactly `BAR_W` at max | an off-by-one shows as non-zero fill at zero health |
| clamping | health above max clamps to full without overflowing the plate; below 0 clamps to empty | unclamped division |
| hidden at full | `fraction == 1.0` emits **zero** vertices | drawing a full bar |
| **fail-closed** | an entity with no attributes received emits zero vertices, distinguishably from health 1.0 | defaulting max to 20.0 |
| billboarding | four camera azimuths give the same projected width to within a pixel, tracking the head | a world-fixed basis — three of the four collapse |
| gating | invisible → 0; non-living → 0; beyond name distance → 0 | each inverse |
| colour threshold | fill is critical below `0.25` and healthy at or above it, exactly at the boundary | a `<=` / `<` flip at the threshold |

Two process rules carried in from §0.0, both earned:

- **Do not verify this live by diffing two frames.** M50's control run (the same
  item rendered twice) differed in 41,284 pixels against a 16,329-pixel signal,
  and M37 retracted a frame-diff witness for the same reason. Count vertices and
  read back decoded values instead.
- **The gate must drive the real emitter**, not a parallel copy of it — M45 and
  M41 both shipped gates that had quietly stopped testing their subject because
  they reimplemented a slice of the app's setup.

---

## Deliberately excluded

- **Numeric health text.** Needs per-run text styling the text pass cannot
  express, and doubles the width budget.
- **Local player.** Vanilla's hearts already cover it and are shipped.
- **Boss bars.** `ClientboundBossEventPacket` is a genuinely server-driven,
  genuinely vanilla widget — it belongs in its own milestone, transcribed
  normally, and must not be conflated with this.
- **Armour / absorption indicators.** More inputs, same open design question;
  revisit once this has been looked at in play.
