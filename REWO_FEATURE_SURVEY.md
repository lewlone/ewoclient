# Rewo feature survey — what 9,291 open-source client mods say to build next

Survey date **2026-07-26**. Regenerate with `python tools/survey_modrinth.py`
(cache is derived data, written under `%APPDATA%/EwoClient/rewo/survey/`, not
committed). Every number below is that tool's output — if you change the
facets, the `MODERN` version set or the classifiers, the numbers move and this
document is stale.

**Scope decision (locked):** read as a **Rewo-native roadmap**, not as a
shopping list for the EwoLoader bundle. Every entry is a feature to
*reimplement* in the `rewo-*` crates. That is what makes the license column
load-bearing rather than decorative — see §2.

---

## §0 Handoff — the seven things worth knowing

1. **9,291 mods collapse to 69 distinct features.** The unit that matters is
   "a problem someone solved", not "a mod". 55 zoom mods are one feature. Of
   the 69: **48 are QoL work to build**, 12 are one port milestone, 3 are
   blocked on audio, 2 are atmosphere, 1 already exists, 3 don't port (§4).
   **Treat 69 as a floor and every count as ±20%** — both directions are
   measured in §1.
2. **More than half the ecosystem is not Rewo's problem.** 53.4% of download
   mass is modding infrastructure, JVM performance, and OptiFine-pack parity —
   all dissolved by construction (§3). That is the strongest external
   validation Rewo has.
3. **The long tail is real and correctly ignorable.** After the 69 features
   and the class split, **4,621 mods remain carrying ~11% of downloads at a
   median of 890 each** — one-off gameplay, localization and cosmetic tweaks.
   The 69 features are **27.5% of all downloads**; do not read that as "the
   features are everything", read it as "the features plus the JVM tax are
   ~81% and the rest is dust".
4. **The market leader in every big solved category is non-open-source.**
   Rendering (Sodium), culling (EntityCulling), minimap (Xaero's,
   JourneyMap) and block-info (Jade, WTHIT) — ~509M downloads, none of it
   readable as reference. That is a pattern, not a coincidence, and it means
   Rewo solves those four from scratch with no source to consult (§2).
5. **The largest still-open OptiFine-parity item is ETF at 88M downloads** —
   emissive + random entity textures. That is Rewo's existing M9b.
6. **Independent-reimplementation count beats download count** as a signal,
   and the two disagree usefully. 111 people wrote a tooltip mod and none
   stuck; 33 wrote inventory sorting and Mouse Tweaks won at 50M. Those imply
   opposite strategies, separable by downloads-per-mod (§5).
7. **The effort column is the weakest input in this document.** It is
   guesswork, never validated against an implementation, and it is the
   *denominator* of the whole ranking. Grounding one estimate against real
   work is the cheapest thing that would improve the ordering.

---

## §1 Method

Modrinth search API v2, faceted:

```
project_type:mod
categories:fabric
open_source:true
server_side:(unsupported OR optional)
```

→ **9,291** projects, paged 100 at a time. `open_source:true` is not cosmetic
(§2). Filtered to **7,560** alive (any 1.21+ or 26.x version), then classified
in this order — the order is load-bearing:

1. **Problem class, applied disjointly** (infrastructure → performance →
   OptiFine-parity), so a library that also says "performance" is counted once.
2. **Addon filter.** An "addon for X" is ecosystem, not a feature.
3. **Feature clustering within class D only**, one entry per distinct feature.
4. Top ~800 by downloads read by hand to catch what regexes miss.

Category tags are **not** used — they are self-reported and useless here:
**5,870 of 7,560 live mods tag themselves `utility`**.

**Concentration.** Top 50 → 54.9%, top 100 → 71.7%, top 250 → 89.5%,
top 500 → 96.7% of 5,720M downloads.

### Three classifier traps, all of which produced wrong numbers first

- **Never leave a trailing `\b` after a stem.** Wrapping an alternation in
  `\b(...)\b` makes `optimi[sz]` require a boundary right after "optimiz", so
  it never matches "optimization" — Lithium was filed as a QoL feature. Same
  for `librar` vs "library" (YACL and half the library ecosystem escaped class
  C) and `skybox` vs "skyboxes". **This understated the JVM-tax share by 15
  points.** Use a `\w*` tail.
- **Measure features only after classes are assigned.** ModernFix ("improves
  performance … fixes many bugs") inflated `Vanilla bug fixes` by 69M until
  class A claimed it first. And 51 of 54 item/recipe-viewer matches were
  JEI/EMI/REI *addons*; unfiltered, that cluster reads 167M instead of 88M.

### The structural trap, and the calibration that bounds it

**A hand-authored feature list will be short, and you cannot tell by how much
without measuring.** This document went 32 → 55 → 59 → 68 features and
33.7% → 49.2% → 53.4% JVM tax across four revisions, each presented as settled.
Every correction moved the same direction: *more* had been missed.

The head of the residue is where the misses hide. The first revision left
5,239 uncovered mods whose head included **AppleSkin at 78M** — a genuine
feature disguised by a description ("food/hunger HUD improvements") that reads
like content until you notice it is a *readout*. Quoting the residue's median
(998) without its head made the gap look smaller than it was.

**So the count was calibrated rather than asserted.** A random sample of 40
mods drawn from the 4,937 that had never been read found roughly a quarter to
be features absent from the list — accessibility, account switching,
ghost-block resync, confirmation prompts, window appearance, container
readouts, motion blur, flight HUDs. Those nine clusters are now folded in.
Extrapolating the same rate, **exhaustive cataloguing would likely reach
75–85 features, so 68 is a floor.**

What bounds the error is *weight*, not count: the still-unread tail is ~4,600
mods carrying **~4% of all downloads** at ~44k each. The list is therefore
meaningfully incomplete as a **catalogue** and near-complete as a
**prioritisation** — nothing left out can reorder the top 15.

**The counts are soft in the other direction too.** A second random sample,
this time of 30 mods from the *covered* set, found **~7 wrong — a false-
positive rate near 20%** — in four distinct failure modes: addon-filter
leaks (`ReplayMod-Pose-Fix` by title prefix, "Let Litematica be able to…"
by verb phrase), server-specific mods that the stated scope excludes but the
code did not (Cobblemon, Wynncraft, "for the Modern Beta SMP"), genuine
wrong-feature assignment (a water-bucket PvP mod matched `smart\w* placement`;
a block-info HUD matched a bare `crosshair`), and residual ambiguity that is
not fixable by pattern (Death Logger really does save "deaths screenshots").
The first three are fixed — `ADDON` grew title-prefix and verb-phrase forms,
`SERVER_SPECIFIC` is new, and two patterns were tightened.

**Features also overlap**: 185 of 1,686 covered mods (11%) match more than
one, so the sum of per-feature counts exceeds the unique count by ~12%. That
is defensible — a mod can genuinely do two things — but it means per-feature
`mods` is not a partition and must not be summed.

Net: **treat any individual count as ±20% and the feature list as a floor.**
The error is roughly uniform across clusters, which is why the *ranking*
survives it while individual figures should not be quoted as fact. That
uniformity is itself an untested assumption.

Three guards now exist because each failed at least once: `report()` prints
the residue's head *and* median; `rank()` asserts every FEATURES key has a
disposition and vice-versa; and `_validate_patterns()` rejects control
characters, because scripted edits twice collapsed a `\b` escape into a
literal 0x08 — which silently changes what a pattern matches instead of
raising. **Print a widened pattern's top hits before trusting its count**:
`accessib\w*` caught "easily accessible" and `readab\w*` caught BetterF3's
"human-readable HUD", between them inflating accessibility from ~25 mods to 56.

---

## §2 The license map — read this before reading anyone's source

Rewo reimplements rather than bundles, so a reference mod's license governs
whether its source may be consulted at all. Materially different from whether
EwoLoader may ship the jar.

**Reference-unsafe** (source-available, not OSI-open; do not read for Rewo):

| downloads | mod | license | category it owns |
|---:|---|---|---|
| 194M | **Sodium** | `LicenseRef-Polyform-Shield-1.0.0` | rendering |
| 141M | **EntityCulling** | `LicenseRef-tr7zw-Protective-License` | culling |
| 97M | **Xaero's Minimap** | `LicenseRef-All-Rights-Reserved` | minimap |
| 60M | **Jade** | `CC-BY-NC-SA-4.0` | block info ("what am I looking at") |
| 13M | **JourneyMap** | `LicenseRef-All-Rights-Reserved` | minimap |
| 3.9M | **WTHIT** | `CC-BY-NC-SA-4.0` | block info |

**~509M downloads, and every one of the four biggest solved categories is in
here.** NonCommercial (CC-BY-NC-SA) is not OSI-open either — it discriminates
by field of endeavour — which is why Jade and WTHIT are absent from the
surveyed population entirely, and why the `Block info under crosshair`
cluster measures a near-empty 2 mods despite the category being worth 64M.
**An empty-looking cluster may mean the category is licensed out of the
population, not that nobody wants it.**

Polyform Shield permits use and redistribution but carves out **competing**
use. Rewo is a competing renderer. **EwoLoader bundling the jars is a separate
question and not what this warns about** — the warning is against reading
their source to inform Rewo. Worth a real legal read before anyone does; this
document is not legal advice.

**Reference-safe alternates:**

| cluster | safe reference | license |
|---|---|---|
| renderer / culling | Rewo is already ahead; nothing needed | — |
| minimap | Surveyor Map Framework (2.1M), Antique Atlas 4 (2.6M) | LGPL-3.0-or-later |
| LOD terrain | Distant Horizons (30M) + DH-for-VulkanMod (24k) | LGPL-3.0-only |
| chunk caching | Bobby (19M) | LGPL-3.0-only |
| particles | Particle Core (13M, MIT), AsyncParticles (5M, LGPL) | MIT / LGPL |
| item/recipe viewer | JEI (65M, MIT), EMI (25M, MIT) | MIT |
| inventory | Mouse Tweaks (BSD-3), Client Sort (Apache-2.0) | permissive |

Everything else in §4 is MIT, Apache-2.0, MPL-2.0, BSD-3, LGPL, GPL or OSL —
safe to read, with the usual copyleft caution that reading is not copying.

---

## §3 What the survey says about work Rewo has already done

Download mass by problem class, over 7,560 live mods, assigned disjointly:

| class | mods | downloads | share | Rewo status |
|---|---:|---:|---:|---|
| C. Modding infrastructure | 481 | 1,453M | **25.4%** | **not applicable** |
| A. JVM/render performance | 331 | 1,002M | **17.5%** | **solved by construction** |
| B. OptiFine resource-pack parity | 59 | 601M | **10.5%** | **mostly shipped (M9 CEM)**, ETF open |
| D. QoL / features | 6,689 | 2,664M | 46.6% | **the roadmap** |

**53.4% of client-mod download mass is JVM tax.** Not features — tax. Cloth
Config (145M), Mod Menu (125M), YACL (105M), Fabric API (216M) exist to make
mods configurable and loadable. FerriteCore (132M), Lithium (112M),
ImmediatelyFast (108M), ModernFix (69M), Krypton (37M) exist to patch a Java
client's frame times. ScalableLux (11M) is a Starlight fork doing what
`rewo-world/src/light.rs` already does natively. None of it is a feature.

**Class B says finish M9b.** ETF at 88M is the largest single unshipped asset
feature in the ecosystem, and `rewo-gpu/src/cem.rs` + `cem_anim.rs` already
provide the machinery. EMF (84M) is matched by M9; Continuity (64M) by the
mesher. CIT Resewn (22M), OptiGUI (22M), Animatica (19M), Polytone (13M) and
the skybox mods (Nuit 13M, Skyboxify 11M) remain open but each is smaller.

Class A hides **three algorithms worth lifting**, all reference-safe:

- **Bobby** (LGPL) — cache chunks past the server's view distance. Rewo owns
  its column store in `rewo-world`; this is persistence plus a loader fallback.
- **Distant Horizons** (LGPL) — LOD terrain. A DH-for-VulkanMod fork exists,
  the closer read for a Vulkan client.
- **Particle Core / AsyncParticles** (MIT / LGPL) — particle culling and GPU
  particles, now that M37 shipped the particle system.

---

## §4 The 69 distinct features

`tools/survey_modrinth.py` measures `mods` and `dl` **live** from the survey —
they are not stored. `DISPOSITION` in that file holds only effort and kind, so
widening a pattern updates the ranking automatically and cannot leave a stale
copy behind. Disposition:

| disposition | count | |
|---|---:|---|
| **QoL — build these** | **48** | the roadmap |
| **[port]** — in the Fabric client, not in Rewo | 12 | **one milestone** |
| **audio** — blocked | 3 | Rewo has no audio subsystem at all |
| **atmosphere** — cosmetic, ranked apart | 2 | ambient particles, motion blur |
| already in the `rewo-*` crates | 1 | Debug / F3 overlay (`overlay.rs`) |
| does not port | 3 | see below |
| **total distinct features** | **69** | from 9,291 mods (a floor — see §1) |

**Coverage, stated honestly.** These 69 account for 1,584 of 6,205 class-D
mods (25.5%) and **27.5% of all downloads** — roughly **half of class D's own
download mass**. They are not "basically everything". Full accounting of the
5,720M:

| segment | share |
|---|---:|
| JVM tax (classes A + B + C) | 53.4% |
| the 69 distinct features | 27.5% |
| addon ecosystem (111 mods) | 3.5% |
| uncovered long tail (4,621 mods) | ~11% |

The uncovered tail's **median is 890 downloads** and — now that the classifier
misses are folded in — its head is gameplay (Cut Through, Do a Barrel Roll,
Fabric Seasons), localization (I18nUpdateMod, JustEnoughCharacters — English-
only is a stated scope exclusion), cosmetics (Eating Animation) and
Sodium-regression fixes (Shadowy Path Blocks). Nothing portable, nothing
large.

**Does not port**, recorded so it isn't re-derived:

- **Vanilla bug fixes** (40 mods, 36M) — Debugify and friends patch specific
  `MC-…` tickets. Rewo is reimplemented from the decompile against gates; most
  of these bugs never exist. Any that do are Rewo's own bugs, not a feature.
- **Server list QoL** (18 mods, 5M) — the server list lives in the launcher,
  not in Rewo.
- **Options preservation** (4 mods, 27M) — YOSBR exists because mods stomp
  vanilla's `options.txt`. Rewo owns its own config; nothing to preserve.

---

## §5 Ranking and sequence

**Policy (locked 2026-07-26): breadth-first, weight independent mods, divide
by effort.** `python tools/survey_modrinth.py --rank` computes it. Reproduce
rather than hand-edit.

```
score = (mods / max_mods) / effort
```

Blending downloads back as a 25% minority term was measured and **changed
nothing material** — top 8 near-identical, nothing moved more than 4 places.
`MODS_WEIGHT` survives as an escape hatch for a future candidate with very few
mods and enormous downloads; the mods-only rule stands.

### Top of the QoL + port ranking

| # | score | mods | dl | eff | feature |
|---:|---:|---:|---:|---:|---|
| 1 | 85.6 | 95 | 11M | 1 | Item durability display |
| 2 | 50.0 | 111 | 66M | 2 | Tooltip overhaul |
| 3 | 49.5 | 55 | 78M | 1 | Zoom **[port]** |
| 4 | 47.7 | 53 | 17M | 1 | Fullbright / gamma **[port]** |
| 5 | 36.5 | 81 | 30M | 2 | Screenshot tooling |
| 6 | 36.0 | 80 | 19M | 2 | Reach / hit indicators **[port]** |
| 7 | 36.0 | 40 | 19M | 1 | Armor HUD **[port]** |
| 8 | 31.5 | 35 | 3M | 1 | Toggle sprint / sneak **[port]** |
| 9 | 29.7 | 33 | 55M | 1 | Block highlight / outline |
| 10 | 27.9 | 62 | 16M | 2 | Scoreboard / tab list |
| 11 | 24.3 | 27 | 12M | 1 | Discord rich presence |
| 12 | 24.3 | 54 | 65M | 2 | Custom crosshair **[port]** |
| 13 | 23.4 | 52 | 22M | 2 | Player / mob health bars |

**6 of the top 12 are `[port]`.** Scored individually they flood the table; in
practice they are **one milestone** — port the 12 legit modules and the HUD
widget set out of `ewo-jni` into `rewo-gpu/src/hud.rs`. That bundle is 12
features, 418 mods and 242M downloads of demand for roughly the effort of one
Tier-1 feature. Rewo's HUD today draws crosshair, hotbar, hearts, hunger, the
F3 overlay and chat/coords text — nothing more.

### Sequence

1. **Port the module + HUD set into Rewo.** 7 of the top 11 rows, one piece of
   work, and the thing standing between Rewo and being a daily driver.
2. **Finish M9b (ETF).** Outside the ranked table (class B, not D) but the
   largest unshipped item in the ecosystem at 88M, machinery already present.
3. **Tooltips as one milestone**: item durability (#1), tooltip overhaul (#2),
   held-item / enchantment info (#24), shulker box preview (#43). Four
   separately ranked features on **one surface** (`rewo-gpu/src/text.rs` +
   `held.rs`); shipping them apart pays the layout cost four times.
4. **Screenshot tooling** (#5). `rewo-gpu/src/offscreen.rs` already renders
   headless PNGs — the best demand-to-effort ratio not already covered.
5. **Block highlight / outline** (#9) — the targeting outline already exists
   from the M-series; configurability is the whole gap. Then **scoreboard /
   tab list** (#10) and **player / mob health bars** (#13), reusing M27's
   world-space and screen-space text.

Deliberate deprioritisations under this policy, flagged so they read as
decisions rather than oversights. Note they share a shape — **a low mod count
next to enormous downloads is the signature of a solved problem, which
mods-per-effort punishes by construction:**

- **Item & recipe viewer** ranks **last (#51)** on 4 mods, yet carries **88M
  downloads** — JEI, EMI and REI. Few mods because three implementations
  saturated the space, not because demand is low. Effort 4 (a real screen, a
  recipe index, search). Download-weighted it would be top-5. **The single
  most likely thing this ranking gets wrong.**
- **Food / hunger HUD** (#26) — 11 mods, **82M downloads**. AppleSkin won
  outright, and EwoClient already bundles it. Effort 1. Same shape as above:
  under-ranked because it is *finished*, not because it is unwanted.
- **Inventory sorting** (#33) — 96M, needs Rewo's first real GUI-screen pass,
  which the recipe viewer also needs. Doing them adjacently compounds.
- **Chat heads** (#40) — 8 mods, 50M. Victory, not apathy.
- **Audio** — sound physics (52M), ambient sounds (52M), music control (12M).
  **117M of demand behind one prerequisite that is a subsystem, not a
  feature.** Rewo has no audio at all.
- **Ambient particles** — 36 mods, **89M**, ranked apart as atmosphere. On raw
  demand it would sit around #4. M37 shipped the particle system, so the
  prerequisite is gone; the reason it is not in the QoL list is that it
  competes on taste, which is a different kind of decision.

### Why the two signals disagree, and what it means for *how* to build

The ranking says what to build. This says how much license to take.

**High mod-count, low download-per-mod = an unsolved itch.** Tooltips (111
attempts, 66M spread thin), durability (96 attempts, 11M). Nobody won.
Building these natively is differentiating — you get to have taste, and
Velvet's identity is typography and layout. **Innovate here.**

**Low mod-count, high download-per-mod = a solved problem.** Item/recipe
viewer (4 mods, 88M), AppleSkin (11, 82M), Chat Heads (8, 50M), Shulker Box
Tooltip (7, 34M). Users do not want a new opinion; they want the feature
present and behaving the way they already expect. **Copy the established
behaviour exactly.** Deviating is a bug, not a differentiator.

The two shapes are separable by a single ratio — **downloads per independent
mod**. Tooltips: 0.6M/mod. AppleSkin's cluster: 7.4M/mod. Item/recipe viewer:
22M/mod. Above ~5M/mod you are looking at a solved problem and should copy;
below ~1M/mod nobody has won and taste is worth spending.

Each milestone ships a headless gate, per the standing verification mandate —
`rewo tooltipshot --check`, `rewo hudshot --check`, in the `*_cmd.rs` pattern
the other 12 gates already follow.

---

## §6 Explicit exclusions

- **Class A performance mods** — solved by construction, except the three
  algorithms in §3.
- **Class C infrastructure** — Rewo has no mod loader and no config-library
  problem.
- **Gameplay-altering content** — filtered per the stated scope. Note the
  category tags leak: Do a Barrel Roll, Fabric Seasons, Traveler's Titles,
  Cut Through and Eating Animation all self-tag `utility` and are not.
- **Server-specific mods** — Hypixel/SkyBlock (Skyblocker 3.8M, SkyHanni
  3.7M), Wynncraft (Wynntils 1.7M), Cobblemon. Large but not portable.
- **The JEI/EMI/REI addon ecosystem** (51 mods) — exists to surface *other
  mods'* recipes. Rewo speaks the vanilla protocol; there are none.
- **Assist/PvP features** — governed by the post-ban legit/pvp split.
- **Sodium, EntityCulling, Xaero's Minimap source** — see §2.

---

## §7 Re-running

```bash
python tools/survey_modrinth.py            # survey tables, cached 7 days
python tools/survey_modrinth.py --rank     # candidate ranking
python tools/survey_modrinth.py --refresh  # force re-fetch (~93 requests, ~2 min)
```

Bump `MODERN` when the client pin moves. Re-copy the measured `mods`/`dl`
columns into `CANDIDATES` after a refresh. Check the *"reference-UNSAFE
despite ranking in the top 400"* section every run — licenses change, and
Sodium's did.
