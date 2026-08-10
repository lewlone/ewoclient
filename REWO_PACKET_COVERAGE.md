# Rewo clientbound-play packet coverage — what the server sends that Rewo ignores

Audit date **2026-07-29** (M67), **re-derived 2026-07-29** (M74), counts
re-checked **2026-08-08** (M124, and again at **M125** and **M126**, neither of
which resolves a new packet — M125 reads two it already consumed, and M126
changes only how their text is carried and drawn), and unchanged through the
**M127–M134** integration, **M135** and **M140** — which between them decorate,
draw, sound and correct packets already consumed rather than resolving new
ones. (M140 is the one worth noting: it gave `level_event`'s sound table its
first production caller since M66, which changes what a handled packet
*consumes* without changing what is resolved — exactly the distinction §4
exists to record.) The §2 table is
machine-checked against `ids.rs` by a unit test, so it is the half to trust when
this prose and the numbers disagree. Against **26.2 / protocol
776**. Ground truth is the bundled datagen report
(`%APPDATA%/EwoClient/rewo/26.2/datagen/generated/reports/packets.json`) — the
same file `crates/rewo-net/src/ids.rs` resolves against — plus the Vineflower
decompile beside it. Nothing here is inferred from a wiki.

**Why this exists.** Rewo's packet handling grew milestone by milestone, so
what it decodes is a *historical accident, not a decision*. Twice a whole
family turned out to be simply absent and was found by noticing it was not in
`ids.rs` at all — the sound packets (M63) and the scoreboard / boss-bar /
tab-list set (M65). This is the enumeration that makes that failure mode
impossible to repeat: every clientbound-play packet in the report appears in
§5 with a status.

> ## ⚠ This table is machine-checked. Edit it when you edit `ids.rs`.
>
> `ids::tests::the_coverage_table_matches_the_code` (in
> `crates/rewo-net/src/ids.rs`) recomputes §2's counts and every §5 status
> from the source and fails `cargo test -p rewo-net` when they disagree. It
> exists because **the M67 audit went stale within hours of being written** —
> see §8. If you resolve a new packet, flip its §5 row to `handled` and bump
> §2; the test will tell you precisely which rows are wrong.

---

## §0 Handoff — the eight things worth knowing

1. **141 clientbound-play packets. Rewo resolves and consumes 118 of them. 23
   are not in `ids.rs` at all.** No packet is resolved-but-ignored: the
   `cb_play_*` field set and the dispatch chain agree exactly, which is a real
   (and slightly surprising) property of this codebase — see §1.
   **These two numbers live in §2, which is machine-checked; this paragraph is
   not.** It has now been stale twice (M104 and M124 both had to correct it),
   which is the same asymmetry CLAUDE.md records — trust §2, and fix §0 when
   you notice it disagreeing.
2. **Class A and class B are both empty.** The 23 gaps split 0 / 0 / 12 / 11
   across pure state, needs-rendering, needs-a-missing-subsystem and
   not-applicable — so every packet Rewo can render *is* rendered, and what is
   left needs a subsystem it has not got or is a reply to something it never
   sends. What the seven B milestones established is worth keeping:
   **the class letter changes the gate, not the standard.** M79's seven (title
   overlay, XP gauge, cooldown sweep), M80's six (the world border) and M81's
   three (hurt tilt, block cracks, item pickup) all have an exact vanilla oracle,
   so the decode *and* the render are transcribed line by line and graded against
   it, with a pixel read-back half on top of the model half. A class-B packet is
   not a guess; it is a transcription that happens to need a renderer to land.
   M83's `waypoint` (138) closed the last non-screen one, M82 took
   `player_combat_kill` (68) together with the screen framework the remaining
   two sit on, M85 took `server_links` (137), and **M84 closed the class with
   `award_stats` (3)**.

   The two screens that closed it ran in parallel and both needed the one gap
   M82 had named — `blitNineSlicedSprite` — so both wrote one. That is a
   scheduling artefact rather than a finding, and it resolved on the evidence:
   M85's took its sheet size and its border from constants, so it could express
   the 200×20 button and nothing else, and it stopped at the two branches that
   resize horizontally. M84's is parameterised and has all four, because a
   130×24 tab sheet drawn at 98 needs an asymmetric border and a 6×32 scroller
   drawn at 6×35 needs the vertical branches. One survived; `serverlinkshot`'s
   four nine-slice witnesses pass against it unchanged.

   **This entry used to call the screens a different kind of problem, "a design
   decision rather than a transcription". M82 found that only half true.** The
   design decision was real and *smaller* than it sounded — vanilla has one
   screen slot, not a stack — and everything else was an ordinary transcription
   that produced the usual inverted readings. §2, §3.
3. **The hand-maintained version of this document decayed at the rate the
   codebase changed.** M67 wrote it by grepping; four packets landed in
   `ids.rs` the same day, three of them from M68. By the time M74 re-derived
   it, **ten of the 141 rows were wrong** — all in the same direction, all
   saying `absent` about code that was present. §8 has the mechanism and the
   fix, and the fix is the machine check above, not vigilance.
4. **`bundle_delimiter` (0) closed in M78** — it was ranked the sharpest gap
   here because its failure mode is a rendering glitch rather than a protocol
   error. The correction M78 made to the ranking's own wording is worth
   keeping: vanilla applies a bundle in one **scheduled task**, not one *tick*,
   and the observable guarantee is that no frame is rendered part-way through.
   §9.
5. **The positional / rotational teleport asymmetry closed in M76.**
   `player_position` (72) had worked since M3 while `player_rotation` (73) and
   `player_look_at` (71) were never resolved, so a server turning your head did
   nothing at all — and the working half misdirected the diagnosis. M76 also
   found this document *wrong about the wire*: `player_rotation` carries no
   relative bitfield, and the bitfield reading decodes every packet without
   erroring. §3 keeps the entry as the worked example of both — a gap whose
   failure mode is silence in one direction only, and a wrong reading that
   never announces itself.
6. **`hurt_animation` (42) closed in M81, and closed `no_damage_tilt` with
   it.** This entry used to read "the input `M52a`'s vacuous `no_damage_tilt`
   module has nothing to disable"; the Velvet-batch note *"to port the disable
   you must first build the thing being disabled"* named the condition, and
   the packet was it. The tilt's direction exists nowhere else — `damage_event`
   arms the same clock and carries no yaw — so the module could not be made
   real without resolving this name. §3.
7. **"Handled" is not "complete."** Six currently-handled packets decode only
   the part a milestone needed — §4 names them, because a partially-consumed
   packet looks identical to a fully-consumed one from `ids.rs`, and the
   machine check in §1 cannot see the difference either.
8. **`player_abilities` (64) landed in M75**, together with the flight /
   no-clip physics it feeds (§4.1). It was listed `absent` when M74 re-derived
   this document; the machine check in §1 is what made flipping the row
   non-optional.

---

## §1 Method, and what "handled" means here

Three questions, asked separately, because conflating them is exactly how M63
and M65 stayed hidden:

1. **Is it in the report?** All 141 rows below come from `packets.json`'s
   `play.clientbound` table, sorted by `protocol_id`. Rewo resolves ids **by
   name**, so the id column is informational — a renumber is not a gap.
2. **Does `ids.rs` resolve it?** Parsed mechanically out of the `resolve`
   block: every `cb_play_*: req!(p, P, C, "<name>")` / `opt!(…)` entry.
3. **Does anything outside `ids.rs` dispatch on that field?** M67 asked this
   as a word-boundary grep across all of `crates/`. M74 asked it more
   strictly: does the field appear in an `==` / `!=` comparison, or as a
   dispatch-table field, **inside `rewo-net`**? The looser question counts a
   field a gate merely mentions; the stricter one counts only fields an
   incoming packet id is actually tested against.

**Both instruments give the same answer, and it is a negative finding: the
resolved-but-unreferenced set is empty.** Every one of the 118 resolved ids
reaches a dispatch arm in `play.rs` or a `route_*` in `lib.rs`. So the gap is
entirely in question (2) — 36 names that were never resolved.

That is still a coarser instrument than it sounds, and §4 is the correction:
a dispatch arm proves a field is *tested*, not that the body is fully consumed.

### The machine check, and the grammar it needs

`ids::tests::the_coverage_table_matches_the_code` re-runs questions (2) and
(3) against this document at every `cargo test -p rewo-net`. It reads three
files at compile time via `include_str!` — this document, `ids.rs` and the
dispatch chain — so it needs no datagen report, no network and no cwd, and it
cannot silently skip.

It asserts:

- §5 has exactly 141 rows, with ids `0..=140`, contiguous and unique.
- Every §5 row's status is `handled` **iff** the packet is resolved in
  `ids.rs` **and** dispatched in `rewo-net`.
- Every name resolved in `ids.rs` appears as a §5 row — so adding a packet
  without adding a row fails, which is the case that actually happened.
- Every `absent` row carries a class letter.
- §2's four status counts and four class counts equal the computed tally.

It therefore requires §5's rows to keep this shape, and §2's tables to keep
their labels:

```
| <id> | `<packet name>` | handled | <resolution> | <note> |
| <id> | `<packet name>` | absent  | **<A|B|C|D>** | <note> |
```

**A milestone marker is not a status.** The M67 table wrote `**M69**` in the
status column for four rows that had landed after the snapshot, which is what
made it unparseable and is half of why the drift went unnoticed. Attribution
belongs in the note column; the status column has two values.

### Classification

Each of the 23 gaps carries one class. The classes are about **what it would
take**, not about how much anyone wants it:

| Class | Meaning |
|---|---|
| **A** — pure state | Decoding it changes a value Rewo could act or gate on **without drawing anything new**. A witness can prove the decode; no human has to look at it. This is the class the M52–M74 batches drew from. |
| **B** — needs rendering | The decode is possible today, but the packet's purpose is a visual Rewo does not have (a title overlay, an XP bar, the damage camera tilt). Landing the *feature* needs an eyeball; landing the *decode* does not. |
| **C** — needs a subsystem Rewo lacks | The **12** remaining are, exactly: a horse/mount screen (41), a map image pipeline (51), a book viewer (58), a sign editor (60), a resource-pack fetcher (80, 81), an advancement tree (85, 129 is its neighbour, 130), a server-transfer/reconnect flow (129), the recipe *manager* (133 — the list vanilla's stonecutter and smithing menus filter, which M93s showed is jar-derivable for vanilla content and is not for a datapack), and a dialog framework (139, 140). The decode is not the hard part and shipping it alone buys nothing. **Three subsystems have come OFF this list**: the screen/menu framework (M82/M87), the recipe book (M93y–M107) and the chat-input path (M110 + M114–M124). |
| **D** — not applicable | Debug/dev tooling, integrated-server-only warnings, or a reply to a serverbound request Rewo never sends. Each row states which. |

The A/B line is drawn at *what the decode itself unlocks*, so a packet whose
state feeds physics (`explode`) is A even though explosions also have
particles, and a packet whose only consumer is a texture (`block_destruction`)
is B even though its body is three fields.

### Codec resolvability

Every codec below was located in the decompile. Spot checks confirmed the
dispatch registries the awkward ones need are present in `registries.json`:
`minecraft:stat_type` (9), `minecraft:custom_stat` (77),
`minecraft:debug_subscription` (16), `minecraft:recipe_display` (5),
`minecraft:menu` (25). **One row is marked not-verified**: `show_dialog` (140)
takes a `Holder<Dialog>` over the **datapack** `minecraft:dialog` registry plus
`Dialog`'s full codec tree; that is resolvable in principle and was not walked
here. No packet was found whose codec is unresolvable from the report plus the
decompile.

---

## §2 The counts

Machine-checked — see §1. Change these together with §5 or the test fails.

| Status | Count |
|---|---|
| Resolved **and** consumed | **118** |
| Resolved but ignored | **0** |
| Not resolved at all | **23** |
| **Total clientbound-play** | **141** |

The 23 gaps, by class:

| Class | Count | Share of the gap |
|---|---|---|
| **A** pure state, no rendering | **0** | 0% |
| **B** needs rendering | **0** | 0% |
| **C** needs a subsystem Rewo lacks | **12** | 52% |
| **D** not applicable | **11** | 48% |

**M87 was the first bite out of class C** — which has since gone 23 -> 12, the
rest of it taken by M91 (the furnace family), M93s (the stonecutter), M93u (the
merchant), M93y (the recipe book's decode), M108 (`delete_chat`), M113 (the
Brigadier tree) and M114 (the two suggestion packets). It is a worked example of what
that class costs. `open_screen` and `container_set_data` are eleven lines of
decode between them; what made them class C is that neither means anything
without a *menu model* — the 25 slot layouts, and an `Inventory` that stops
being hard-wired to the player's 46 slots. The decode landed first and on its
own precisely because it is the small half: for a while the row above said
`handled` while nothing was visible — which is exactly the state the
"what is not" column exists to record, and which M87k/M88/M89 then closed.

**Both actionable classes are now empty**, which changes what this document is
for. It was written to enumerate what Rewo ignores; what remains is **12
packets that need a subsystem and 11 that do not apply** — §2's machine-checked
table is the authority, and this sentence said 16 until 2026-08-10, which is the
third time this narrative has drifted from the table above it. The subsystems are
a map-image pipeline, a resource-pack fetcher, a reconnect flow, an advancement
tree and a dialog framework; **the chat-input path was on that list and is no
longer**, since M110 built the `ChatScreen` and M114–M124 the command line. **The recipe book was on that list and is
no longer** — M93y decoded it, M93z–M107 built the UI (M107 closed it), and its
four packets are `handled`. Picking work
from here now means **choosing a subsystem**, not choosing a packet — and
`REWO_FEATURE_SURVEY.md` is the better guide to that than a class-C count is.
The one thing this document still adjudicates on its own is §4: "handled" is not
"complete", and six consumed packets decode less than their body carries.

M67 audited 56 / 0 / 85 with class A at 31. **Sixty-two** packets separate
that published 56 from this 118, and **ten of them had already landed when M67
published** (§8) — three from M67's own sibling work, three from M68, three
from M69, and `set_passengers` from the M68/M70/M72 trio. Six are M74's, one is
M75's, three are M76's, three are M77's, eight are M78's, seven are M79's, six are M80's, three are
M81's, and one each from M82, M83 and M85.

**Class A and class B are both exhausted** (this heading said "class B is down
to one screen" until 2026-08-10; M84 closed it, and §0 item 2 had said so). M76
took the last
three pure-state gaps (`player_rotation`, `player_look_at`,
`set_default_spawn_position`, §3), and the five milestones after it took eighteen
of class B's twenty. What they established is worth keeping: **the class letter
changes the gate, not the standard.** M79's seven have an exact vanilla oracle
(`Gui.renderTitle`, `ExperienceBar`, `ContextualBar.extractExperienceLevel`,
`ItemCooldowns`, `GuiGraphicsExtractor.itemCooldown`) and M80's six share one
(`WorldBorder`), so decode and render alike are transcribed line by line and
graded by an oracle, with a pixel read-back half on top of the model half. A
class-B packet is not a guess; it is a transcription that happens to need a
renderer to land. M80 also shows why the *unit* matters: its six are one feature
with one state machine, one physics consequence and one wall, and splitting the
decode from the render would have left the state machine untestable against
anything.

The last of them was `award_stats` (3), the statistics screen, taken by **M84**
in parallel with M85 — so **class B is empty**: every clientbound-play packet
that needs a renderer is rendered, and the whole remaining gap is class C (a
subsystem Rewo lacks) and class D (not applicable). This paragraph was written
while that milestone was still in flight and read "the one that remains … when it
lands" for six days after it landed.

**M82 took the third screen, `player_combat_kill` (68), and with it the
framework the other two were waiting on.** What this document said about them —
"those genuinely change character, because a screen framework is a design
decision rather than a transcription" — turned out to be **half right and half
backwards**. The design decision was real and it was smaller than expected:
vanilla has *one* screen slot, not a stack, so most of what a framework looked
like it needed was not there to build. The rest was an ordinary transcription
(`AbstractWidget`, `WidgetSprites`, `ContainerEventHandler`'s routing), and it
produced the usual crop of inverted readings — a hovered *disabled* button
draws the plain disabled sprite, and `Esc` on a death screen does nothing at
all. `award_stats` and `server_links` are now class-B transcriptions like any
other: `rewo_world::screen` is the shared part and
`rewo_world::death_screen` is the worked example of a consumer.

**M85 took `server_links` (137), and it needed two more framework pieces plus
three corrections to what this document said the packet was for.** The
corrections first, because each one changes what has to be built:

* **The pause screen shows one button, not a list.** `getCustomAdditions`
  returns `Dialogs.SERVER_LINKS` when the list is non-empty, and
  `addCustomDialogButtons` makes that a single 204-wide button that opens a
  separate `ServerLinksDialogScreen`. Three screens, not one.
* **The disconnect screen shows at most one link, and only ever `BUG_REPORT`** —
  through `DisconnectionDetails.bugReportLink`, filled *only* on the client's
  own error paths. `DisconnectedScreen` never mentions `ServerLinks`, and a
  server that kicks you politely shows no link however many it advertised.
* **The packet exists in configuration too**, so this row covers two ids. Third
  time (M69 `update_tags`, M78 `custom_payload`/`store_cookie`), and the pattern
  is now reliable enough to check first: if the handler is on
  `ClientCommonPacketListener`, look for the configuration copy.

The framework pieces: `blitNineSlicedSprite` (M82 named it as a gap and its
`p11` witness asserted the skip; a 310-wide dialog button is what needed it) and
a transcription of `GridLayout` / `LinearLayout` / `FrameLayout` /
`HeaderAndFooterLayout`, because the pause and dialog screens are laid out
rather than positioned by arithmetic the way `DeathScreen` is.

---

## §3 The three most valuable gaps

Ranked by *how badly the failure hides*, not by how visible the feature is.
M67's original top three — the velocity/mount cluster, the inventory's
authoritative writes, and `update_tags` — are all closed, by M68, M69 and M69
respectively. **M78 closed two of the three that replaced them**
(`bundle_delimiter` and `disguised_chat`, kept below as worked examples,
because what each was ranked *for* is what the milestone had to get right).
All three are closed now; the section is kept because what each was ranked
*for* is what the milestone had to get right.

### 1. `bundle_delimiter` (0) — closed, M78

Empty body, and the cheapest row in the table. `ClientboundBundlePacket` wraps
a run of packets between two delimiters, and the delimiter is how the client
knows where the run ends.

Rewo applied each packet as it drained the socket. The server bundles precisely
the things that are wrong apart: an `add_entity` with its `set_entity_data`,
its `set_equipment` and its `update_attributes`. Split across frames, a mob
renders for a frame or two as an un-named, un-equipped, default-metadata
version of itself — a slime at size 1, an armour stand with no armour, a
zombie with no held item — and then pops into correctness.

That is indistinguishable from a renderer bug, which is what made it worth
ranking first: every other gap in this section announces itself as *nothing
happening*, and this one announces itself as the wrong subsystem.

**One phrase above was wrong and M78 corrected it**: the entry used to say
vanilla applies the run "in one tick". It does not — `handleBundlePacket` is a
plain `for` loop on one **scheduled task**, and the guarantee that falls out is
that no *frame* is rendered part-way through. The distinction matters because
the tick reading suggests a client should defer a bundle to its next tick
boundary, which vanilla never does. §9.

### 2. `player_rotation` (73) and `player_look_at` (71) — **closed, M76**

Kept because the *shape* of the failure is the lesson, and because two of the
three things this section said about them were wrong.

`player_position` (72) was handled and its rotational twin was not, so a server
that moved your body worked and one that turned your head did nothing. The
failure was silent and one-sided, and the working half is what made it hard to
find: the natural diagnosis is "teleports work, so it isn't the teleport path".

**What this section got wrong.** It said `handleRotatePlayer` sets yaw and pitch
"with per-axis relative flags, the same `RelativeMovement` set the positional
teleport uses". The *semantics* are shared — `handleRotatePlayer` builds a
`Set<Relative>` and calls the same `calculateAbsolute` — but the **wire layout
is not**: `ClientboundPlayerRotationPacket` is `FLOAT, BOOL, FLOAT, BOOL`,
ten fixed bytes with each flag *after* the float it qualifies, and carries no
packed mask at all. A reader written from this paragraph would have consumed
the yaw's four bytes as the mask. The handler is also named `handleLookAt`, not
`handlePlayerLookAt`.

Both were class A and both were left alone by M74 because they write
`rewo_world::physics::PlayerState`, which M75 owned while it landed flight and
no-clip (§4.1). M75 landed; M76 took them.

### 3. `disguised_chat` (33) — closed, M78

`Component` plus a `ChatType.Bound`, and Rewo already renders chat lines, so
this was decode-and-append with no new subsystem. What sends it is
`/msg`-style and plugin-routed chat — `disguised_chat` is what a server uses
when a message should render as chat without carrying a player signature.

So on a plugin-driven server an entire category of message never appeared, with
no error, no log line and nothing missing from the screen to point at. It
ranked here for the same reason M67 ranked `update_tags` first: the client is
not broken in any way an observer can name.

"Decode-and-append with no new subsystem" turned out to understate the decode
by one field: the `ChatType.Bound` opens with `ByteBufCodecs.**holder**`, whose
`0` means *an inline `ChatType` follows* — two whole `ChatTypeDecoration`s —
rather than chat type 0. A `holderRegistry` reading, which is the convention
the adjacent dimension holder uses, would have read the first decoration's
translation key as the sender's name. §9.

**Honourable mention, also closed by M78:** `store_cookie` (120) was the other
half of a loop Rewo already answered. `cookie_request` was resolved and replied
to — with nothing, because nothing ever stored a cookie. On a network that uses
`transfer` between backends that is a session that silently forgets itself on
every hop.

---

## §4 "Handled" is not "complete"

`ids.rs` cannot express *partially*, and neither can the machine check in §1 —
it proves a dispatch arm exists, not that the arm reads the whole body. These
are the known partials, found by reading the arms. Listed because a future
audit run by the same instruments will call every one of them handled.

**`game_event` closed in M71**, **`chunk_batch_finished` closed in M74** and
**`cookie_request` closed in M78**; all three are kept below as worked examples
of the class, because the "what is not" column is what turned each of them into
a milestone.

**`cookie_request` is also a new *shape* of partial, and worth stating
separately: the two play loops disagreed.** `Connection::run_play` (the M1-era
`rewo net` / `rewo view` harness) answered it; `PlaySession` — the loop `rewo
play` and `rewo live` actually run — had no arm for it at all. The machine
check in §1 cannot see that, because it asks whether *anything* in `rewo-net`
dispatches the field, and one of the two loops did. Any packet handled in only
one of the two reads as fully handled.

| Packet | What is consumed | What is not |
|---|---|---|
| `game_event` (38) — **closed, M71** | All fourteen types, via `rewo_net::game_event`. Ten are applied: the four weather levels, `CHANGE_GAME_MODE` → `ClientGameState::game_mode`, the `IMMEDIATE_RESPAWN` / `LIMITED_CRAFTING` flags, `WIN_GAME` / `DEMO_EVENT` / `LEVEL_CHUNKS_LOAD_START` as markers, `NO_RESPAWN_BLOCK_AVAILABLE` as a queued translation key, and the three local sounds. | **`GUARDIAN_ELDER_EFFECT`'s particle** — `ParticleTypes.ELDER_GUARDIAN` is not one of M37's six transcribed kinds, and M37's rule is that an unknown kind is dropped rather than rendered as something else. The gamemode is **modelled, not acted on** (§4.1). `WIN_GAME` and `DEMO_EVENT` open screens Rewo has no screen system for. |
| `chunk_batch_finished` (11) — **closed, M74** | The `batchSize` **VarInt**, folded into `ChunkBatchSizeCalculator` along with `chunk_batch_start`'s clock stamp, and the reply is now `getDesiredChunksPerTick()`. | Nothing. Recorded because the row it used to have was **wrong in two ways**: it called the unread field "the `batchSize` float" (it is a VarInt — the *float* in this exchange is the serverbound reply's `desiredChunksPerTick`), and it classed a hard-coded `p.f32(64.0)` as an unread field when it was a **live flow-control divergence**: vanilla's seeded opening bid is `3.5`, so Rewo over-bid the server ~18× on every batch of every session and never adapted. See §8. |
| `explode` (36) | The physics prefix — position and the `Optional<Vec3> playerKnockback` that M68 applies to the local player. | The particle / sound / weighted-block-list tail, deliberately: the particles need M37 kinds Rewo has not transcribed and the sound needs playback, which M63 scoped out. `crate::motion::read_explode` returns how many bytes it used precisely so this stays honest. |
| `level_event` (46) | **Both halves.** The particle half through M37's `route_level_event`, and since **M140** the sound half through `route_level_event_sound` — the block CENTRE (`Level.java:475`), the `data` gates, and the per-row volume. | The four camera- and listener-placed ids of seventy, which need the camera the decode layer does not have; and `distance_delay`, which vanilla defers by `distance / 340` s and Rewo's queue cannot express, so those arrive early rather than not at all. |
| `block_changed_ack` (4) | The id. The arm is a `log::debug!`. | The sequence number and the block-prediction rollback it acknowledges — Rewo does not predict block changes, so there is nothing to roll back yet. |
| `container_set_content` / `container_set_slot` (18 / 20) — **closed, M87** | Both targets. `container_target` picks the player's inventory or the open menu, and `apply_container_set_content` takes an `expect_container` where it once hard-coded 0 — one dispatcher, because `handleContainerContent` is `if id == 0 { inventoryMenu } else if id == containerMenu.containerId { containerMenu }` and splitting it would put two seams on one packet id. | Nothing. Kept because M34's stated reason — "there is no screen to put them in" — outlived the wall it justified: M87k drew the panel, M88's r19/r20 witness it in the windowed client, M89 made it clickable. `set_player_inventory` (108) and `set_cursor_item` (96) are M69 and no longer share the old rule. |
| `player_info_update` (70) | `ADD_PLAYER`, `UPDATE_GAME_MODE`, `UPDATE_LISTED`, `UPDATE_LATENCY`, `UPDATE_LIST_ORDER`, `UPDATE_HAT`, and the walk past the rest. | `UPDATE_DISPLAY_NAME` and `INITIALIZE_CHAT` are walked and discarded rather than stored. The walk is correct — M62 unified it into one function after finding a drifted copy — but the values do not survive it. |
| `move_minecart_along_track` (55) — M77 | The whole body, and the whole client-side `NewMinecartBehavior` schedule it feeds. The first handler guard, `instanceof AbstractMinecart`, is enforced. | The **second** handler guard, `getBehavior() instanceof NewMinecartBehavior`. It is not a class fact — the constructor picks the behaviour from `level.enabledFeatures().contains(MINECART_IMPROVEMENTS)` — and `update_enabled_features` is a **configuration** packet, outside this survey's scope and not in `ids.rs`. It is also structurally unreachable: the only sender of this packet is `handleMinecartPosRot`, which `ServerEntity` reaches down the same `instanceof` branch. Decoding the flag set is the follow-up. |
| `set_entity_link` (100) — M77 | Both i32s, and the holder id onto the leashed entity. | **The rope.** Class B, and unstarted. Also `getLeashHolder`'s *cache*: vanilla promotes the delayed id to an `Entity` reference once and keeps it, so a holder that leaves the tracking range stays attached until the server re-sends; Rewo resolves on demand and reports none. Nothing reads it yet. |
| `disguised_chat` (33) — **closed, M127** | The whole body, and the **decoration**: `parse_registry_data` captures `minecraft:chat_type`, the `ChatType.Bound` resolves against it (or carries its own decoration, in the inline branch), and `boundChatType.decorate` builds the `Component.translatable` the chat store renders. `/msg` reads "X whispers to you: …" in GRAY italic. | Nothing. Kept as a worked example: the partial survived from M78 to M126 and its *stated* blocker outlived the wall by two milestones — M125 gave `PlaySession` the language table and M126 made a chat line a list of styled spans, and this row still said "`parse_registry_data` does not capture" it, which was true of the code and no longer true of the difficulty. `player_chat` (65) carried exactly this partial from M7 and closed with it. |
| `server_data` (86) | Both fields — the MOTD flattened, the icon as bytes. | `ServerData.validateIcon` (a PNG parse capped at 1024×1024, where an invalid icon leaves the previous one alone), and vanilla's `serverData != null` guard, which records only for a session started from the multiplayer server list. Rewo has no server list and no PNG decoder in `rewo-net`. |
| `cookie_request` (21) — **closed, M78** | The key, and a reply carrying the jar's payload for it. Answered by **both** play loops now. | Nothing. Recorded because of *how* it was partial: `Connection::run_play` answered it and `PlaySession` had no arm, so the client that matters left it unanswered while the table said `handled`. See the note above this one. |

### §4.1 What wiring the gamemode to physics would actually take

M71 models `CHANGE_GAME_MODE` and stops there, because acting on it is a
larger job than the packet suggests. `MultiPlayerGameMode.setLocalMode` ends in
`GameType.updatePlayerAbilities(abilities)`, which writes four booleans —
`mayfly`, `instabuild`, `invulnerable`, `flying` (and note **SPECTATOR sets
`flying = true` while CREATIVE only sets `mayfly`**, so entering creative does
not start you flying). Rewo has **none of those concepts**. The work is:

1. An abilities struct on `PlayerState`, and `GameType::updatePlayerAbilities`
   transcribed onto it.
2. A flight branch in `rewo_world::physics::tick` — vanilla's creative flight
   is its own velocity model, not gravity with a different constant.
3. Spectator no-clip: `physics` currently always consults `baked.solid`.
4. `player_abilities` (clientbound **and** serverbound) — neither is in
   `ids.rs`. The server is authoritative about `mayfly`, and the client must
   send its flying state back or the server rubber-bands it.

**All four shipped in M75**, which is why M74 left `player_rotation` and
`player_look_at` alone despite ranking them second in §3: they write the same
`PlayerState`. **M76 took them once M75 landed.**

Two other authoritative sources also feed this state and are not wired: the
**login packet** carries `showDeathScreen` and `doLimitedCrafting` (the
`game_event` ids 11/12 are only the mid-session gamerule change), and
**`spawn_info`** carries `gameType` + `previousGameType` on both login and
respawn — `crates/rewo-net/src/spawn_info.rs` already decodes both fields and
nothing consumes them. `handleRespawn` also copies `showDeathScreen` onto the
new player but **not** `doLimitedCrafting`, so that one resets in vanilla too.

---

## §5 The full table

`handled` = resolved in `ids.rs` **and** dispatched. `absent` = not in
`ids.rs`, with its class from §1. Machine-checked — see §1 for the grammar.

| id | packet | status | resolution / class | note |
|---:|---|---|---|---|
| 0 | `bundle_delimiter` | handled | `req!` → `cb_play_bundle_delimiter` | **M78.** Empty body, and **not an inert packet** — `BundleDelimiterPacket.handle` throws. Consumed by `crate::bundle` *before* dispatch, so the `else if` ladder never learns bundles exist. §9. |
| 1 | `add_entity` | handled | `req!` → `cb_play_add_entity` | |
| 2 | `animate` | handled | `req!` → `cb_play_animate` | M19/M20 — combat swings. |
| 3 | `award_stats` | handled | `req!` → `cb_play_award_stats` (M84) | The statistics screen. Both levels of `Stat.STREAM_CODEC`'s dispatch are `ByteBufCodecs.registry`, i.e. a raw VarInt, so the walk is total for a stat type it has never seen — the `DataComponentPatch` hazard does not apply. |
| 4 | `block_changed_ack` | handled | `opt!` → `cb_play_block_ack` | §4 partial — the arm is a log line. |
| 5 | `block_destruction` | handled | `req!` → `cb_play_block_destruction` | **M81.** The crack overlay. The stage byte is **unsigned**, so the server's `(byte) -1` arrives as 255 and the removal is the range test failing, not a sentinel. |
| 6 | `block_entity_data` | handled | `req!` → `cb_play_block_entity_data` | |
| 7 | `block_event` | handled | `req!` → `cb_play_block_event` | |
| 8 | `block_update` | handled | `req!` → `cb_play_block_update` | |
| 9 | `boss_event` | handled | `req!` → `cb_play_boss_event` | |
| 10 | `change_difficulty` | handled | `req!` → `cb_play_change_difficulty` | **M74.** A VarInt `Difficulty` id through a **WRAP** out-of-bounds map — a third enum convention, neither `readEnum` nor `ByIdMap ZERO` — then a bool. |
| 11 | `chunk_batch_finished` | handled | `req!` → `cb_play_chunk_batch_finished` | **M74** consumed the `batchSize` VarInt and fixed the reply. §4. |
| 12 | `chunk_batch_start` | handled | `req!` → `cb_play_chunk_batch_start` | **M74.** Empty body. Stamps the clock the batch-size calculator measures against; without it the reply is a constant, not an estimate. |
| 13 | `chunks_biomes` | handled | `opt!` → `cb_play_chunks_biomes` | |
| 14 | `clear_titles` | handled | `req!` → `cb_play_clear_titles` | **M79.** One boolean, and it does something the clear itself does not: `clearTitles()` drops the text and zeroes the countdown, and only `resetTimes` puts the three **durations** back to 10 / 70 / 20. So `/title clear` and `/title reset` differ in what the *next* title does, not in what is on screen. |
| 15 | `command_suggestions` | handled | `req!` → `cb_play_command_suggestions` | **M114.** The autocomplete reply: a request id, a `start`/`length` span, and a list of `(text, optional component tooltip)`. `toSuggestions` builds its list with the **constructor**, not `Suggestions.create`, so the server's order is shown verbatim and duplicates are not removed — routing it through `create`, which is what the rest of the subsystem does, would silently re-order every server's autocomplete. Matched against a **single** outstanding request id, so a reply to a superseded one is dropped; vanilla's idle sentinel for that id is `-1`, which is also a legal VarInt, so a server sending `-1` while nothing is pending dereferences a null future in vanilla and is inert here. |
| 16 | `commands` | handled | `req!` → `cb_play_commands` | **M113.** A counted list of nodes then the root index. **An argument node's properties have no length prefix and only its own type knows their size**, so an unknown `command_argument_type` id makes the rest of the packet unreadable — vanilla's reader returns `null` *without consuming them* and then reads the next node from the wrong offset, so Rewo makes it a decode error. 44 of the 57 types are `SingletonArgumentInfo` and read nothing; the other 13 are transcribed. The suggestion id is read **after** the properties, not beside its flag. |
| 17 | `container_close` | handled | `req!` → `cb_play_container_close` | **M74.** One VarInt container id, which vanilla reads and then **ignores** — `handleContainerClose` closes whatever is open without comparing ids. |
| 18 | `container_set_content` | handled | `req!` → `cb_play_container_set_content` | **M34 + M87.** Addresses either menu via `container_target`; the id is a parameter, not a hard-coded 0. |
| 19 | `container_set_data` | handled | — | M87 decode, **M91/M92 consumers**. VarInt container id then **two signed `readShort`s**, applied only when the id matches the open menu. Every menu that sends it now draws from it: the three furnaces (flame + arrow), the brewing stand (fuel bar, brew arrow, bubbles), the enchanting table (three row states + numerals + cost text) and the beacon (button chrome + effect icons). The slot *meanings* are per-menu and invert against each other — the furnace puts fuel at 0 and the brewing stand puts its tick counter there, and the beacon encodes bsent\ as 0 with ids shifted up by one where the enchanting table uses -1. Negative values are the normal case for the enchanting clues, which is why the shorts must be signed. |
| 20 | `container_set_slot` | handled | `req!` → `cb_play_container_set_slot` | **M34 + M87.** Same routing as 18. Its index is a **signed short** among the var-ints. |
| 21 | `cookie_request` | handled | `opt!` → `cb_play_cookie_request` | Answered from the jar `store_cookie` (120) fills, since **M78**. Before it, "handled" was true only of the M1-era `Connection::run_play` harness — `PlaySession` had no arm at all, so the real client left a play-state request unanswered. §4, §9. |
| 22 | `cooldown` | handled | `req!` → `cb_play_cooldown` | **M79.** An `Identifier` **cooldown group** + a VarInt duration — no start tick, no end tick; `addCooldown` supplies the start from `ItemCooldowns.tickCount`. `duration == 0` routes to `removeCooldown`, so it **cancels** rather than starting a zero-length cooldown whose percent would be `0/0`. |
| 23 | `custom_chat_completions` | handled | `req!` → `cb_play_custom_chat_completions` | **M114.** An `Action` ordinal then a counted string list. `readEnum` indexes an array, so an out-of-range ordinal is a **decode error**, not a defaulted `ADD` — M65's strict convention. `SET` **clears before it adds**, so it is not `ADD` on an empty set. The result is unioned with the online-player names by `getCustomTabSuggestions`, deduped, and is what makes plain-chat Tab completion work without any parser. |
| 24 | `custom_payload` | handled | `req!` → `cb_play_custom_payload` | **M78.** An unknown identifier is **discarded, not rejected**, and the fallback consumes the remainder. The copy a vanilla server actually sends is **configuration 1** — see §9. |
| 25 | `damage_event` | handled | `req!` → `cb_play_damage_event` | |
| 26 | `debug/block_value` | absent | **D** | Debug subscription — sent only to a client that subscribed via `debug_subscription`, which Rewo never will. |
| 27 | `debug/chunk_value` | absent | **D** | As above. |
| 28 | `debug/entity_value` | absent | **D** | As above. |
| 29 | `debug/event` | absent | **D** | As above. |
| 30 | `debug_sample` | absent | **D** | Remote tick/ping profiler samples for vanilla's F3 chart; requires a serverbound subscription first. |
| 31 | `delete_chat` | handled | `req!` → `cb_play_delete_chat` | **M108.** One `MessageSignature.Packed`, and unreadable without a `MessageSignatureCache`: `Packed.read` is `readVarInt() - 1`, so wire `0` means 256 inline bytes and anything else is an **index into the client's own cache**, fed by every `player_chat` on receipt. Resolving to an empty slot is a no-op; an out-of-range id is too, where vanilla's unchecked `entries[id]` throws. |
| 32 | `disconnect` | handled | `req!` → `cb_play_disconnect` | |
| 33 | `disguised_chat` | handled | `req!` → `cb_play_disguised_chat` | **M78, completed M127.** Decoded whole and **decorated**. Its `ChatType.Bound` opens with `ByteBufCodecs.**holder**` (`id + 1`, `0` = inline), not `holderRegistry` — and the inline branch carries two whole `ChatTypeDecoration`s, which M127 reads rather than walks past. |
| 34 | `entity_event` | handled | `req!` → `cb_play_entity_event` | M17. |
| 35 | `entity_position_sync` | handled | `req!` → `cb_play_entity_position_sync` | |
| 36 | `explode` | handled | `req!` → `cb_play_explode` | **M68.** §4 partial — the physics prefix only; the particle/sound/weighted-list tail is deliberately unconsumed. |
| 37 | `forget_level_chunk` | handled | `req!` → `cb_play_forget_chunk` | |
| 38 | `game_event` | handled | `req!` → `cb_play_game_event` | All 14 types since M71. Was 4 of 14 — the §4 worked example. |
| 39 | `game_rule_values` | handled | `req!` → `cb_play_game_rule_values` | **M78.** A counted map of `Identifier` → string, kept wholesale. Vanilla has **no store** — the map goes to a screen if one is open and nowhere otherwise — so replacement is the reading, not merge. |
| 40 | `game_test_highlight_pos` | absent | **D** | Game-test tooling. |
| 41 | `mount_screen_open` | absent | **C** | Horse/nautilus inventory screen. |
| 42 | `hurt_animation` | handled | `req!` → `cb_play_hurt_animation` | **M81.** The damage camera yaw-tilt — and with it, `M52a`'s vacuous `no_damage_tilt` module became real. §0 item 6. |
| 43 | `initialize_border` | handled | `req!` → `cb_play_initialize_border` | **M80.** The whole border at once. Its `lerpTime > 0` guard picks `setSize(newSize)` over `lerpSizeBetween` — the guard `set_border_lerp_size` (89) does *not* have. |
| 44 | `keep_alive` | handled | `req!` → `cb_play_keep_alive` | |
| 45 | `level_chunk_with_light` | handled | `req!` → `cb_play_level_chunk` | |
| 46 | `level_event` | handled | `opt!` → `cb_play_level_event` | M37 (particles) + **M140** (sounds). Both halves of the id table are consumed; §4 records the two boundaries that remain — camera-placed ids and `distance_delay`. |
| 47 | `level_particles` | handled | `opt!` → `cb_play_level_particles` | M37. |
| 48 | `light_update` | handled | `opt!` → `cb_play_light_update` | |
| 49 | `login` | handled | `req!` → `cb_play_login` | |
| 50 | `low_disk_space_warning` | absent | **D** | `Minecraft.sendLowDiskSpaceWarning` — the integrated server warning about its own save directory. |
| 51 | `map_item_data` | absent | **C** | Map-item colour patches + decorations; needs a map image pipeline and a map renderer. |
| 52 | `merchant_offers` | handled | — | Villager trade list (M93u). **The class-C label was wrong** — the packet needed nothing Rewo had not already built: `ItemStack` (M34/M41) and the `TypedDataComponent` walker M52e wrote for `can_place_on`. Unlike M91/M93s, the *data* really is server-rolled and off-wire; what was wrong was that decoding it needed a new subsystem. |
| 53 | `move_entity_pos` | handled | `req!` → `cb_play_move_entity_pos` | |
| 54 | `move_entity_pos_rot` | handled | `req!` → `cb_play_move_entity_pos_rot` | |
| 55 | `move_minecart_along_track` | handled | `req!` → `cb_play_move_minecart_along_track` | **M77.** The **only** movement channel an experimental-movement cart has: `ServerEntity.sendChanges` routes such a cart down `handleMinecartPosRot` instead of the generic position branch entirely, so it is never sent `move_entity_pos` / `teleport_entity` / `entity_position_sync`. The steps are two **full-double** `Vec3`s each (`Vec3.STREAM_CODEC`, not `LP_STREAM_CODEC`), two rotation bytes and an f32 weight. The second client guard (`getBehavior() instanceof NewMinecartBehavior`) is **not** enforced — it depends on the `minecart_improvements` feature flag, which needs `update_enabled_features` (configuration; out of this survey's scope). |
| 56 | `move_entity_rot` | handled | `req!` → `cb_play_move_entity_rot` | |
| 57 | `move_vehicle` | handled | `req!` → `cb_play_move_vehicle` | **M68.** Carries **no entity id** — the client resolves `getRootVehicle()`. Sent only as a rejection of a serverbound vehicle move, so a passenger-only client never receives one. |
| 58 | `open_book` | absent | **C** | Book screen. |
| 59 | `open_screen` | handled | — | M87. VarInt container id, the `minecraft:menu` id as a **raw 0-based** `registry` (not `holder`'s `id + 1`), then an NBT title. All 25 layouts resolve; an unregistered type opens nothing, as `MenuScreens.create` does. M87k/M88/M89 render it: the packet opens the screen, the panel is the menu's own (r19/r20 in `live --render-check`), and clicks route to the shown menu. |
| 60 | `open_sign_editor` | absent | **C** | Sign edit screen. |
| 61 | `ping` | handled | `req!` → `cb_play_ping` | |
| 62 | `pong_response` | absent | **D** | The reply to a serverbound `ping_request` Rewo never sends (`pingDebugMonitor`). |
| 63 | `place_ghost_recipe` | handled | — | Decoded in M93y and **drawn in M103** — the ghost item sandwiched between a red wash under and a white wash over, only the lower of which widens for a big result slot. The book itself was built by M93z–M107. |
| 64 | `player_abilities` | handled | — | Flags byte + `flyingSpeed` + `walkingSpeed`, nine fixed bytes. Landed by **M75** with the flight / no-clip physics it feeds and the `GameType` binding M71 left unstarted. The **serverbound** twin is one byte carrying only `FLAG_FLYING` — writing the clientbound body there desyncs the stream by eight. |
| 65 | `player_chat` | handled | `opt!` → `cb_play_player_chat` | |
| 66 | `player_combat_end` | handled | `req!` → `cb_play_player_combat_end` | **M78.** Vestigial: the handler is an **empty method**, so nothing is stored — inventing a field would be a divergence dressed as decode-and-state. The body is **not** empty (a VarInt `duration`), and the only gradeable property is that the reader consumes exactly it. §9. |
| 67 | `player_combat_enter` | handled | `req!` → `cb_play_player_combat_enter` | **M78.** Vestigial, empty handler, and `StreamCodec.unit` — **zero** bytes. Graded the same way as its sibling: reader position, which here means reading nothing at all. §9. |
| 68 | `player_combat_kill` | handled | `ids.rs` + `route_player_combat_kill` | M82 — the death screen, and the screen framework under it. VarInt `playerId` + a `TRUSTED_STREAM_CODEC` message; the id is always your own, so it resolves against the local-player door (§0.0 gotcha 13), never the entity table. |
| 69 | `player_info_remove` | handled | `req!` → `cb_play_player_info_remove` | |
| 70 | `player_info_update` | handled | `req!` → `cb_play_player_info_update` | §4 partial — display name and chat session are walked and discarded. |
| 71 | `player_look_at` | handled | `req!` → `cb_play_player_look_at` | **M76.** `/teleport … facing`. An anchor `readEnum`, three doubles, a flag, and **only then** a conditional entity + anchor pair. An unresolvable entity falls back to the packet's own coordinates, which are the sender's snapshot of `toAnchor.apply(entity)` — not a placeholder. |
| 72 | `player_position` | handled | `req!` → `cb_play_position` | The positional teleport. Its rotational twin (73) landed in **M76**, closing §3's asymmetry. |
| 73 | `player_rotation` | handled | `req!` → `cb_play_player_rotation` | **M76.** Ten fixed bytes: `FLOAT yRot, BOOL relativeY, FLOAT xRot, BOOL relativeX` — **two interleaved booleans, not** the packed `Relative` mask 72 carries. It is the only one of the two that answers the server. |
| 74 | `recipe_book_add` | handled | — | Decoded into the session's recipe map (M93y). `replace` CLEARS it first. |
| 75 | `recipe_book_remove` | handled | — | Decoded (M93y). |
| 76 | `recipe_book_settings` | handled | — | Decoded (M93y). Four positional open/filter pairs. |
| 77 | `remove_entities` | handled | `req!` → `cb_play_remove_entities` | |
| 78 | `remove_mob_effect` | handled | `req!` → `cb_play_remove_mob_effect` | |
| 79 | `reset_score` | handled | `req!` → `cb_play_reset_score` | M65. |
| 80 | `resource_pack_pop` | absent | **C** | Server resource-pack pipeline (download, prompt, apply). Rewo loads a CEM pack from disk; it fetches none. |
| 81 | `resource_pack_push` | absent | **C** | As above. |
| 82 | `respawn` | handled | `req!` → `cb_play_respawn` | |
| 83 | `rotate_head` | handled | `req!` → `cb_play_rotate_head` | |
| 84 | `section_blocks_update` | handled | `req!` → `cb_play_section_blocks_update` | |
| 85 | `select_advancements_tab` | absent | **C** | Advancements screen. |
| 86 | `server_data` | handled | `req!` → `cb_play_server_data` | **M78.** MOTD flattened, icon kept as bytes. §4 partial — vanilla runs the icon through `ServerData.validateIcon` (a PNG parse capped at 1024²) and records only when the session came from the server list; Rewo does neither. |
| 87 | `set_action_bar_text` | handled | `req!` → `cb_play_set_action_bar_text` | **M79.** `setOverlayMessage(text, **false**)` — the animated rainbow belongs to `setNowPlaying` (a jukebox) and is unreachable from this packet. Its 60-tick clock and its `/20.0F` fade are constants, unrelated to `set_titles_animation`. |
| 88 | `set_border_center` | handled | `req!` → `cb_play_set_border_center` | **M80.** Two `f64`; moves the box without touching the size or a running lerp. |
| 89 | `set_border_lerp_size` | handled | `req!` → `cb_play_set_border_lerp_size` | **M80.** Two `f64` and a **var-long**. `handleSetBorderLerpSize` is unguarded, so a zero duration builds a *moving* extent with an infinite lerp speed. |
| 90 | `set_border_size` | handled | `req!` → `cb_play_set_border_size` | **M80.** One `f64`. `setSize` replaces the extent object, so it **cancels** an in-flight lerp rather than retargeting it. |
| 91 | `set_border_warning_delay` | handled | `req!` → `cb_play_set_border_warning_delay` | **M80.** One VarInt → `warningTime`, in **ticks** since 26.x (`WorldBorderWarningTimeFix` ×20). Same body as 92 and a different field; only the id separates them. |
| 92 | `set_border_warning_distance` | handled | `req!` → `cb_play_set_border_warning_distance` | **M80.** One VarInt → `warningBlocks`. Both warning packets feed one threshold, `max(warningBlocks, min(lerpSpeed × warningTime, remaining travel))`. |
| 93 | `set_camera` | handled | `req!` → `cb_play_set_camera` | **M74.** One VarInt entity id; an id the client cannot resolve leaves the camera **where it was**. Feeds `LabelViewer::camera_entity`, which M70 had hard-wired to the local player. |
| 94 | `set_chunk_cache_center` | handled | `req!` → `cb_play_set_chunk_cache_center` | M67. |
| 95 | `set_chunk_cache_radius` | handled | `req!` → `cb_play_set_chunk_cache_radius` | M67. |
| 96 | `set_cursor_item` | handled | `req!` → `cb_play_set_cursor_item` | **M69.** The server's authoritative carried stack — M35's predicted cursor had no other correction path short of a full resync. |
| 97 | `set_default_spawn_position` | handled | `req!` → `cb_play_set_default_spawn_position` | **M76.** `LevelData.RespawnData` = a dimension **identifier string** + a packed `BlockPos` long + two floats. Stored verbatim: `STREAM_CODEC` does not apply `RespawnData.of`'s wrap/clamp. A dimension change **resets** it to `(8, 64, 8)` of the new level — the opposite of the difficulty beside it, which `handleRespawn` copies across. |
| 98 | `set_display_objective` | handled | `req!` → `cb_play_set_display_objective` | M65. |
| 99 | `set_entity_data` | handled | `req!` → `cb_play_set_entity_data` | |
| 100 | `set_entity_link` | handled | `req!` → `cb_play_set_entity_link` | **M77.** Leash holder id, stored and **not drawn** — rendering the rope is still separate (B). Both fields are fixed big-endian **i32**s, not var-ints, and `destId == 0` is the wire's null. The cast is `instanceof Leashable`, an *interface*, so the gate is the union of `Mob`'s and `AbstractBoat`'s subtrees. |
| 101 | `set_entity_motion` | handled | `req!` → `cb_play_set_entity_motion` | **M68.** The body is `Vec3.LP_STREAM_CODEC` (`LpVec3`), **not** the legacy `short / 8000.0` fixed point, which no longer exists in 26.2. |
| 102 | `set_equipment` | handled | `req!` → `cb_play_set_equipment` | |
| 103 | `set_experience` | handled | `req!` → `cb_play_set_experience` | **M79.** The **wire order is not the declaration order**: the fields read progress / total / level and the reader is `readFloat, readVarInt **level**, readVarInt **total**`. Both are var-ints, so the swapped reading decodes without erroring and shows lifetime XP as the level. `totalExperience` has **no client reader at all** beyond the assignment. |
| 104 | `set_health` | handled | `opt!` → `cb_play_set_health` | |
| 105 | `set_held_slot` | handled | `req!` → `cb_play_set_held_slot` | |
| 106 | `set_objective` | handled | `req!` → `cb_play_set_objective` | M65. |
| 107 | `set_passengers` | handled | `req!` → `cb_play_set_passengers` | **M68 + M70 + M72** landed disjoint halves and the packet is now fully consumed: the riding graph for `isVehicle()`, the local player's mount state for physics, and every rider's position from its vehicle's PASSENGER attachment point. Remaining, and not about this packet: a mounted humanoid's seated leg pose, and the camel/horse animation-driven seat offsets. |
| 108 | `set_player_inventory` | handled | `req!` → `cb_play_set_player_inventory` | **M69.** An authoritative write addressed by **inventory index**, not menu slot — and the two armour ranges run in opposite directions, so a constant offset puts boots on the head. |
| 109 | `set_player_team` | handled | `req!` → `cb_play_set_player_team` | M62. |
| 110 | `set_score` | handled | `req!` → `cb_play_set_score` | M65. |
| 111 | `set_simulation_distance` | handled | `req!` → `cb_play_set_simulation_distance` | M67. |
| 112 | `set_subtitle_text` | handled | `req!` → `cb_play_set_subtitle_text` | **M79.** `setSubtitle` sets the field and **arms no clock** — `extractTitle` is gated on `title != null && titleTime > 0` and draws the subtitle inside that block, so a subtitle sent alone shows nothing. Same one-NBT-tag body as 114 and 87; only the id separates them. |
| 113 | `set_time` | handled | `req!` → `cb_play_set_time` | |
| 114 | `set_title_text` | handled | `req!` → `cb_play_set_title_text` | **M79.** The only one of the trio that arms `Hud.titleTime`, at the full `fadeIn + stay + fadeOut`. Drawn at **4× scale** centred on the screen, with the fade alpha as a *default* colour a span's own `color` replaces — keeping the caller's alpha, so a coloured title still fades. |
| 115 | `set_titles_animation` | handled | `req!` → `cb_play_set_titles_animation` | **M79.** Three **fixed big-endian i32s** (twelve bytes), each a per-axis no-op when negative — and the trailing `if (titleTime > 0)` **re-arms a live title at its full duration** rather than retiming the remainder. |
| 116 | `sound_entity` | handled | `req!` → `cb_play_sound_entity` | M63 — decode only, no playback. |
| 117 | `sound` | handled | `req!` → `cb_play_sound` | M63 — decode only, no playback. |
| 118 | `start_configuration` | handled | `opt!` → `cb_play_start_configuration` | |
| 119 | `stop_sound` | handled | `req!` → `cb_play_stop_sound` | M63. |
| 120 | `store_cookie` | handled | `req!` → `cb_play_store_cookie` | **M78.** The jar `cookie_request` answers from is now fillable, so the reply carries a payload instead of always writing `false`. The 5120-byte limit is an **error**, not a truncation. The **configuration** copy (id 10) is resolved too. §9. |
| 121 | `system_chat` | handled | `opt!` → `cb_play_system_chat` | |
| 122 | `tab_list` | handled | `req!` → `cb_play_tab_list` | M65. |
| 123 | `tag_query` | absent | **D** | The reply to a serverbound `/data get` query Rewo never sends. |
| 124 | `take_item_entity` | handled | `req!` → `cb_play_take_item_entity` | **M81.** The pickup animation — and the **removal**, which this row previously said arrived separately via `remove_entities`. It does not: `handleTakeItemEntity` shrinks the client's own copy of the stack and removes the entity itself. |
| 125 | `teleport_entity` | handled | `req!` → `cb_play_teleport_entity` | |
| 126 | `test_instance_block_status` | absent | **D** | Game-test tooling. |
| 127 | `ticking_state` | handled | `req!` → `cb_play_ticking_state` | **M74.** An f32 `tickRate` then a bool `isFrozen`. Decode and state only — the 20 Hz loop does not consult it yet. |
| 128 | `ticking_step` | handled | `req!` → `cb_play_ticking_step` | **M74.** One VarInt. |
| 129 | `transfer` | absent | **C** | Reconnect to another host — needs a transfer/reconnect flow. |
| 130 | `update_advancements` | absent | **C** | Advancements screen. |
| 131 | `update_attributes` | handled | `req!` → `cb_play_update_attributes` | M52/M73. |
| 132 | `update_mob_effect` | handled | `req!` → `cb_play_update_mob_effect` | M13. |
| 133 | `update_recipes` | absent | **C** | Recipe property sets + stonecutter recipes; recipe book / crafting. |
| 134 | `update_tags` | handled | `req!` → `cb_play_update_tags` | **M69.** The play copy is the `/reload` case; the join-time copy is **configuration 13**, and resolving only this one would have looked like it worked until someone reloaded. |
| 135 | `projectile_power` | handled | `req!` → `cb_play_projectile_power` | **M77.** A VarInt id (unlike `set_entity_link`'s fixed i32, one packet away) then an f64. Written onto `AbstractHurtingProjectile` only — an **arrow is not one**, it is an `AbstractArrow` on a sibling branch, so a `projectile_power` naming one mutates nothing. |
| 136 | `custom_report_details` | absent | **D** | Key/value metadata to attach to a crash report. |
| 137 | `server_links` | handled | M85 (`session`/`server_links`, both states) |
| 138 | `waypoint` | handled | M83 — the locator bar. |
| 139 | `clear_dialog` | absent | **C** | The dialog framework. |
| 140 | `show_dialog` | absent | **C** | The dialog framework. `Holder<Dialog>` over the **datapack** `minecraft:dialog` registry plus `Dialog`'s codec tree — resolvable in principle, **not verified in detail** here. |

---

## §6 What this audit does NOT verify

Stated explicitly, because the counts above are easy to over-read.

- **It does not verify that the 118 handled packets are decoded correctly.**
  It verifies that the id is resolved and that a dispatch arm tests it.
  Correctness of those 118 is what the `*shot --check` gates and the unit tests
  cover, and they cover it unevenly: `inventoryshot` is exhaustive about the
  container packets, while `player_info_update`'s walk is graded only by unit
  tests inside `rewo-net`.
- **It does not verify that the 118 are decoded *completely*.** §4 lists the
  known partials found by reading the arms; there may be more. Nothing
  mechanical distinguishes "consumed the body" from "read the first field" —
  and that includes the machine check in §1, which is why §4 exists.
- **It does not measure what a real server actually sends.** No capture was
  taken. A packet in the report may never appear on a vanilla connection
  (`game_test_highlight_pos`), and the ones that do appear are not weighted by
  frequency. The class-D reasons are read out of `ClientPacketListener`, not
  observed.
- **`show_dialog` (140)'s codec was not walked.** It is the one row whose
  resolvability is asserted rather than checked — see §1.
- **The classes are a judgement.** The A/B boundary in particular is a call
  about what a decode buys on its own; `set_experience` (B) and
  `projectile_power` (A, handled since M77) are the same shape on the wire and
  differ only in whether anything but a renderer would consume them.
- **Serverbound is out of scope.** The report lists 69 serverbound-play
  packets and this audit says nothing about them. Several class-A gaps above
  have serverbound halves that would also be needed to *act* on them
  (`move_vehicle`, `container_close`).
- **Configuration and login states are out of scope**, though both are fully
  resolved in `ids.rs` today. M69 records the cost of that scope: `update_tags`
  exists in **both** configuration (13) and play (134), and a play-only survey
  named only the second — the one a vanilla server sends *last*.
- **`rewo play`'s `CORRECTIONS 0` proves less than it is often cited as
  proving.** The harness walks on flat ground; M68 added a knockback and mount
  scenario, but the meter itself is one-sided — vanilla's move check flags a
  client that moves too **much**, and one that ignores a shove moves too
  **little**. Treat the number as "no correction *on the paths the harness
  exercises*".

---

## §7 What M67 decoded

Three packets, chosen for being small, self-contained, class A, and one
coherent thing rather than three unrelated ones: **the server's view area**.

| id | packet | body |
|---:|---|---|
| 94 | `set_chunk_cache_center` | two VarInts (chunk x, chunk z) |
| 95 | `set_chunk_cache_radius` | one VarInt |
| 111 | `set_simulation_distance` | one VarInt |

They land in `crates/rewo-net/src/view_area.rs` as one `ViewArea` struct on
`PlaySession`. **Decode and state only** — nothing unloads a column, throttles
a mesh, or gates an entity tick on them yet.

The four rules where a plausible implementation is silently wrong are
documented at their sites and each is mutation-tested:

1. **`calculateStorageRange(viewRange) = max(2, viewRange) + 3`.** A server
   radius of 2 still storages 5. Using the packet's number directly evicts
   columns the server still considers loaded, and does so only at small render
   distances — invisible at 12, wrong at 2.
2. **A server radius of `0` means "no cap", not "render nothing".**
3. **`inRange` is Chebyshev, not Euclidean** — a square.
4. **The login packet carries the initial pair**, and `read_login_prefix` now
   returns them from the walk it was already doing rather than a second copy.

### The mutation survivor was a real gap

13 witnesses, 16 deliberate mutations, **one survived**. Replacing
`read_simulation_distance`'s `r.varint()` with `r.varlong()? as i32` passed
every test: for any `i32` a server writes, a VarLong reader consumes the same
bytes and its low 32 bits are the same number. The two differ only on a
**malformed** body with a sixth continuation byte. So the arity witness was
measuring the field's *length in the happy case* and calling it "this is a
VarInt". Fixed by an explicit overlong-rejection witness — a pattern M74's new
VarInt readers each copy.

---

## §8 What M74 did — the re-derivation, and the drift

M74 re-derived §5 from the code rather than patching the rows it was told were
wrong, and then implemented six of the class-A gaps.

### The drift, and its mechanism

**Ten of M67's 141 rows were wrong**, all in one direction — `absent` about
code that was present:

| id | packet | M67 said | truth |
|---:|---|---|---|
| 36 | `explode` | absent | handled (M68) |
| 57 | `move_vehicle` | absent | handled (M68) |
| 94 | `set_chunk_cache_center` | absent | handled (M67 itself) |
| 95 | `set_chunk_cache_radius` | absent | handled (M67 itself) |
| 96 | `set_cursor_item` | `**M69**` | handled |
| 101 | `set_entity_motion` | absent | handled (M68) |
| 107 | `set_passengers` | `**M68 + M70 + M72**` | handled |
| 108 | `set_player_inventory` | `**M69**` | handled |
| 111 | `set_simulation_distance` | absent | handled (M67 itself) |
| 134 | `update_tags` | `**M69**` | handled |

The headline counts were wrong by the same ten: **56 / 85 published against
66 / 75 true**, with class A at 31 against a true 21.

**The mechanism is not neglect and not a misreading.** M67 wrote the table by
grepping, and four packets landed in `ids.rs` **the same day** — three of them
from M67's own sibling milestone M68. The audit was a *snapshot of a moving
tree*, and it began going stale within hours of being taken.

M67 saw this happening and worked around it in two ways, both of which made it
worse. It added an "After §7" column to §2 predicting where the counts would
land, so the published table described a moment that never existed. And it
wrote milestone markers (`**M69**`) into the *status* column for rows that had
landed since, which made the table unparseable and put four rows outside the
grammar any future check could read.

**That generalises**: any hand-maintained inventory of a live codebase decays
at the rate the codebase changes, and annotating the decay is not a fix. The
only durable answer is to make the document derive from the code, or fail when
it disagrees.

### The fix — `ids::tests::the_coverage_table_matches_the_code`

A unit test in `crates/rewo-net/src/ids.rs`, not a `*shot` gate command,
because it should fire on **the event that causes the drift** — someone
editing `ids.rs` — and `cargo test -p rewo-net` is what runs then. It reads
this document, `ids.rs` and the dispatch chain with `include_str!`, so it has
no runtime dependency on the datagen report, the network or the cwd.

Deliberately *not* a spot-check of a few rows: a spot-check has the same
failure mode as the grep it replaces. It recomputes all 141 statuses and both
count tables. §1 states the grammar it requires.

Its own limits, stated because the point of §6 is that counts are easy to
over-read: it verifies **status**, not correctness and not completeness. A
packet that is dispatched and half-decoded is `handled` to this test, which is
exactly the gap §4 is maintained by hand to cover.

### The six packets

Two of them are a behaviour fix and four are decode-and-state.

| id | packet | body | where |
|---:|---|---|---|
| 12 | `chunk_batch_start` | empty | `chunk_batch.rs` |
| 11 | `chunk_batch_finished` | one VarInt | `chunk_batch.rs` |
| 10 | `change_difficulty` | VarInt id + bool | `client_state.rs` |
| 17 | `container_close` | one VarInt | `client_state.rs` |
| 93 | `set_camera` | one VarInt | `client_state.rs` |
| 127 | `ticking_state` | f32 + bool | `ticking.rs` |
| 128 | `ticking_step` | one VarInt | `ticking.rs` |

**The chunk-batch pair is a live divergence, not a missing decode.** Rewo
replied `p.f32(64.0)`; vanilla replies `ChunkBatchSizeCalculator
.getDesiredChunksPerTick()`, seeded at `7e6 / 2e6` = **3.5**. Rewo therefore
over-bid the server by ~18× on the first batch of every session and never
adapted, and the server sizes its chunk batches to that number. Both halves
were needed: the calculator without `chunk_batch_start` has no interval to
measure and would only have produced a *differently* wrong constant.

**`Difficulty` is a third enum convention.** The project's notes record two —
`readEnum` (out-of-range is an error) and `ByIdMap.continuous(…, ZERO)`
(out-of-range is the zero value). `Difficulty.STREAM_CODEC` is
`ByteBufCodecs.idMapper` over `ByIdMap.continuous(…, **WRAP**)`, so a VarInt id
of 5 is `EASY` — where ZERO gives `PEACEFUL` and `readEnum` rejects the packet.
And WRAP is `Math.floorMod`, not `%`: a negative id is legal and indexes from
the far end, where Rust's `%` would panic.

**`container_close`'s id is read and ignored.** `handleContainerClose` is one
line with no comparison against `containerMenu.containerId`. Gating on it —
which is exactly what M34/M35 correctly do for `container_set_slot` — would
drop the packet whose only job is to close the screen.

**`set_camera` closes a stub M70 left behind.** `LabelViewer::camera_entity`
was hard-wired to `session.player_id` with a comment reading "Rewo never
detaches the camera"; this is the packet that detaches it. An unresolvable
entity id leaves the camera **where it was** rather than resetting to the
player — and the resolution must treat the local player's own id as valid,
because vanilla's `level.getEntity` finds the player and Rewo's `EntityTable`
never contains it. Without that clause the server could never hand the camera
back at the end of a spectate.

**Excluded on purpose:** `player_rotation` (73) and `player_look_at` (71),
despite ranking second in §3 — they write `rewo_world::physics::PlayerState`,
which the concurrent §4.1 milestone owns. (Both landed in **M76**, after M75
finished with that state.) `TickRateManager` is transcribed in
full, including `tick()`, but the session's 20 Hz loop does **not** consult it:
gating the loop would retime every existing harness and wants its own live
gate.

### One doc claim that the decompile contradicted

Writing `client_state.rs` I asserted in a doc comment that the login packet
carries the difficulty and that `NORMAL` was therefore a stand-in. It does not.
`ClientboundLoginPacket`'s fields are `playerId`, `hardcore`, `levels`,
`maxPlayers`, `chunkRadius`, `simulationDistance`, `reducedDebugInfo`,
`showDeathScreen`, `doLimitedCrafting`, the spawn info, `onlineMode` and
`enforcesSecureChat` — no difficulty. `handleLogin` writes
`new ClientLevelData(Difficulty.NORMAL, …)` with the constant in the source, so
`change_difficulty` is the **only** source and Rewo's default is vanilla's
literal rather than a guess.

It also turned up the reason not to reset it: `handleRespawn` builds its
replacement level data from `this.levelData.getDifficulty()`, carrying the
value across a dimension change. `ClientState` therefore lives on
`PlaySession` and is deliberately untouched by `apply_respawn` — the same rule
`ViewArea` follows, and for the same reason.

---

## §9 What M78 did — session, server metadata and chat

Eight packets, chosen as one coherent layer rather than eight unrelated rows:
everything the connection itself knows about the session it is in. Seven are a
reader plus a field and live in `crates/rewo-net/src/session.rs`; the eighth
changes how packets are *applied* and lives in `crates/rewo-net/src/bundle.rs`.

| id | packet | body | where |
|---:|---|---|---|
| 0 | `bundle_delimiter` | empty | `bundle.rs` |
| 24 | `custom_payload` | `Identifier` + a payload chosen by it | `session.rs` |
| 33 | `disguised_chat` | `Component` + `ChatType.Bound` | `session.rs` |
| 39 | `game_rule_values` | counted map, `Identifier` → string | `session.rs` |
| 66 | `player_combat_end` | one VarInt | `session.rs` |
| 67 | `player_combat_enter` | **empty** | `session.rs` |
| 86 | `server_data` | `Component` + `Optional<byte[]>` | `session.rs` |
| 120 | `store_cookie` | `Identifier` + `byteArray(5120)` | `session.rs` |

### The bundle semantics, exactly

`ClientboundBundlePacket` never appears on the wire. `PacketBundleUnpacker`
expands it on the sending side into `delimiter, sub-packets…, delimiter`, and
`PacketBundlePacker` reassembles it on the receiving one. The delimiter's body
is empty, and **that is not the same as an inert packet**:
`BundleDelimiterPacket.handle` throws `AssertionError("This packet should be
handled by pipeline")`, because it is a pipeline instruction rather than a
message. A client that decodes it as a no-op has decoded it wrong in the one
way that leaves no trace.

Four rules, all from `PacketBundlePacker.decode` and `BundlerInfo`:

1. **A bundle is applied all at once, and only when it closes.** The run is
   handed on as one `ClientboundBundlePacket`, and `handleBundlePacket` is a
   plain `for` loop calling `subPacket.handle` directly — one scheduled task on
   the main thread, with the sub-handlers' own `ensureRunningOnSameThread`
   already satisfied. **§3's original wording — "in one tick" — was wrong**;
   the guarantee is that no *frame* is rendered part-way through, and nothing
   defers a bundle to a tick boundary.
2. **An unterminated bundle is withheld, not dropped and not applied.**
   `currentBundler` stays non-null across `decode` calls, so everything after
   an unclosed opening delimiter accumulates and nothing downstream sees any of
   it. This is the case that makes bundling worth having in Rewo at all: the
   drain is `try_recv` until `Empty`, so a socket that hands over a bundle in
   two reads would otherwise apply the first half a frame early — exactly the
   glitch §3 ranks first.
3. **There is no nesting.** `Bundler.addPacket` opens with
   `if (packet == delimiterPacket) return constructor.apply(bundlePackets)`, so
   a second delimiter *always* terminates and a third opens a fresh run. A
   depth counter — the natural implementation — never closes the outer bundle
   and withholds every subsequent packet for the rest of the session.
4. **The size limit is an error, not a cap.** `BUNDLE_SIZE_LIMIT` is 4096 and
   the check runs *before* the add (`if (size() >= 4096) throw`), so exactly
   4096 sub-packets fit and the 4097th kills the connection. Neither delimiter
   counts. Moving the check above the delimiter test — which reads like a
   tidy-up — would make a legitimately full bundle fatal at the moment it
   correctly closed, i.e. only on the servers that send large bundles.

Plus `verifyNonTerminalPacket`: a packet inside a bundle whose `isTerminal()`
is true is a `DecoderException`. In clientbound-play there is exactly one,
`start_configuration`. Rewo's `PlaySession` does not dispatch that packet, so
the other half of vanilla's terminal handling — removing the bundling stage
from the pipeline once one passes through *outside* a bundle — has nothing to
model here.

Rewo has no exceptions, so both fatal cases end the session the way a closed
socket does. What is deliberately not done is to recover: a client that carried
on past a malformed bundle would be applying a run the server never meant to
send as one.

**Wired into `PlaySession::drain_inbound` only.** The M1-era
`Connection::run_play` harness behind `rewo net` / `rewo view` renders no
frames, so bundling there would change nothing measurable.

**Live, against the bundled 26.2 server**: `CORRECTIONS: 0` over 800 ticks with
place and dig both server-observed — and, because that number is equally true of
a bundle machine that never fired, `rewo play` now reports the machine's own
counter: **`bundles applied: 177 (largest run: 3 sub-packets)`** in 40 seconds
against a stock vanilla server. Three is the entity-spawn shape above.

### What `store_cookie` changes about the `cookie_request` reply

`handleRequestCookie` is one line —
`send(new ServerboundCookieResponsePacket(packet.key(), serverCookies.get(key)))`
— so the reply is a straight jar lookup and `null` is the miss, written as a
`writeNullable` present-flag of `false`.

Rewo already answered `cookie_request`. It answered `false` **unconditionally**,
because `serverCookies` had no writer: `store_cookie` is the only thing that
ever calls `put`, and it was not resolved. After M78 the reply carries the
stored payload for a key the server has set, and is byte-identical to the old
behaviour for one it has not. On a network that uses `transfer` between
backends, that is the difference between a session that survives a hop and one
that silently forgets itself on every one.

**"Already answered" turned out to be half true, and closing that is part of
M78.** Only `Connection::run_play` — the M1-era harness behind `rewo net` and
`rewo view` — had an arm for the play-state `cookie_request`. `PlaySession`,
the loop `rewo play` and `rewo live` run, had none, so the client that matters
never replied at all. `store_cookie`'s jar is observable *only* through that
reply, and a jar nothing reads is not a feature, so `PlaySession` gained the
arm. Both loops now write through the same `session::write_cookie_response`.
The §1 machine check is structurally blind to this: it asks whether *anything*
in `rewo-net` dispatches the field, and one of the two loops did — see §4.

Two details that read backwards:

- **The 5120-byte limit is an error, not a truncation.**
  `FriendlyByteBuf.readByteArray(input, maxSize)` throws before copying a
  single byte. A client that clamped would store a cookie it would later hand
  back to a server that never issued it.
- **`store_cookie` and `custom_payload` are `common` packets and exist in
  configuration too** (ids 10 and 1). Both configuration ids are resolved, and
  for `custom_payload` that is not a nicety: `ServerConfigurationPacket-
  ListenerImpl` sends `minecraft:brand` from its opening burst and a vanilla
  server never sends a second one in play. Resolving only the play copy would
  have looked like it worked against every server there is. **This is M69's
  `update_tags` finding, one packet over**, and it is the second time a
  play-only survey has named the id a vanilla server does *not* send — see §6's
  scope note. Observed live against the bundled 26.2 server: with
  `RUST_LOG=rewo_net=debug`, `net: server brand "vanilla"` is logged from the
  **configuration** arm and the play arm never fires.

### Four more things that read backwards

- **`custom_payload`'s unknown identifier is discarded, not rejected**, and the
  fallback **consumes the remainder**. `DiscardedPayload`'s reader takes
  `buf.readableBytes()` and skips exactly that; it throws only above 1 048 576.
  The instinct on a discriminated union is M41's — an untranscribed member is
  fatal because the reader cannot skip it — and it is wrong here precisely
  because this union *has* a fallback codec. Rejecting would kill the
  connection to any modded server.
- **`disguised_chat`'s chat-type is `ByteBufCodecs.holder`, not
  `holderRegistry`.** `0` means an inline `ChatType` follows — two whole
  `ChatTypeDecoration`s, each a string, a counted list of VarInt parameters and
  an NBT `Style` — rather than chat type 0. A raw reading would take the first
  decoration's translation key as the sender's name and every field after it
  with it. M16 records the opposite convention for the dimension holder and M65
  found two enum conventions one field apart; this is the same hazard inside a
  single three-field record.
- **The two vestigial packets are not the same shape.** Both handlers are `{}`,
  but `player_combat_enter` is `StreamCodec.unit` — zero bytes — while
  `player_combat_end` carries a VarInt `duration`. Nothing is stored for either,
  because vanilla stores nothing and inventing a field would be a divergence
  dressed as decode-and-state. **The only gradeable property is the reader
  position**, which is why both readers return the bytes they consumed: with no
  observable state, a reader one byte off is otherwise indistinguishable from a
  correct one right up until it desynchronises whatever follows.
- **`game_rule_values` has no vanilla store at all.** The map goes to an
  `InWorldGameRulesScreen` if one is open, and the screen ignores every packet
  after its first. Rewo keeps the last map wholesale — replacement, not merge —
  because the packet is a full snapshot and merging would strand a rule the
  server stopped sending.

### Where the gate lives, and why it is not a new command

`rewo abilityshot --check` grew a session section rather than M78 minting a
26th `*shot` command. It is the right host by construction: it is already the
serverless CPU-only oracle that resolves **real packet ids through
`Ids::resolve` on the pinned version's datagen report**, which is exactly what
these eight need on top of pure decode, and M75's subject (the session's
authoritative local-player state) is the adjacent one. Every witness names a
mutation partner in its detail string.
