# Rewo feature survey — what 9,291 open-source client mods say to build next

Survey date **2026-07-26**. Regenerate with `python tools/survey_modrinth.py`
(cache is derived data, written under `%APPDATA%/EwoClient/rewo/survey/`, not
committed). Every number below is that tool's output — if you change the
facets or the `MODERN` version set, the numbers move and this document is
stale.

**Scope decision (locked):** this survey is read as a **Rewo-native roadmap**,
not as a shopping list for the EwoLoader bundle. Every entry below is a
feature to *reimplement* in the `rewo-*` crates. That choice is what makes the
license column load-bearing rather than decorative — see §2.

---

## §0 Handoff — the five things worth knowing

1. **The population is ~500 mods, not 9,291.** 7,560 are alive on 1.21+/26.x,
   and the top 250 carry **89.5%** of all downloads. Enumerate everything;
   weight almost nothing.
2. **Two-thirds of the ecosystem is not Rewo's problem.** 33.7% of download
   mass exists *only* because the game is a JVM client with a mod loader
   (§3). Rewo dissolves that class by construction. The remaining 66.3% is
   the actual roadmap.
3. **The market leaders in two clusters are not open source** and must not be
   read as reference implementations: Sodium (Polyform Shield), Xaero's
   Minimap (All Rights Reserved), EntityCulling (bespoke protective license).
   §2 has the safe alternates.
4. **The single largest still-open OptiFine-parity item is ETF at 88M
   downloads** — emissive + random entity textures. That is Rewo's existing
   M9b, and this survey says it outranks every other asset feature.
5. **Independent-reimplementation count beats download count** as a signal.
   115 people wrote a tooltip mod and none stuck; 32 people wrote inventory
   sorting and Mouse Tweaks won outright at 50M. Those two facts imply
   opposite strategies (§5).

---

## §1 Method

Modrinth search API v2, faceted:

```
project_type:mod
categories:fabric
open_source:true
server_side:(unsupported OR optional)
```

→ **9,291** projects, paged 100 at a time by descending downloads. That facet
set reproduces the population exactly; `open_source:true` is not cosmetic
(§2). Filtered to **7,560** alive projects (any 1.21+ or 26.x version), then:

- clustered by description text, because the category taxonomy is useless
  here — **5,870 of 7,560 live mods self-tag `utility`**;
- the top ~760 by downloads read by hand, after excluding libraries, content
  mods, and server-specific mods (Hypixel/Wynncraft/Cobblemon);
- counted per feature by *independent reimplementations*, not downloads.

**Concentration.** Top 50 → 54.9%, top 100 → 71.7%, top 250 → 89.5%,
top 500 → 96.7% of 5,720M downloads.

---

## §2 The license map — read this before reading anyone's source

Rewo reimplements rather than bundles, so a reference mod's license governs
whether its source may be consulted at all. This is a materially different
question from whether EwoLoader may ship the jar.

**Reference-unsafe** (source-available, not OSI-open; do not read for Rewo):

| downloads | mod | license |
|---:|---|---|
| 194M | **Sodium** | `LicenseRef-Polyform-Shield-1.0.0` |
| 141M | **EntityCulling** | `LicenseRef-tr7zw-Protective-License` |
| 96M | **Xaero's Minimap** | `LicenseRef-All-Rights-Reserved` |

Polyform Shield permits use and redistribution but carves out **competing**
use. Rewo is a competing renderer. **EwoLoader bundling the Sodium and
EntityCulling jars is a separate question and is not what this warns about** —
the warning is specifically against reading their source to inform Rewo's
Vulkan renderer or culling. Worth a real legal read before anyone does;
this document is not legal advice.

**Reference-safe alternates** for the same ground:

| cluster | safe reference | license |
|---|---|---|
| renderer / culling | Rewo is already ahead; nothing needed | — |
| minimap | Surveyor Map Framework (2.1M), Antique Atlas 4 (2.6M) | LGPL-3.0-or-later |
| LOD terrain | Distant Horizons (30M) + DH-for-VulkanMod (24k) | LGPL-3.0-only |
| chunk caching | Bobby (19M) | LGPL-3.0-only |
| particles | Particle Core (13M, MIT), AsyncParticles (5M, LGPL) | MIT / LGPL |

Everything else on the shortlists in §4 is MIT, Apache-2.0, MPL-2.0, BSD-3,
LGPL, GPL or OSL — safe to read, with the usual copyleft caution that reading
is not copying.

---

## §3 What the survey says about work Rewo has already done

Download mass by problem class, over the 7,560 live mods:

| class | mods | downloads | share | Rewo status |
|---|---:|---:|---:|---|
| A. JVM/render performance | 253 | 658M | 11.5% | **solved by construction** |
| B. OptiFine resource-pack parity | 48 | 497M | 8.7% | **mostly shipped (M9 CEM)**, ETF open |
| C. Modding infrastructure | 182 | 775M | 13.5% | **not applicable** |
| D. QoL / features | 7,077 | 3,791M | 66.3% | **the roadmap** |

Two conclusions:

**Class A is the strongest external validation Rewo has received.** 658M
downloads of people patching around the Java client's frame times — FerriteCore
(132M, "memory usage optimizations"), ImmediatelyFast (108M), ModernFix (69M),
Krypton (37M), BadOptimizations (36M), ScalableLux (11M, a Starlight fork that
does what `rewo-world/src/light.rs` already does natively). None of it is a
feature. All of it is tax. Rewo does not pay that tax.

**Class B says finish M9b.** ETF at 88M is the largest single unshipped asset
feature in the ecosystem, and Rewo already has the CEM machinery (`rewo-gpu/
src/cem.rs`, `cem_anim.rs`) that makes it tractable. EMF (84M) is already
matched by M9. Continuity (64M) is matched by the mesher. CIT Resewn (22M),
OptiGUI (22M), Animatica (19M), Polytone (13M) and the skybox mods
(Nuit 13M, Skyboxify 11M) are all still open, but each is smaller than ETF.

Class A does still hide **three algorithms worth lifting**, all
reference-safe, all of which Rewo has the substrate for:

- **Bobby** (LGPL) — cache chunks client-side past the server's view distance.
  Rewo already owns its column store in `rewo-world`; this is persistence plus
  a fallback path in the chunk loader.
- **Distant Horizons** (LGPL) — LOD terrain. A DH-for-VulkanMod fork exists,
  which is the closer read for a Vulkan client.
- **Particle Core / AsyncParticles** (MIT / LGPL) — particle culling and GPU
  particle rendering. Rewo has no particle system at all yet, so this is
  greenfield with a good reference rather than a retrofit.

---

## §4 The gap catalog

Demand is the tool's `independent reimplementations` count; downloads are the
cluster total. "Rewo today" is measured against the actual crates, not the
Skia HUD in `ewo-jni` (that is the Fabric path and does not carry over).

### Already covered — do not rebuild

| feature | mods | dl | where |
|---|---:|---:|---|
| Custom crosshair | 58 | 68M | `ewo-jni/src/crosshair.rs` (Fabric path); Rewo draws vanilla crosshair in `rewo-gpu/src/hud.rs` |
| Reach / hit indicators | 85 | 20M | modules `crosshair_on_reach`, `hit_indicator`, `hit_color` |
| Fullbright / gamma | 61 | 17M | module `fullbright`; Rewo has the real lightmap (M13) |
| Freecam / freelook | 25 | 21M | module `freelook` |
| Armor HUD | 48 | 19M | HUD widget |
| Toggle sprint / sneak | 39 | 3M | modules |
| Ping display | 25 | 14M | HUD widget |
| Keystrokes display | 21 | 0.5M | HUD widget |
| Zoom | 67 | 83M | bundled Zoomify — **not native to Rewo yet**, see below |

> **Caveat on that last row.** Everything above except Zoom exists in the
> *Fabric* client (`ewo-jni` + `ewo-core::modules`). Rewo's HUD
> (`rewo-gpu/src/hud.rs`) currently draws crosshair, hotbar, hearts, hunger,
> the F3 overlay (`overlay.rs`) and chat/coords text (`text.rs`) — nothing
> more. **Porting the 12 legit modules and the HUD widget set into Rewo is
> itself an unlisted milestone**, and it is a prerequisite for Rewo being a
> credible daily driver. It is not in the tables below because the survey
> can't see it; flagging it here so it doesn't get lost.

### Tier 1 — build these

| # | feature | mods | dl | safe reference | Rewo surface |
|---|---|---:|---:|---|---|
| 1 | **Inventory & container QoL** — sorting, quick-stack, shulker preview, chest search | 32 + 7 | **125M + 34M** | Mouse Tweaks (BSD-3), Client Sort (Apache-2.0), Shulker Box Tooltip (MIT), Inventory Profiles Next (AGPL — read with care) | new container-screen layer; needs Rewo's first real GUI-screen pass |
| 2 | **Tooltip overhaul** — layout, durability numbers, enchantment text, held-item info | 115 + 103 + 23 | 66M + 11M + 54M | Adaptive Tooltips (MPL-2.0), Tooltip Overhaul (GPL-3.0), Held Item Info (LGPL), Show Durability (MIT) | `rewo-gpu/src/text.rs` + `held.rs`; pure typography, plays to the Skia-adjacent strengths |
| 3 | **Screenshot & capture** — clipboard, high-res, viewer, panorama | 83 | 30M | Fabrishot (MIT), Screenshot to Clipboard (MIT), Screenshot Viewer (MIT), Panorama Screenshot (MIT) | **`rewo-gpu/src/offscreen.rs` already does this** — the headless `--out png` path is the same machinery. Cheapest high-demand win in the survey |
| 4 | **Menu & loading flow** — no reload screen, fast quit, pinned worlds, skip transitions | 77 | **95M** | RRLS (OSL-3.0), FastQuit (MIT), Cherished Worlds (LGPL) | launcher-adjacent; Rewo has no menu system yet, so this is design space rather than a patch |
| 5 | **Status-effect timers** | 34 | 28M | Status Effect Bars (LGPL) | `hud.rs`; metadata already decoded |

### Tier 2 — strong, but a milestone each

| # | feature | mods | dl | safe reference | note |
|---|---|---:|---:|---|---|
| 6 | **Chat QoL + chat heads** | 34 + 8 | 12M + 50M | Chat Patches (LGPL), Chat Heads (MPL-2.0) | Chat Heads alone is 43M. Rewo already renders player skins (M7c) — the head sprite is nearly free |
| 7 | **Shoulder / 3rd-person camera** | 37 | 27M | Shoulder Surfing Reloaded (MIT) | camera math only; Rewo owns its camera |
| 8 | **Capes / cosmetics** | 49 | 30M | Capes (LGPL-2.1), Cosmetica (Apache-2.0) | Half done already: `rewo-net/src/skins.rs` **parses the `CAPE` texture URL** off the profile, and M7c's 32-slot skin pool generalizes. What's missing is the cape geometry + render path — nothing in `rewo-gpu` references a cape today |
| 9 | **Player / mob health bars** | 48 | 6M | Mob Plaques, Health Bars (MIT-ish) | needs world-space text — **already shipped for signs (M27)** |
| 10 | **Sound physics / muffling** | 10 | **52M** | Sound Physics Remastered (GPL-3.0) | Rewo has *no audio at all* — verified, and `rewo-world/src/entities.rs:940` says so in as many words. This is a whole subsystem, not a feature; the 52M is really demand for *audio that works*, which Rewo would first have to have |
| 11 | **Discord rich presence** | 29 | 14M | CraftPresence (MIT) | trivial; but violates "OFFLINE FIRST. NOTHING PHONES HOME." — needs an explicit opt-in decision |
| 12 | **Borderless fullscreen** | 12 | 27M | Cubes Without Borders (MIT) | window-layer, not renderer |
| 13 | **Light-level overlay** | 5 | 6M | Lighty (Apache-2.0) | Rewo's light engine (M10) makes this near-free — the data is already exact |

### Tier 3 — real demand, real cost

| # | feature | mods | dl | note |
|---|---|---:|---:|---|
| 14 | **Minimap / waypoints** | 74 | 28M | Market leader is ARR. Safe refs are small. Rewo has the column data in memory and a Vulkan stack — a top-down pass is very doable, but this is an M-number |
| 15 | **Schematics (Litematica-class)** | 42 | 26M | Litematica is LGPL and readable. Large surface: schematic format, ghost rendering, material lists |
| 16 | **Dynamic lights** | 5 | 18M | Only 5 mods but 18M downloads — LambDynamicLights won. Requires touching Rewo's light propagation per-frame; interacts with M10's exactness gate |

---

## §5 Ranking and sequence

**Policy (locked 2026-07-26): breadth-first, weight independent mods, divide
by effort.** `python tools/survey_modrinth.py --rank` computes it. Reproduce
the table below rather than hand-editing it.

```
score = (mods / max_mods) / effort
```

Blending downloads back in as a 25% minority term was measured and **changed
nothing material** — top 8 near-identical, nothing moved more than 4 places.
The `MODS_WEIGHT` constant survives as an escape hatch for a future candidate
with very few mods and enormous downloads, but the mods-only rule stands.

Top of the computed ranking (32 candidates, full table from the tool):

| # | score | mods | dl | eff | feature |
|---:|---:|---:|---:|---:|---|
| 1 | 89.6 | 103 | 11M | 1 | Item durability display |
| 2 | 58.3 | 67 | 83M | 1 | Zoom **[port]** |
| 3 | 53.0 | 61 | 17M | 1 | Fullbright / gamma **[port]** |
| 4 | 50.0 | 115 | 66M | 2 | Tooltip overhaul |
| 5 | 41.7 | 48 | 19M | 1 | Armor HUD **[port]** |
| 6 | 37.0 | 85 | 20M | 2 | Reach / hit indicators **[port]** |
| 7 | 36.1 | 83 | 30M | 2 | Screenshot tooling |
| 8 | 33.9 | 39 | 3M | 1 | Toggle sprint / sneak **[port]** |
| 9 | 25.2 | 29 | 14M | 1 | Discord rich presence |
| 10 | 25.2 | 58 | 68M | 2 | Custom crosshair **[port]** |
| 11 | 21.7 | 25 | 14M | 1 | Ping display **[port]** |
| 12 | 21.3 | 49 | 30M | 2 | Capes / cosmetics |
| 13 | 20.9 | 48 | 6M | 2 | Player / mob health bars |

**The ranking's loudest result: 7 of the top 11 rows are `[port]`.** Those are
features the Fabric client already has and Rewo does not — Zoom, Fullbright,
Armor HUD, reach/hit indicators, toggle sprint/sneak, custom crosshair, ping
display (plus keystrokes at #14 and freecam at #23). Scored individually they
flood the table; in practice they are **one milestone**: port the 12 legit
modules and the HUD widget set out of `ewo-jni` into `rewo-gpu/src/hud.rs`.
Breadth-first arithmetic and common sense agree for once — that milestone is
first, and it is worth ~429 mods and 245M downloads of demand for roughly the
effort of one Tier-1 feature.

### Sequence

1. **Port the module + HUD set into Rewo.** 7 of the top 11 rows, one piece of
   work. Also the thing standing between Rewo and being a daily driver.
2. **Finish M9b (ETF).** Not in the ranked table — it is class B, not class D —
   but it is the largest single unshipped item in the ecosystem at 88M
   downloads and the CEM machinery already exists.
3. **Tooltips as one milestone**: item durability display (#1), tooltip
   overhaul (#4) and held-item / enchantment info (#20). Three separately
   ranked features on **one surface** (`rewo-gpu/src/text.rs` + `held.rs`);
   shipping them apart would pay the layout cost three times. Shulker box
   preview (#29) rides on the same infrastructure and should be folded in
   despite its low individual score.
4. **Screenshot tooling** (#7). `offscreen.rs` already renders headless PNGs;
   this is the best demand-to-effort ratio that isn't already covered above.
5. **Capes** (#12) — `rewo-net/src/skins.rs` already parses the URL — then
   **player/mob health bars** (#13), which reuse the world-space text shipped
   for signs in M27.

Everything below ~#15 is genuinely a "later" list. Two notes on where
breadth-first deliberately deprioritises something valuable: **inventory
sorting** (#24) has the highest download mass in the entire survey at 125M but
needs Rewo's first real GUI-screen pass, and **sound physics** (#31) sits on a
52M-download cluster that Rewo cannot touch until it has an audio subsystem at
all. Both are correct to defer under this policy and both would jump if the
policy were download-weighted — flagged so the deferral is a decision rather
than an oversight.

### Why the two signals disagree, and what it means for *how* to build

The ranking says what to build. This says how.

**High mod-count, low download-per-mod = an unsolved itch.** Tooltips (115
attempts, 66M spread thin), durability (103 attempts, 11M). Nobody won.
Building these natively is differentiating — you get to have taste, and
Velvet's whole identity is typography and layout. This is where Rewo should
*innovate*.

**Low mod-count, high download-per-mod = a solved problem.** Inventory sorting
(32 mods, 125M — Mouse Tweaks and IPN took it), Chat Heads (8 mods, 50M),
Shulker Box Tooltip (7 mods, 34M). Users do not want a new opinion here; they
want the feature present and behaving the way they already expect. **Copy the
established behaviour exactly.** Deviating is a bug, not a differentiator.

Note this cuts *across* the ranking rather than reordering it. Tooltips rank
#1/#4 and get taste; the ported modules rank high and should be
behaviour-identical to their Fabric originals; shulker preview ranks #29 and
should copy Shulker Box Tooltip's behaviour precisely when it eventually
ships. Rank decides *when*, this decides *how much license to take*.

Each milestone ships a headless gate, per the standing verification mandate —
`rewo tooltipshot --check`, `rewo hudshot --check`, and so on, in the
`*_cmd.rs` pattern the other 12 gates already follow.

---

## §6 Explicit exclusions

Recorded so they are not re-derived:

- **Class A performance mods** — solved by construction, except the three
  algorithms named in §3.
- **Class C infrastructure** — Rewo has no mod loader and no config-library
  problem.
- **Gameplay-altering content** — adventure/magic/worldgen/mobs/food/economy
  categories were filtered out of the survey per the stated scope.
- **Server-specific mods** — Hypixel/SkyBlock (Skyblocker 3.8M, SkyHanni
  3.7M), Wynncraft (Wynntils 1.7M), Cobblemon. Large but not portable.
- **Assist/PvP features** — the post-ban legit/pvp split governs those; the
  survey's `auto-clicker / macro` cluster (91 mods) is deliberately not
  tiered above.
- **Sodium, EntityCulling, Xaero's Minimap source** — see §2.

---

## §7 Re-running

```bash
python tools/survey_modrinth.py            # cached 7 days
python tools/survey_modrinth.py --refresh  # force re-fetch (~93 requests, ~2 min)
```

Bump `MODERN` in the tool when the client pin moves. If a mod that dominates
a cluster turns out to be non-OSI, the tool prints it under
*"reference-UNSAFE despite ranking in the top 400"* — check that section every
re-run, because licenses change and Sodium's did.
