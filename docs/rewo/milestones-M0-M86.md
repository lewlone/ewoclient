# Rewo — the CLAUDE.md milestone record, M0–M86 (plus the Velvet type stack and the headless wire subsystems)

<!-- Moved verbatim from CLAUDE.md lines 2148–4918 on 2026-09-26, when CLAUDE.md
     was cut from 7,192 lines to an always-loaded index plus path-scoped
     rules in .claude/rules/. Everything below the rule is the original text,
     unedited. It is history: read it for the reasoning behind a rule. -->

> **Superseded statements in this file** (kept verbatim below; the current truth is in `CLAUDE.md` or `.claude/rules/`):
>
> - The status block at the top (test counts, gate counts, render-check totals) is a snapshot. Current numbers live only in `REWO_PLAN.md` §0.0.
>
> - "Committed locally on branch `codex/…`, not pushed" lines (M16, M16.1, M17, M18): everything is merged to `main`.

---


## Rewo — from-scratch native Minecraft client (online play, native CEM, exact light/colour, dimensions, the combat + block-entity arcs, weather, particles, the first-person hand, the Velvet type stack, the container arc, the recipe book, chat, translated text, styled spans, the chat decoration, clickable text, the scoreboard sidebar, the tab list with faces and hearts, and two real client options)

**[REWO_PLAN.md](REWO_PLAN.md) is the plan of record — a fresh session must
read its §0.0 HANDOFF first** (it consolidates current state, what to do next,
the headless verification toolkit, the load-bearing gotchas, and a categorized
list of every known issue/gap/deviation, explicitly framed for critique).
**[REWO_AUDIO_PLAN.md](REWO_AUDIO_PLAN.md) is the detail behind its audio
item** — M138a–d, `level_event`'s sounds, the music fade, **M141's ten tickable
ramps** (ten, not the "~8" that plan says), M142's three ambient handlers and
**M143's wire into the client** have all shipped, so `rewo live --audio` on a
build with `--features audio` opens a device and plays what the engine
resolves. **The listening pass is the outstanding work and it is the user's**,
because no gate in this project opens an audio device — an absent, muted,
exclusive-mode or unplugged one all look identical from inside the process, so
everything a machine can check passes and that is *not* the same claim. The
feature is **off by default**, so a default build links no audio stack and the
other 34 gates are unchanged. **Two gates shipped on 2026-08-13/14 and both are
exceptions to that sentence in different ways.** `soundshot` is the gate
`REWO_AUDIO_PLAN.md` §4 specified and M138c recorded as unbuilt, leaving the
largest subsystem in Rewo with only crate-local unit tests that could be deleted
silently; it grades the wire, the resolution and the engine's arithmetic in a
default build (**28** witnesses) and adds decode and the mixer under
`--features audio` (**48**), fail-closing on whichever lock applies, and **it
does not close the listening pass** — its module doc says so verbatim.
`tablistshot` is the other, and it grades a feature that had been finished and
invisible: see the M151 entry.
**Everything is shipped and gated** as of 2026-08-25 (M180) —
**3495 tests / 0 failures** (world 1259, net 1257, gpu 320, data 235, app 236,
mesh 52, proto 16, audio 120 — EIGHT crates now, read off the runner per
crate; a loop written against the old seven drops the new one silently),
`mobshot` 246/246,
`containershot` **109/109**, `inventoryshot` **165/165**, `itemshot` 75/75,
`handshot` 34/34, `swingshot` 97/97, `tablistshot` **42/42**, `soundshot` 37/57,
`bookshot` **24/24**, **`optionshot` 12/12**, **`signshot` 23/23**,
**`advshot` 20/20**,
all **43** serverless
gates green with 0 validation errors, `live --render-check` **64/64** with
validation ON and 0 validation errors (r63/r64 arrived with M174), packet coverage
**124 / 0 / 17**, demo PNG `2cc56b4acbfb92cb`.

**M173 (2026-08-23) built the volume sliders — and the options wiring that
never existed.** The research fan-out's sharpest finding was about M157, not
the sliders: the options framework was model-only end to end — `build()` had
zero production callers, the pause OPTIONS button logged "not implemented",
`save_options` was `#[allow(dead_code)]`. M173 wired the whole thing (open,
navigate, press, drag, save) with the eleven `soundCategory_*` sliders and
the music-frequency cycle inside it. The engine half fixed a real audible
divergence: the slider's `onValueUpdate` is `refreshCategoryVolume` — NOT
`updateCategoryVolume`, whose `gainBySource` is the music crossfade's own
channel — and Rewo had no refresh at all, so vanilla's put-PLUS-refresh had
shipped as the put alone since M140b and the music fade's gain never reached
the playing channel. Under it, `instance.getVolume()` folds the
`sounds.json` entry volume INSIDE the getter, so `Live` now carries
`resolved_volume` and the tick loop's recompute agrees with play. File
codec: the SINGULAR `getName()` keys, JSON double `{:?}` (writes `1.0`),
legacy BOOL alternative, out-of-range REJECTED not clamped (the SCREEN's
setter clamps). Widget: press = `(mx - (x+4)) / (width-8)`; arrow step is
one handle-pixel `1/(width-8)` — 1/302 vs 1/142 by design; labels TRUNCATE
(`0.699999` -> `69%`) and only exactly 0.0 is OFF; a slider never saves the
file (page exit does). The handle's nine-slice border is the ASYMMETRIC
`{2,2,2,3}`; the highlighted TRACK state is deliberately unmodelled (it
needs arrow-key list navigation). `optionshot` is the 41st gate; its o6
probe was wrong three times before the code was once — a value-0 slider
parks its handle AT the left edge, and a probe must avoid everything the
widget draws over its chrome.

**M172 (2026-08-23) rendered the written-book reader M171 modeled — the
`sub-book` item is closed.** A 4-agent research fan-out preceded the code and
its decompile findings corrected M171 twice: `BookAccess.fromItem` falls back
to `WRITABLE_BOOK_CONTENT` (a book-and-quill opens read-only in the same
reader — there is NO WritableBookViewScreen in 26.2), and with NEITHER
component the packet opens nothing — so the capture grew writable pages plus
presence flags for both components (a zero-page written book opens EMPTY, not
the draft). The screen-pass atlas grew 512x256 -> 512x512 (old placements
unchanged in texels — the pre-M172 screen gates stayed green untouched);
`book.png` is CROPPED at bake to the 192x192 the blit samples, making
`Fill::Stretch` the exact 1:1 blit. The wrap is `split_lines_wrapped`, NOT
the chat's `wrap_components` (which prepends an indent space vanilla book
pages don't have). Two of my bugs were caught by pixels, not witnesses:
`book.pageIndicator` is `Page %1$s of %2$s` — POSITIONAL — and a sequential
`%s` substitution rendered the raw pattern identically in both fixtures (the
digit-diff witness measured zero); and the Done button drew unlabeled because
the frame arm pushed `book_text_lines` but not `screen_text_lines` — the
EYEBALL caught it, p12 now pins it. r61's first placement (0.93) broke r24
because `Screens` is ONE slot and the book replaced the inventory the overlay
needed — moved to 0.465..0.5, where the inventory force-open then replaces
the reader and exercises a real-world-load-bearing sync (anything opening
over the reader must drop the stale `self.book`). Open: page-text click
events (M128's active-text hit test, unwired), the lectern's menu-backed
reader, the page-turn sound (Rewo's screens play no UI sounds at all).

**M171 (2026-08-23) is the written-book decode + screen model; the render is
M172.** `open_book` (58) was absent; the written-book reader is a genuine
decode+render milestone, so M171 lands the decode and the tested model — the
M52f/M93z pattern of a screen's model shipping before its render. `open_book`
is consumed (the hand, recorded as a request the app polls), the held book's
pages are captured as `StackComponents::book_pages` (the way `lore` is) and
threaded to the inventory via `SlotText::book_pages`, and
`rewo_world::book_view_screen::BookViewScreen` is the layout + navigation,
verbatim from the decompile (192x192 background at `((width-192)/2, 2)` — top
a fixed 2; page text at `(left+36, top+30)` stepping 9 px, at most `128/9 = 14`
lines; the 23x13 back/forward buttons that hide at the ends; only
PageUp/PageDown turn the page). `ScreenKind::BookView` is the render's seam.
Coverage 120/0/21 -> 121/0/20 (class C 10 -> 9). **M172 is the render**:
`book.png` into a GUI atlas, the styled page text via M126 spans + M100 wrap,
the `PageButton` sprites, and the open/input wiring. (A git/battery race
briefly committed a mutated `forward_visible`; caught by re-reading the
committed content and amended — never run git ops while a mutation battery
modifies the same files.)

**M170 (2026-08-23) drew the leash rope.** The decode has been complete and
gated since M77 (`set_entity_link` -> `is_leashable` -> `set_leash_holder`,
graded by `rideshot`); nothing drew the rope. M170 is `rewo_gpu::leash`
(a verbatim port of `LeashFeatureRenderer`), a `WorldRenderer::draw_leash`
pass (a colour-only POSITION_COLOR triangle list, depth-tested GREATER no-write
against terrain and entities), and `collect_leashes` in `live_cmd` gathering
the endpoints. Five inversions a tidy rewrite loses: the two-pass forward/
backward ribbon (fudge 0.05/0.0) is two-sided; the slack curve is asymmetric
in `dy` (up sags `dy·p²`, down `dy - dy·(1-p)²`); the alternating dim keys to
`backwards` so the twist lines up; `offset` is already folded into `start`
(re-adding it doubles the attach point); and the light interpolates per vertex.
The gather is faithful to `EntityRenderer`'s single-leash branch —
`start = pos + (0, eyeHeight, bbWidth·0.4).yRot(-bodyYaw)`, `end =
holder.getRopeHoldPosition` (a fence knot's is `pos + (0, 0.2, 0)`), `slack =
true` (the `LeashState` default the branch never sets). The colour is folded
LINEAR on the CPU because the world attachment expects linear (the selection
outline's convention). `leashshot` renders `build_ribbon`'s output through the
real pass and reads the frame back (empty/line/sag/dim/fade); r60 drives the
live `collect_leashes` path — a staged cow + fence knot + `set_entity_link`
builds exactly one 294-vertex ribbon into the pass. Two battery survivors were
weak fixtures (the width-collapse hid in the camera's view axis; the edge-fudge
in a loose tolerance), closed with unit witnesses. Open: the light is an RGB
interpolation of the two endpoints rather than the packed coords, and the
happy-ghast quad-leash is not drawn.

**M169 (2026-08-23) fed the jump bar M168 could only draw.** M168 laid the
jump bar out and rendered it and passed `jump: None`; M169 is the whole
`LocalPlayer.aiStep` jump-riding meter (`rewo_net::jump_riding`), the
`START_RIDING_JUMP` packet (the first serverbound `player_command` Rewo has
sent), the SADDLE equipment slot (ordinal 7, which `apply_set_equipment`
discarded — it is the whole of `isSaddled()`), the class sets for
`PlayerRideableJumping` / `Camel` / `AbstractNautilus`, the camel/nautilus dash
cooldowns armed from their DASH metadata, and `Hud.nextContextualInfoState`'s
four-way selector threaded once per frame so the XP / locator / jump bars
cannot disagree about the slot. Five meter inversions a tidy rewrite loses:
the ramp has NO 1.0 cap (tick 10 is 1.0, then it DECAYS toward 0.8); the
release parks at -10 and the bar holds full through the jump; the PRESS zeroes
the scale; a release during a dash cooldown is lost; the packet data is
`Mth.floor(scale * 100)`. And the selector arm that reads backwards: WITHOUT
waypoints a jumpable vehicle ALWAYS wins, scale 0 or not. r59 drives the chain
live — and the `/ride` command did NOT survive the render check (the server
never sent `set_passengers` naming the bot, so `mounts` stayed empty; a
diagnostic read `mounted 0 jumpkey 1115` before the fix), so it INJECTS the
horse + saddle + set_passengers like every other r-witness. Two battery
survivors were weak fixtures, not equivalent mutants: the press-zero shows
only on a re-press mid-park, and the saddle wire path is caught by a living-
entity unit test rather than the gate. Open: the camel's
`LAST_POSE_CHANGE_TICK` decodes but no metadata path arms it, so
`refuseToMove()` is unit-tested only.

**M168 (2026-08-23) rebuilt the survival HUD, and the handoff item that named
it was five gauges short of the truth.** §0.0 listed armour, air, effects,
vehicle hearts and the jump bar as the HUD's gaps; the hearts and food already
on screen were an M3 approximation nobody could test, because a draw inside a
Vulkan pass has no unit test — rounded where vanilla `Mth.ceil`s (0.3 hp drew
nothing), one row, no absorption / blink / heart types / regeneration wave /
low-health jitter, and a hunger row filled from the LEFT where `extractFood`
fills from the right, identical pixels only because ten cells of eight mirror
exactly. The whole thing is the tab list's pattern now: `SurvivalInputs` ->
`rewo_gpu::survival_hud::layout` -> `Vec<HudBlit>` in vanilla's draw order,
and the pass draws what it is handed. Inputs that were parsed and discarded
became real — `DATA_AIR_SUPPLY_ID` (index 1), `DATA_TICKS_FROZEN` (7),
`DATA_PLAYER_ABSORPTION_ID` (17 in 26.2: `Avatar` owns 15 and 16), the
saturation, every active effect rather than the two the lightmap reads, and
`LocalPlayer.hurtTo`'s window, whose FIRST call of a life arms nothing. The
effect category is not on the wire (the report carries only `protocol_id`),
so `tools/gen_mob_effects.py` extracts it with the colour from
`MobEffects.java` — 40 effects, **20 / 16 / 4** by a direct grep (a reader
had reported 22 / 14 and the test caught it). Transcription facts that
invert: `compareFalseFirst(ambient)` under `Ordering.natural().reverse()`
draws the AMBIENT effect first; NEUTRAL effects share the harmful row because
the only test is `isBeneficial()`; `getAirBubbleYLine`'s `(rows - 1) * 10` moves
the line DOWN when there is no vehicle; the jitter is one LCG reseeded
`tickCount * 312871` per frame whose draws are consumed in draw order; and
`/effect give ... true` hides the ICON along with the particles, because the
five-argument `MobEffectInstance` constructor is `this(.., visible, visible)`
— the live witness r58 measured zero icons until the staging dropped the
argument. **Five witnesses were wrong before any code was**, including the
one that assumed the ambient effect sorts last. **The jump bar is geometry
only** — laid out and rendered, never fed: it needs the saddle slot, the
jump-riding ramp and a serverbound `player_command` Rewo has never sent.

**M166 (2026-08-21) fixed a hang, and the handoff item that named it was half
the bug.** A server setting `resource-pack=` **or** `enable-code-of-conduct=true`
left `rewo live` with no window and no error, forever:
`ServerConfigurationPacketListenerImpl.addOptionalTasks` queues **two** tasks
that do not finish themselves, `startNextTask` runs the queue one at a time, and
both replies died on one `_ => {}` arm — whose comment **listed
`code_of_conduct` as something it ignored**. The socket stays live and a
keep-alive lands every 15 s, so the read timeout can never fire; silence is the
whole symptom. The coverage doc's class-C label ("download, prompt, apply") was
wrong for the **sixth** time — the fix is a decode and a 17-byte reply, no
pipeline. The reply is `FAILED_DOWNLOAD`, chosen because
`ServerCommonPacketListenerImpl:107` kicks on **`DECLINED` alone**, so it is both
true and joinable on a `require-resource-pack` server. Coverage **120 / 0 / 21**.
`render_check.py` now stages both tasks (a regression scores *zero* witnesses,
not a failed row) and grew a **timeout**, because until M166 a hung client hung
the gate with nothing to read.

**M167 then took the other two items §0.0 called most valuable, and neither was
what it said.** `is_usable_for_crafting` had been **fixed two days earlier** by
M163's `SlotText` field split and is pinned twice — the item cited
`SlotText::name`, a symbol renamed the day before the item was written, so it
was falsifiable by grep. The witness-name item's mechanism was right and its
size and urgency were not: **there is no `Checker`** (30 independent
definitions, one per gate file) and **no gate emits a duplicate name today**
(all 37 run, `soundshot` in both configurations). M167 ships the source-level
half as a unit test that fails closed on floors and pins its four
branch-exclusive files as exact counts; the runtime half — a shared `Checker`
rejecting duplicates, which is the only thing that sees a *formatted* name — is
left as an explicit 30-file decision rather than assumed. **The standing
asymmetry: §0.0's measurements were exact again and three of its
forward-looking sentences in two milestones were not.**
**M152–M157 landed 2026-08-14** and closed every item `REWO_PLAN.md` §0.0 was
carrying: **M152** `update_recipes` and the smithing quick-move — the last
decline in the container arc (coverage **119 / 0 / 22**); **M153** the
stereo-attenuation divergence as an owned decision the plan had asked for and
nobody had made; **M154** a live bug — the client passed a literal `1.0` for
`getMusicVolume()` under a comment asserting no biome declares the attribute,
and `pale_garden` declares **0.0**, so music should be silent there and was
not; **M155** the tab list's faces and hearts, where the "structural" reason
M151 gave for the faces did not survive the code (two 8x8 CPU crops, not a
64x64 GPU sample); **M156** the static audio decode on a worker, **measured at
20.1 ms against a 50 ms tick**; and **M157** the two real client options — the
third §0.0 named having never been an option at all.

**M158–M165 landed 2026-08-16/20 — a verification milestone, a live bug, a seam,
and the first parallel wave.** **M158** found that M155's tab-list hearts and
faces had **no witness of any kind** — each emitter had one caller, the frame
loop, and replacing either body with `return Vec::new()` left the gate 34/34
green. **M159** moved the streaming audio decode off the client tick and its
measurement inverted its own brief (the refills are the cheap half at 0.6 ms;
the expensive event is *starting* a stream). **M160** guarded the merge-silent
seams after a 20-agent survey found **fifteen of twenty specs independently
claiming the same render-check witness id** — git merges that cleanly and the
gate prints a number that looks right. **M161** is the bug the survey's own
tooling exposed: `live --render-check` was red at r46 for three milestones, and
M157 had seeded the music-frequency option through `setMinutesBetweenSongs`
(which re-rolls `nextSongDelay`) instead of the constructor's plain read — so
**no music played in any session shorter than about twenty minutes**, with 36
gates and 3287 tests green throughout. **M162–M165** are the first parallel
implementation wave, four branches merged with renumbering: the sound halves of
`explode` and `level_event`, the wire-time flattens outside chat (on a plugin
network every nametag, named item, lore line and sign was drawing literal `§c`
characters), waterlogged blocks rendering their water, and `mobtexshot` — the
37th gate and the real-texture multi-entity mob witness missing since M22, which
**narrows rather than closes** the M46 symptom and found that Rewo bakes **zero
of the jar's 147 baby sheets**.

**The lesson the wave taught, in one line:** every branch was green alone and
four of them held production code that could be deleted whole with every gate
still green. Adversarial re-review is what found it; a green suite did not.

**M159 landed 2026-08-17 — the streaming decode moved off the client tick, and
the measurement inverted the milestone's own brief.** M156 moved the static
decode and left this half; §0.0 described it as "modelling `ChannelAccess`'s
one-thread discipline". Measured first over all **98 streamed variants**: a
steady-state refill is **0.6 ms**, and the expensive event is *starting* a
stream — open plus prime, ~4.7 ms warm and **~12 ms cold**, in one hitch, and
every music track is a cold first touch. So the refills the brief names are the
cheap half and **the open moved too**, which is also what vanilla does
(`getStream` is a `supplyAsync`). It is a far smaller cost than M156's 20.1 ms
static decode, and both halves moved because vanilla runs neither on the client
thread rather than because either threatened a frame. Three things the
transcription could not copy, each recorded: chunks return to `LiveSink` instead
of going to the device, because **`CommandRing` is single-producer by
construction**; the open shares the single worker where vanilla uses a pool; and
**a stream needs an EPOCH where M156's static `pending` documents at length that
it needs none** — a static payload is an asset key and a re-acquired channel
wants that same buffer, while a stream chunk is a *position*, so a late one
would splice the middle of a track into its beginning. The queue invariant also
needed a second counter, because asynchronously "asked for" and "landed" stop
agreeing and gating on the latter re-asks for everything in flight every tick.
Battery **9/10 with one proven equivalent** (the explicit prime at the open is
redundant, because `tick` runs the landing and the sweep in that order — kept
anyway, since `attachBufferStream` primes its own queue). Two of its three
survivors were a fixture whose source **never returned empty**, so exhaustion had
no witness. It also corrected a wrong measurement inside M156's own "measure
first" paragraph: *"41 of 578"* streamed variants is neither of the real
figures (**344 of 7,963** entries, or **98 of 4,843** names), and **M143's `344
of 8,024` three milestones earlier was right**. `rewo-audio` 112 -> **120**
tests; `soundshot` 28/28 default and 48/48 with audio; containment re-verified
with `cargo tree` — a default build still links **0** audio crates.

**M158 landed 2026-08-16** and is a verification milestone rather than a
feature: **M155's two emitters had no witness of any kind.**
`tab_list_view::hearts` and `::faces` each had exactly one caller — the frame
loop — and `tablistshot` called neither, building its icon list from `icons()`
alone; no unit test called them either. Measured rather than argued
(`tools/m158_gap.py`): replacing each body with `return Vec::new()` left the
gate **34/34 green**, so both features could have been deleted whole. That is
M45's `install_shapes` shape a fourth time, and the first on a feature that
shipped with its own battery — **M155's ran entirely through `cargo test`, so it
never asked.** §0.0's description was wrong twice over: it called M155's heart
witnesses "model-level, driving the production emitters" (they drive
`heart_blits`, one crate down) and did not mention the faces at all. Seven pixel
witnesses now grade both, every predicted colour a literal read out of the jar's
PNGs — and **the battery then found the hole all seven shared**: they took their
rects from the emitter under test, so a client that stacked every heart at the
column's left edge, or drew 7x7 sprites, survived. `h11` predicts from the
*layout* instead and carries two pitches in one frame. Three of the seven were
also wrong before the code was, the sharpest being a flip witness that **could
not fire** because no player in the scene is named Dinnerbone. `tablistshot`
34 → **42**, battery **12/12** killed over two rounds.

**The recipe book is closed** (M105–M107) and **M108–M111 shipped chat** —
`ChatComponent`, the wrap under it, the `MessageSignatureCache` without which
`delete_chat` cannot be read, the text, the backdrop fills (which took a colour
channel on the HUD vertex), the **`ChatScreen`** you type into, and its
scrollbar. What is left of chat all needs a subsystem Rewo lacks. **M112** then
closed `isHovering`'s narrow-window override and found the recipe book's 77 px
displacement missing from four more consumers — the click, the double-click, the
drag and the item-hover highlight. **M113** decoded the Brigadier command tree
(2,017 nodes off a real server, consumed exactly), and **M114** built
`CommandSuggestions` on it — the two remaining chat packets, brigadier's own
suggestion primitives (graded against the **real jar**, because brigadier is a
library and absent from the decompile), the popup's model and geometry, and its
place at the head of `ChatScreen`'s key order. Coverage is **119 / 0 / 22**,
class C **12**. **M115** then drew it — the rows, the truncation bars, the
scroll dashes and the greyed ghost suffix — and its `--render-check` witness
r29 is built to measure the **production chain** rather than a hand-built
`Suggestions`: the gate injects a `custom_chat_completions` packet through the
real router and types one character, so a break anywhere from the decode to the
render drops it to zero. Mutation-verified live, twice. **M116** then built
the client-side **Brigadier dispatcher**, so `/g` completes to
`gamemode`/`give` with **no packet at all** where M114 asked the server for
every keystroke — and found that **`canUse` is always true for suggestions**:
`getSuggestionsProvider()` returns the provider granted
`ALLOW_RESTRICTED_COMMANDS` explicitly, so `FLAG_RESTRICTED` governs a
send-confirmation prompt and not what the popup offers (M113's guess that
`hasAllowedInput` reads it is wrong — that reads `ChatAbilities`). **M117**
then built the two things that read the parse: the **syntax highlighting**
(where `LITERAL_STYLE` is GRAY, so a parsed command *dims* and only its
arguments stay bright) and the **usage box**, which grows **upward** from the
bottom and is **mutually exclusive** with the suggestion popup. Its sharpest
finding is that `getSmartUsage` **decides with one string and prints another** —
the `LinkedHashSet` of deep usages only settles whether the alternatives
differ, and the pipe list is then built from `getUsageText`. **M118** added the
**entity selector parser** — `@e[…]` parses and completes locally — whose
mechanism is a **function pointer the parse reassigns**, which is why
suggestions survive a throw: `EntityArgument.listSuggestions` catches the
exception with an **empty body** and calls `fillSuggestions` anyway. Two of its
seven suggestion states are **dead in vanilla**. **M119** added
`block_state` and `item_stack`, and found where the **namespace rule** lives:
`suggestResource`'s `filterResources` tests the typed text against an
identifier's namespace and path **separately** when no colon has been typed,
which is the other half of M114a's refusal to split `matchesSubStr` on `:`.
**M120** then claimed **39 of the remaining 45** argument types — the
coordinate family, the fixed word lists, the ranges, the word-shaped scalars
and the identifier family — leaving **six structured ones** (`component`,
`style`, `nbt_*`, `dialog`) named rather than half-done, with a test asserting
every other type IS claimed so the list cannot rot. Its finding: **a bare `~`
is a complete coordinate** (the number after it is optional), and **`^` is
all-or-nothing across a triple** — `^1 ~2 3` is `ERROR_MIXED_TYPE`, not a
mixed one. **M121** closed the set: **every `minecraft:` argument type now
parses**, with the six structured ones handled as **extents rather than as a
grammar**. That is a stated, test-asserted approximation — 26.x's SNBT is a
916-line packrat grammar, and an approximate one would silently accept text the
server rejects, so Rewo measures where the value *ends* and does not validate
it. It over-accepted `{a:}` — and **M122** closed that by transcribing the
grammar itself, so SNBT is now validated rather than measured and M117's red
unparsed tail appears where vanilla shows one. Its findings are the kind a
plausible parser gets silently wrong: **`0b` is zero-as-a-byte and `0b1` is
binary one** (resolved by backtracking, not lookahead), **a leading zero is an
error rather than a value**, and **`_` is a digit separator banned only at the
ends**. **M123** then closed the range gap M122 had recorded as a
failing-on-purpose test, and it was not a bounds check bolted on: **the BASE
decides the signedness** (binary and hex default to UNSIGNED where decimal
defaults to SIGNED), so `0xFFFFFFFF` is a valid int where `4294967295` is not
and `-0xF` is an error; **`s` is both the signed prefix and the SHORT width**;
an array element may **narrow** its width but not widen it, keeping its own
base, so **`[B;255]` is an error and `[B;0xFF]` is fine**; and a float is
rejected for being **infinite**, not unparseable. **M124** then closed the
literal tables — **eight** argument types, not the seven the plan claimed, and
**not** merely a suggestion gap: `heightmap` is the enum **filtered by
`keepAfterWorldgen`** (four names, not six), `swizzle` has a real parse and
deliberately **no suggester at all**, the two slot types read **to the next
space** so `container.*` survives and differ from each other **in the parse**,
and `time`'s suggester **re-anchors past the number** so its unit completes as
a suffix.

**M125** then took the chat decoration §0.0 offered and found a prerequisite
under it, much more visible than the decoration: **every `translate` component
Rewo received rendered as its raw key with its arguments dropped**, so a real
server's join messages read `multiplayer.player.joined`, every death message
`death.attack.player`, and every command's feedback
`commands.give.success.single`. Both walkers said so in their own doc comments.
It ships `decomposeTemplate` as **parts** (so one `FORMAT_PATTERN` serves the
plain and the styled paths — M100's lesson), the resolution itself, and the
wiring, and its composition rule is the thing worth pinning: `Component.visit`
opens with `getStyle().applyTo(parentStyle)`, so a template's literals take the
translatable's style while a component argument applies its own **on top** — a
resolution that substituted plain strings would paint the whole line one colour
and read perfectly correctly. `getArgument`'s `arg.toString()` needed two
artefacts rather than the decompile: `JavaOps` (read out of the shipped
`datafixerupper-10.0.21.jar`) proves the numeric **width survives**, so an
`IntTag(3)` renders `3` and not `3.0`, and `Double.toString` is graded against
a real JDK 25 by `tools/java_tostring_oracle/` — the M114 precedent — where the
plain-versus-scientific band turns out **inclusive at 1e-3 and exclusive at
1e7**. **Its real finding is not about chat at all**: a live trace showed
`/give` rendering as "Gave  [Diamond Sword]" with the count gone, because **NBT
lists are homogeneous**, so a mixed one is written as a list of compounds with
every non-compound element boxed as `{"": value}` (`ListTag.wrapIfNeeded`) and
unwrapped on read (`addAndUnwrap`) — and **Rewo's reader had never unwrapped,
from M1 to M125**. Nothing caught it because skipping the unwrap does not fail;
it yields a plausible wrong tree, and the first thing in 124 milestones to look
at such an element was a translatable's `with`. Its gate witness r37 was also
**wrong before the code was**, the fifth documented instance: it drove the join
message on the premise that a server announces a joining player to that player,
which `PlayerList.placeNewPlayer` disproves (broadcast at line 202,
`players.add` at line **210**).

No branch or worktree holds a commit
off `main` — re-verified 2026-08-14 after the audio-verification landing
(M150, `soundshot`, M139, M151) merged as `c538afd`. The
long-unmerged-branch risk closed on 2026-07-27 and has stayed closed;
branch new work from `main` and keep it that way.

> **⚠ §0.0's prose goes stale faster than its numbers.** The 2026-08-02 pass
> found the handoff still claiming M57 at `aadd8e9` and still offering two
> "cheapest things to pick up" that had both shipped (M58, M59), while every
> *measurement* in the same file was exact. `REWO_PACKET_COVERAGE.md` does not
> have this problem because its table is **machine-checked** against `ids.rs`
> by a unit test. Treat §15's log and the coverage table as current; treat any
> forward-looking paragraph as suspect until checked against `git log`.

> **⚠ The M-numbers are not a contiguous index — use commit subjects.**
> Several sessions have run concurrently with parallel agents, so numbers were
> assigned independently and reconciled on merge: `M52` appears on more than
> one piece of work (as does `M61` — the wavy cape and the bundle decoder),
> `M68` also names two (the motion packets and the sheep's undercoat), `M53` is a *specification* rather than code, and the
> ladder jumps to M58/M59. **`REWO_PLAN.md` §0.0 carries the authoritative
> numbering note** — read it rather than inferring order from the numbers.
> When you need to know what actually shipped, read `git log --oneline`
> subjects. Rewo (from
"rewolution", as Ewo came from "ewolution") is a from-scratch Rust Minecraft
client speaking the vanilla protocol (pin: **26.2 / protocol 776**, read from
the bundled jar's version.json), rendered with **raw Vulkan via ash** —
frame-time consistency (1%/0.1% lows) and input latency first. It plugs into
this launcher as a `Native` instance kind reusing auth + spawn + reaper; it
is NOT a JVM/mod project — `ewo-jni`/mixin machinery does not apply.

**[REWO_FEATURE_SURVEY.md](REWO_FEATURE_SURVEY.md) is the feature roadmap** —
what to build *after* the M-series milestones, derived from a survey of all
9,291 open-source client-side Fabric mods on Modrinth (2026-07-26,
regenerate with `python tools/survey_modrinth.py`). Read it when picking the
next feature rather than the next milestone. Three things from it are
load-bearing anywhere in the repo: (1) **the market leader in every big solved
category is non-open-source** — Sodium (Polyform Shield), EntityCulling
(bespoke protective), Xaero's Minimap + JourneyMap (All-Rights-Reserved), Jade
+ WTHIT (CC-BY-NC-SA, NonCommercial); ~509M downloads whose source must not be
read as reference for Rewo, though bundling the jars in EwoLoader is a
separate question; (2) **53.4% of all client-mod download mass** exists only because the
game is a JVM client with a mod loader (modding infrastructure 25.4% +
JVM performance 17.5% + OptiFine-pack parity 10.5%), which is the strongest
external validation Rewo has; (3) the 9,291 mods collapse to **75 distinct
features** — 50 QoL to build, 11 that are one "port the modules + HUD set into
Rewo" milestone, **5 already at vanilla parity** (M40 tooltips, M41 durability
bars, M51 screenshots, the crosshair, the selection outline — audit against the
crates before scheduling anything from this doc), 3 blocked on audio. Counts
carry a measured **~22-25% error**, the list is a **floor**, and the error is
**not uniform** — it tracks keyword distinctiveness, so vague-keyword clusters
(`Reach / hit indicators` 38% wrong) rank too high while distinctive ones
(`Tooltip overhaul` 0%) are clean. Use it as a prioritisation, never as a
citation.

**[REWO_VELVET_UI_PLAN.md](REWO_VELVET_UI_PLAN.md) is the Velvet UI spec** —
the type stack Rewo needs for tooltips, chat and F3, and the record of a
deliberate **visual freeze**. Read its §8/§9 before touching HUD visuals. The
short version: the glyph/text/chrome machinery landed and is keeper work; the
widget transcription **stopped at one widget on purpose**, because EwoClient's
HUD is getting a visual overhaul and anything transcribed now would be redone.
The chrome palette is de-baked into a `ShellStyle` table so a redesign is a
data edit, not a shader edit. §3's colour-space note is the one thing that
survives the overhaul unchanged, because it is a property of the renderer:
**the Velvet passes must be built with `world::unorm_of(target_format)` and
drawn inside `WorldRenderer::with_gamma_space`**, or the pipeline format
mismatches the attachment.

- **M0–M6 all shipped + headlessly verified + pushed (2026-07-21).** It's a
  playable windowed client (`rewo live`) on offline vanilla 26.2 servers:
  connect, walk/dig/place/chat with 0 physics-corrections, full block-model
  rendering, GPU-driven (compute cull + one indirect draw), with a
  deterministic `rewo bench` regression gate. Subcommands: `net` (M1
  protocol), `view` (M2 snapshot), `play` (M3 headless bot), `live` (windowed
  client), `demo` (M4 model showcase), `bench` (M6 render benchmark).
- **Three load-bearing gotchas** (full list in REWO_PLAN §0.0): (1) mesher
  emits WORLD-space vertices — the shader must NOT add a column origin (the
  double-add was the real M4 "far-field holes" bug). (2) Collision uses
  `baked.solid`, NOT `matches!(Cube)` — grass_block renders as a Model, so
  keying off the render fast-path makes the player fall through the ground.
  (3) 26.x model textures can be `{sprite}` objects, not just strings.
- **Biggest open gaps to critique** (REWO_PLAN §0.0, which is authoritative —
  the rest of this bullet is the *historical* record of how the M0–M6-era gaps
  closed, kept because the corrections are instructive). **Current gaps:** no
  **inventory model** (the container packets have zero references in
  `rewo-net`, so the hotbar draws empty slots and the local player holds
  nothing — the named blocker for first-person hand and GUI); no particles; no
  sound; entity collision ignored; the HUD is crosshair/hotbar/hearts/hunger
  only. Everything below this sentence is closed. All three §4 deviations are
  now closed (rayon mesh pool, Native `live` arm, async upload ring). Fixed
  2026-07-21 (several passes): meshing moved OFF the
  main thread (rayon `MeshPool` + `Arc<Column>` CoW snapshots — mesher
  unchanged, demo PNG byte-identical, bench gate green); the launcher
  Native arm now spawns `rewo live` (+ `EWO_DEV_SERVER=host:port` dev-join
  knob, `package.ps1` stages rewo.exe; UI eyeball still pending);
  **entity rendering shipped** — full movement/player-info decode, vanilla
  3-tick lerp, capsule pass + bitmap-font nametags, verified live
  (position cross-check exact, 129-entity soak at ~1,170 fps, "RewoCap2"
  legible in a headless PNG); **fluids + translucent pass shipped** —
  water (corner-height surfaces, texture-alpha blend, CPU-sorted
  back-to-front per column) + opaque fullbright lava, demo-PNG verified,
  view-replay byte-identical + bench green; **texture animation** (water
  ripples/lava churns via .mcmeta-driven 20 Hz layer re-uploads, PNG-diff
  verified); **the player model** — Steve (12 cuboids incl. overlays,
  box-UV from skin.rs, jar default skin, yaw + head pitch + **walk-cycle
  limb swing** derived client-side from motion) replaces the player
  capsule, headless-PNG verified; the **async upload ring** (4-slot,
  no per-frame fence wait — closes the last §4 deviation); and **gradient
  sky + distance fog** (view-ray sky, terrain fades to the horizon color
  so the chunk-boundary edge dissolves — PNG-verified); and the **in-game
  HUD** (crosshair, hotbar + selection, health hearts, hunger drumsticks
  from the jar's gui sprites; live-only so demo/view/bench are unchanged);
  the **slime mob model** (first real mob — the entity pass now has a
  model registry `EntityModelKind {Player, Slime, Capsule}`; slime is the
  vanilla 8³ cube, face/size deferred); and **block targeting** (voxel
  raycast → vanilla-style selection outline + left-click dig / right-click
  place, so `rewo live` can mine and build); and the **zombie mob** (reuses
  the player model geometry with the zombie skin + arms-forward pose; the
  `chat_command` packet + a `REWO_SUMMON` op'd-summon verification knob came
  with it); and the **cow mob** (the quadruped body plan — rotated body box,
  `box_uv_faces` extracted for the per-vertex transform); and **screen-space
  text** (a `TextPass` rendering the vanilla font with drop shadows — a
  coordinates/facing line + a chat overlay, the client's first on-screen
  text); and **entity metadata decode** (`set_entity_data` → a serializer
  skip table → custom nametags on mobs; slime size/baby deferred as
  entity-specific indices); and **quadruped leg animation** (the cow walks
  in vanilla's diagonal gait — `emit_model`'s rotation generalized to a
  `(pivot_y, pivot_z)` pivot for the front/back legs); and **mob head-look**
  (humanoids turn their heads toward nearby players via the `rotate_head`
  packet — `LimbPart::Head` yaws to its own absolute angle; the same change
  caught+fixed a variable-shadowing regression where the head/body-yaw
  binding shadowed `emit_model`'s model-scale `s`, silently scaling every
  mob by `sin(yaw)`); and an **F3 debug overlay** (vanilla-style block — XYZ
  / block+in-chunk / chunk / facing+axis / loaded-chunks+entities, with
  `rem_euclid`/`div_euclid` chunk math; F3 toggles it windowed, always on
  headless); the **pig** (fourth mob model — grew the entity atlas to
  256×256, generalized `quadruped_model_quads` to `(off, leg, snout)` so cow
  legSize=12 and pig legSize=6+snout share it; cow renders unchanged); and
  the **sheep** (fifth mob — own body dims + an inflated white wool overlay
  from `SheepFurModel`; extracted `build_quad_parts` as the shared quadruped
  builder, and replaced `EntityPass::new`'s 8 positional texture params with
  an `EntityTextures` struct). The mob registry spans humanoid/cube/quadruped.
  Also un-broke `rewo view` (stale M2 bake-sanity check). The mob textures
  those passes shipped were UV-scrambled (verified by silhouette+colour only
  — the "verify the property, not a proxy" lesson); **fixed 2026-07-22 by
  the mob redo**: `crates/rewo-gpu/src/mobs.rs` is a verbatim port of
  vanilla `ModelPart.Cube`/`Polygon` + vanilla's exact entity transform
  (the old path was also X-mirrored), all mob meshes re-transcribed from the
  26.2 decompile (the 26.2 cow is its own mesh, not the generic quadruped),
  and the set grew (two more same-day passes) to **88 mobs — every living
  vanilla mob**: full zombie/skeleton/illager/piglin families, witch,
  guardian+elder, shulker, blaze, ghast 4.5× + happy ghast 4.0×,
  silverfish/endermite, phantom, vex, hoglin/zoglin, strider, magma cube,
  all farm + overworld passives (cat/fox/goat/bee/frog/armadillo/axolotl/
  dolphin/turtle/fish×4/panda/polar bear/camel/llama/parrot/horse family/
  bat), snow/iron/copper golem, allay, warden, sniffer (192²), breeze
  (two-texture: body + wind funnel), creaking, ravager, wither, nautilus
  pair, and the **ender dragon** (256², full mesh). Capsules remain only
  for object entities. Atlas 1024² + shelf packer (16²..256²). Verified
  by the serverless **`rewo mobshot --check` facelabel gate**
  (face-colored debug textures vs a perspective ray-cast of the same
  geometry — occlusion-exact; **246/246 mob-views green**, with 6 mobs
  auto-detected as color-check-N/A where vanilla reuses texels across
  faces — run it after any mob/UV change) + `rewo mobshot --out` contact
  sheet + `--only` closeups + live summon shots; demo PNG stayed
  byte-identical, bench flat, 0 VUIDs. **Animations: every procedural
  vanilla `setupAnim` is formula-exact** (spider leg waves, golem
  triangle-wave limbs, blaze rod orbits, ghast/squid tentacles,
  phantom/allay/vex/bee wing flaps, fish tails, wolf tail wag, silverfish
  wiggle, wither side heads) — parts have base rotations + a parent
  hierarchy + pivot-motion anims; `set_entities` takes a time param
  (`ageInTicks` = s·20). **The keyframe rigs run too**: vanilla
  `AnimationDefinition`s machine-extracted by `tools/gen_anim_defs.ps1`
  into generated `anim_defs.rs` (re-run after a version bump) + a
  vanilla-exact evaluator (next-frame interpolation mode, catmullrom,
  additive apply, per-mob `applyWalk` params) — frog/camel/sniffer/
  armadillo/creaking/copper-golem walks, bat flight, breeze idle,
  nautilus swim, rabbit hop. **Gesture rigs run too**: Pose metadata
  (index 6) + sniffer/armadillo state enums (index 17) decoded; a
  `GestureTracker` times rigs from the observed state change;
  `KfGate::{During, Unless, NotShell}` + `KfDriver::GestureAge` + part
  `Show` visibility rules play warden roar/sniff/emerge/dig, frog
  croak/tongue (+throat pouch), breeze shoot/slide/inhale/jump, sniffer
  dig/sniff/happy/rise + the SEARCHING walk-swap, armadillo
  roll/scared/unroll with the shell-ball swap (verify with
  `rewo mobshot --gesture name[,age] [--shell]` or
  `REWO_FORCE_GESTURE`). **M17 fires the exact model-visible entity
  events** — Warden attack/sonic-boom + Armadillo re-peek from the
  `entity_event` packet (see the M17 bullet below). **M18 shipped the Allay
  dance** — `DATA_DANCING` metadata (index 16, BOOLEAN serializer 8), not an
  event (event 18 is heart particles only): exact `Allay.tick()` counters +
  `AllayModel` root/head formulas, gated by `rewo danceshot --check` 24/24
  (see the M18 bullet below). Still open: the Warden tendril (event 61),
  generic `ClientboundAnimate` arm swings, and dragon flight (bespoke
  procedural code, posed).
  [`REWO_MOB_REDO_HANDOFF.md`](REWO_MOB_REDO_HANDOFF.md) is now a completion
  record; details in REWO_PLAN §15 "2026-07-22 — the mob redo shipped".

- **M0 shipped 2026-07-21** (`crates/rewo-gpu` + `crates/rewo-app`, binary
  `rewo`): ash 1.3 device + MAILBOX swapchain + frame-time strip-chart
  overlay + GPU timestamps + tracy. Verified headlessly on the RTX 5080:
  ~4.3k fps clear+overlay, cpu p99 0.87 ms, validation-clean.
- **M1 shipped 2026-07-21** (`crates/rewo-proto` + `rewo-data` + `rewo-world`
  + `rewo-net`): the full vanilla protocol — Handshake→Login(offline)→
  Configuration→Play with zlib compression + the liveness contract, chunk/
  light/entity decode, packet record/replay. Ground truth = the decompiled
  26.2 jar (Vineflower) + Mojang datagen reports under
  `%APPDATA%/EwoClient/rewo/26.2/` (git-ignored, derived from the user's own
  download). **Verified against a live vanilla 26.2 offline flat-world server
  Claude set up + ran headlessly**: 329 chunks decoded with zero failures,
  block queries hit the exact flat-world layers (bedrock/dirt/grass/air), and
  replay reproduced the live world digest bit-for-bit. `rewo net soak` /
  `rewo net replay` are the M1 verification tools. Key wire gotchas captured:
  the paletted long array is **fixed-size, not length-prefixed**; each 16³
  section starts with **two shorts (non-empty + fluid count)**; packet ids
  are resolved **by name** from the datagen report so a version bump fails
  loud instead of misfiring.
- **M2 shipped 2026-07-21** (`crates/rewo-mesh` + `rewo-data::assets` +
  `rewo-gpu::world` + `rewo view`): first pixels — client-jar asset bake
  (cube-family models only; indexed-PNG expand was the gotcha), face-culled
  mesher with per-face shade × server light, texture array + CPU mips +
  depth + frustum cull, snapshot viewer (`rewo view --replay|--host …
  [--out png]`). Verified headlessly: recognizable flat world PNGs from
  both the M1 recording and a live server; windowed fly-cam ~1k fps, p99
  2.16 ms. **Launcher `Native` arm landed too**: `InstanceLoader::Native`,
  "Native · Rewo" in the new-instance modal, `try_real_launch` spawns
  `rewo.exe` (REWO_* env contract; `view --host` args when a server join is
  active), reaper covers rewo.exe. UI eyeball of the modal + Launch flow
  still pending; `package.ps1` doesn't copy rewo.exe into dist yet.
- **M3 shipped 2026-07-21** (`rewo-world::physics` + `rewo-net::play` +
  `rewo-data::items` + `rewo play`): be a player. Faithful vanilla 20 Hz
  physics port from the decompile (walk/sprint/jump speeds unit-locked to
  vanilla), live play session (split socket: reader thread + 20 Hz tick
  loop) with the exact `LocalPlayer.sendPosition` cadence, dig/place/attack/
  chat/hotbar. Verified headlessly vs the live 26.2 server: **0 server
  corrections over 3,000 ticks** of continuous movement, place→dirt &
  dig→air confirmed by world query + block_update echo, chat round-trip.
  `rewo play` is the DoD bot harness. Gotcha fixed: `Column::block_state_at`
  wasn't consulting the `overrides` map that `set_block` writes (block
  edits looked ignored though the server applied them). Not exercised:
  attack-a-mob (no mobs on flat creative). **The live windowed client
  shipped too** (`rewo live`): the protocol+physics session feeds the M2
  renderer in one loop (20 Hz tick accumulator + per-frame dirty-column
  remesh budget + eye camera + WASD/mouse). Headless `--out PNG` renders the
  first-person eye view; windowed soak ~988 fps, 0 corrections. Rewo is now
  a playable windowed client.
- **M4 shipped 2026-07-21** (`rewo-data::assets` model parser + `rewo-mesh`
  AO/model path + `rewo demo`): real meshing. Full block-model resolution
  (variants w/ x/y rotation, multipart w/ when-conditions, elements/faces/
  cullface/element-rotation) → cube fast-path or baked quad list; 2,320
  cubes + 26,555 models. Mesher adds 26-neighbor AO + the model-quad path;
  tint baked into texture layers (grass/foliage colormap). `rewo demo`
  renders a synthetic showcase (stairs/slab/fence/glass/torch/plants/log)
  headless — verified all model families correct. Also switched the world
  pass to **reversed-Z depth** (`world::perspective_reverse_z`, GREATER, 0.0
  clear) to fix distant-terrain z-fighting holes — helps M2/M3 too. 26.x
  gotcha: model textures can be `{sprite, force_translucent}` objects, not
  just string refs (glass baked invisible until handled). Known cosmetic
  follow-up: grazing-angle far-field slivers on flat ground (needs MSAA /
  back-face cull). Deferred: greedy meshing, fluids, per-biome tint,
  animation, packed vertices.
- **M5 shipped 2026-07-21** (`rewo-gpu::world` full rewrite + `cull.comp`):
  GPU-driven rendering. Mega-buffer arena (device-local vert+index buffers,
  free-list suballocation, one-shot staging uploads) + per-column metadata
  SSBO + compute cull (frustum test → indirect commands + atomic count) +
  single `vkCmdDrawIndexedIndirectCount`. Enabled Vulkan 1.2
  draw_indirect_count + multiDrawIndirect. Verified: renders identically to
  M4, validation-clean, GPU cull drew 113/329 (216 culled on GPU via
  readback), windowed ~974 fps, removed the live-remesh per-frame wait_idle.
  **Two bugs fixed while verifying** (both predated M5): (1) world-space
  vertex double-add — mesher emits world-space but the shader also added the
  column origin, THE real cause of the M4 far-field holes (not depth);
  dropped the origin add. (2) grass_block collided as non-solid — it renders
  as a Model (cube + overlay element), and the collision table was
  matches!(Cube), so the bot fell through grass every tick (258 corrections);
  added a proper `solid` flag to the bake (Cube OR full-16³-element Model).
  Deferred M5 follow-ons: dedicated async transfer queue, visibility-graph
  cull, mega-buffer resize (over-cap columns dropped w/ log).
- **M6 shipped 2026-07-21** (latency/measurement pass): `rewo bench` — the
  deterministic render benchmark (replay world + orbit camera + GPU
  timestamps → avg/p50/p99/p99.9/1%-low/0.1%-low/max + histogram), the
  merge-gate metric. `stats.rs` gains 1%/0.1% lows (mean of slowest N%
  frames) + histogram. Frames-in-flight knob (`--fif`, `Renderer::
  with_frames_in_flight`). Measured on the 5080: GPU render 0.198 ms avg /
  0.367 ms 0.1%-low (rock-solid); windowed frame-consistency avg ~1 ms /
  ~5 ms 0.1%-low; fif=1 measurably tighter lows than fif=2 at same fps
  (latency-first, default stays 2). `VK_NV_low_latency2` deferred
  measure-first (GPU render ~0.2 ms is far below the frame budget — not the
  bottleneck until high RD/complexity). Subcommands: net/view/play/live/
  demo/bench.
- **M7 shipped 2026-07-22** (online-mode): the offline-only restriction is
  gone. `rewo-net/src/crypt.rs` — the login-encryption handshake
  (RSA-PKCS1v15 key exchange → Mojang session join with the BigInteger
  server hash → AES-128-CFB8 both directions, all KAT-tested), wired as a
  `NetStream` that ciphers at the Read/Write seam and splits per-direction.
  `rewo-net/src/chat_sign.rs` — signed chat: fetch the player certificate
  (`api.minecraftservices.com/player/certificates`), announce
  `chat_session_update`, SHA256withRSA-sign each message over the verbatim
  `PlayerChatMessage.updateSignature` layout with a chain index. Account
  handoff via `crypt::OnlineAuth::from_env` (REWO_ACCESS_TOKEN/UUID/
  USERNAME); `ewolauncher --mint-rewo-env` refreshes + prints them
  headlessly. **Verified with the user's real account** on an
  `enforce-secure-profile` server: session join + AES + a signed chat
  message that logged with no `[Not Secure]` prefix (0 corrections). Cert
  gotcha: Mojang's private key is PKCS#8 DER under a PKCS#1 label wrapped
  at 76 chars — strip the armor + parse DER directly (the rsa crate's
  strict RFC-7468 reader rejects it). Milestone detail in REWO_PLAN §15.
- **M7c shipped 2026-07-22** (real player skins): online play shows each
  player's actual skin. `rewo-net/src/skins.rs` decodes the Player Info
  `textures` property → URL + slim/wide; `rewo-app/src/skin_fetch.rs`
  fetches the PNG → 64×64 RGBA (username→profile resolution too); the
  entity atlas reserves a 32-slot 64² skin pool and `EntityPass::
  upload_skin` region-copies a fetched skin into it, returning a UV offset
  that relocates the default-Steve player quads onto that slot. Slim
  (`EntityModelKind::PlayerSlim`, vanilla 3-px arms) + wide, chosen from
  the profile model; overlays ride along. Live wiring is a `SkinLoader`
  worker thread + per-UUID registry in live_cmd. Verified headlessly with
  `mobshot --skin <username|url>` — lewlone's slim skin + Notch's wide
  skin, both distinct from default, overlays + arm width correct; facelabel
  243/243, demo byte-identical, bench flat.
- **Metadata mob detail shipped 2026-07-22**: slime/magma **size** + **baby**
  scaling. Both live at SynchedEntityData **index 16** (polymorphic — INT
  there is `AbstractCubeMob.ID_SIZE`, BOOLEAN there is `AgeableMob`/`Zombie`
  `DATA_BABY_ID`; the serializer type disambiguates in one decode). Index
  pinned by counting `defineId` up the hierarchy (Entity 0–7, LivingEntity
  8–14, Mob 15, subclass 16; cross-checked by the working `DATA_POSE=6`).
  `EntityDraw.scale_mul` scales the model (slime = size/2; baby ×0.5, a
  documented uniform approximation — vanilla keeps the head bigger).
- **M9 native CEM shipped 2026-07-22** (the EMF/ETF-equivalent — the answer
  to "can we do it without mods": yes): Rewo loads an OptiFine CEM resource
  pack and renders both the **custom mob models** and their **animations**,
  no mod loader. Stack: `rewo-data/src/cem.rs` (pack-zip loader → raw
  `.jem`/`.jpm` strings), `rewo-gpu/src/cem.rs` (JEM→`Model` with named
  bones + the `_animations.jpm` → program parse), `rewo-gpu/src/cem_anim.rs`
  (the OptiFine expression-language interpreter — lexer+Pratt parser+eval,
  parses all 284 real FA expressions). `EntityPass::new_with_cem` overrides
  a kind's built-in model; `part_transforms` applies the per-frame bone
  deltas. `mobshot --pack <zip> [--walk sw,amt --time t]` + `rewo live
  --pack` (also `REWO_PACK`). **Two load-bearing CEM conventions** (learned
  the hard way, don't re-derive): (1) a top-level `.jem` `part.translate` is
  the *rotation pivot*, NOT static position → boxes sit at raw coords, only
  *submodel* translates accumulate, and `pivot = to_model(−translate)`
  (verified exact vs vanilla); (2) the model bakes through a 180° Z-rotation
  (that's what `invertAxis:"xy"` is), so the animation's X/Y rotation angles
  + translations are **negated**, Z passes through — this is what turns
  flung-apart limbs into a cohesive walk. Verified on the user's real Fresh
  Animations pack: all body plans render + animate (zombie strides, pig/cow
  walk), no-pack facelabel gate stays 243/243 (additive). Polish left:
  foot-submodel leg pivots ~1px off (flat humanoid rigs exact), per-face
  `uvNorth`, scale channels, ETF textures (M9b). Detail in REWO_PLAN §15.
- **M10 client light engine shipped 2026-07-23** — a placed torch now lights,
  a dug tunnel now brightens. Vanilla clients recompute light for their own
  edits (the server only sends authoritative light at chunk load), so
  `rewo-world/src/light.rs` is the two-phase flood fill (decrease → increase)
  over block + sky, bounded by loaded columns; wired into `PlaySession` so
  `rewo live` relights and remeshes exactly the affected columns. Every rule
  is transcribed from the decompile: `dampening = isSolidRender ? 15 :
  (propagatesSkylightDown ? 0 : 1)`, step cost `max(1, dampening)`, **a face
  passes no light when the two occlusion shapes cover it** (this, not a graded
  cost, is why a `dampening 0` stair still shadows), and the sky column
  descends only while the edge is unoccluded. Data comes from a new machine
  extractor **`tools/gen_block_light.py`** (re-run after a version bump): it
  maps block → implementation class → the `propagatesSkylightDown` /
  `getLightDampening` **overrides** up the `extends` chain (glass returns
  true — sky passes it at full strength; leaves pin 1), expands the
  `ColorCollection` **and `WeatheringCopperCollection`** families by reading
  the id tables in `references/BlockItemIds.java` (the copper naming is
  irregular — `copper_block` but `exposed_copper` — and each name carries its
  weather-state index so a copper bulb's per-state 15/12/8/4 resolves), and
  **validates every generated name against blocks.json** (0 unresolved). Two traps worth remembering: `RenderKind::Cube` is **not** an
  opacity proxy (glass/leaves/ice all bake as `Cube`), and glass/leaves/water
  dampen by **1** — neither 0 nor 15. The gate **`rewo play --light-check`**
  recomputes loaded columns and diffs against the server's own light engine —
  the lighting equivalent of `CORRECTIONS`; **884,736 cells, 0 mismatches**
  on flat terrain, a village, an enclosed shaft, and a sealed torch-lit room.
  It immediately caught two long-standing bugs beyond lighting: the chunk
  payload's `empty_sky` mask was read and **discarded**, so every section above
  the terrain silently read sky-0; and **`section_blocks_update` was entirely
  unhandled**, so any multi-block edit to an already-loaded chunk (a `/fill`,
  an explosion, a piston, another player building) never appeared at all —
  hidden by the harness, because a structure built right after a `tp` is
  already there when the chunks stream in. M10 also added property-driven
  emission (candles `lit ? 3 × candles : 0`, glow berries, sea pickles, light
  blocks, vaults, trial spawners — each rule keyed by a source signature so a
  version bump fails loud) and shape occlusion for the rest of vanilla's
  `useShapeForLightOcclusion` set. **Gate caveat**: `--light-check` diffs
  against the *stored* light, which incremental relighting writes — pass
  `--no-relight`, or build in one run and grade from a fresh join, or the
  engine grades itself. Detail + open items in REWO_PLAN §15.
- **M11 vanilla lightmap + day/night shipped 2026-07-23** — M10 made the light
  *values* right; they were still rendered through an invented formula.
  `shaders/lightmap.glsl` now transcribes vanilla's: block and sky stay
  **separate** to the fragment shader (packed into the spare bits of the
  per-vertex layer word, `layer | block<<16 | sky<<20`, so the vertex doesn't
  grow), each goes through the curve `l/(4-3l)`, and they are **added** — not
  `max`ed — with block tinted warm. There is **no floor**, so an unlit cave is
  finally black (the old `0.25 + 0.75*l` could never go below 25%).
  `rewo-world/src/daylight.rs` transcribes 26.x's keyframed
  `Timelines.OVERWORLD_DAY` (the hard-coded `getSkyDarken` is gone): sky light
  1.0→0.24 and white→blue at night, sky gradient and fog darkening with it.
  Because the factor is a **uniform**, a sunrise costs one push constant, not
  a remesh — and a torch stays as bright at midnight as at noon. Wire gotchas:
  `set_time` is now `gameTime` + a **map of clock states**, a server sends
  **two** clocks (overworld *and* the_end — match by registry id, don't take
  the first), `ByteBufCodecs.holderRegistry` writes the id **raw** (the `id+1`
  scheme is a different codec), and clock states are only sent when they
  change. It also fixed a real long-standing bug: `emit_model` sampled the
  block's **own** cell (inside a solid block = always dark), so since
  grass_block renders as a Model the entire ground plane of every overworld
  was lighting at zero — hidden by the old floor. Detail in REWO_PLAN §15.
- **M12 sun/moon/stars/sunrise shipped 2026-07-23** — the sky was a bare
  gradient; M12 draws the clear-weather Overworld celestials in a Vulkan pass
  between the gradient sky and terrain. `rewo-world/src/celestial.rs` ports the
  exact 26.2 `Timelines.OVERWORLD_DAY`: the sun/moon/star **angle** tracks carry
  `symmetricCubicBezier(0.362, 0.241)` over a two-keyframes-at-tick-6000
  wrap-around pair (a naive lerp would freeze the sun), so `EasingType.CubicBezier`
  is ported verbatim (Newton-Raphson `solve_t` + bisection fallback); star
  brightness + the sunrise `ARGB_COLOR` track use linear ease, the latter
  interpolated by `srgbLerp` (componentwise `Mth.lerpInt`, **alpha included** —
  that settles RGB-vs-ARGB). `rewo-gpu/src/celestial.rs` + four shaders draw
  sunrise→sun→moon→stars in `addSkyPass` order, rotation-only sky space
  (`view_proj·T(eye)`), no depth, with the decompiled transform chains (base
  `Y(−90°)`, per-body `X(angle)`, sun `T(0,100,0)·scale(30,1,30)`, moon
  `scale(20,1,20)`, fan `X(90°)·Z(angle+90°)·scale(z=alpha)`), OVERLAY/TRANSLUCENT
  blends, reversed moon UV winding, and an 8-cell atlas. Textures come from the
  user's own client jar — sun + **eight separate moon-phase files**
  (`environment/celestial/moon/<phase>.png`, `MoonPhase.index()` order); no jar,
  no celestials. **Stars are generated bit-for-bit** vs a JOML 1.10.8 Java oracle
  (seed-10842 `BitRandomSource` LCG, reject sq-length `≤0.010000001`/`≥1.0` →
  **780 accepted / 4680 indices**, fingerprint `fef182656c6fe202`): the catch is
  JOML's `Math.fma` is **non-fused** by default, so `lengthSquared` is the
  right-associative `x*x+(y*y+z*z)` (2 ULP off a true FMA), and `libm::sin` (new
  dep, fdlibm) matches Java's `(float)Math.sin` where Windows' libm drifts. The
  sunrise fan samples the **`Mth` 65,536-entry sine table**, not platform trig —
  load-bearing at the half-turn, where `Mth.sin(π)` is a tiny *positive* table
  entry so the fan stays on side 0° while platform `sin(π_f32)` is negative and
  would flip (fan fingerprint `75280003503b2a33`). M12 also fixed the M11
  `SKY_COLOR` bug (only the horizon was tinted → blue midnight zenith; the zenith
  now scales by the sky tint too) and a **frozen-clock** bug: the server
  broadcasts `SetTime(gameTime, empty map)` every 20 ticks (only join/`/time`
  carry a clock state), so `day_ticks` froze while game time advanced. The fix
  ports 26.2's `ClientClockManager` — a `WorldClock` advanced from both
  `apply_set_time` (advance-then-overwrite, so an empty sync still moves the
  cycle) and a per-tick `ClientLevel.tickTime` local `+1`; running both isn't
  double-counting because each `advance` re-bases on `last_game_time`. Java
  primitive semantics are exact (`Mth.floor` returns an **`int`** → narrow to
  `i32` before widening `long fullTicks`; `partial` truncates back to `f32`;
  wrapping `long` arithmetic). Verified by a new permanent serverless gate
  **`rewo skyshot --check`** (validation layers on) that reconstructs each
  transform independently in f64 and asserts read-back pixel properties (zenith
  tint ratios, phase/alpha/discard/UV-winding, projected sun/moon envelopes,
  analytic sunrise-fan footprint, the 780/4680 star count) — not a "looks right"
  proxy. Gates: **142** unit tests green (world 44, net 41, gpu 33, data 5, mesh
  8, proto 11), skyshot green, mobshot 243/243, demo PNG byte-identical, bench
  GPU 0.228 ms avg, light gate EXACT, world clock advanced +278/280 ticks.
  In-game visual parity is **not** claimed (no eyeball pass); the properties the
  gate checks are what M12 verifies. Detail in REWO_PLAN §15.
- **M13 complete 26.2 lightmap shipped 2026-07-23** — ports the remaining
  `LightmapRenderStateExtractor`/`lightmap.fsh` terms: the exact four-draw
  LegacyRandom block flicker, gamma (default 0.5), night vision, darkness and
  its 22-tick blend state. The extractor partial is fixed **1.0**, not render
  interpolation. The full shader order is preserved, and the actual block tint
  is **0xFFFFD88C (255/216/140)**; M11's 0xFFD86C blue was wrong. Configuration
  captures the two effect registry raw IDs; update/remove packets affect only
  the local player, with exact duration/replacement semantics. One resolved RGB
  lightmap state now drives terrain, water and entities. Permanent gate:
  **`rewo lightmapshot --check`**, a validation-required production Vulkan
  readback matrix that independently proves tint, block factor, gamma ramp,
  night vision, black NaN store, darkness, water parity and entity RGB. It
  caught an adjacent asset-bake bug in the uncompromised M10 oracle: the fluid
  branch skipped light assignment, so water dampening was 0 (must be 1) and
  lava emission 0 (must be 15). Fixed from generated tables, not by editing
  generated code. Final gates: **180/180** six-crate tests + 10 app tests,
  lightmapshot/skyshot validation ON, mobshot 243/243, byte-identical demo,
  physics corrections 0, light **884,736 cells / 0 mismatches**. Replay median
  remained ~0.23 ms but later tail samples were system-noisy; exact numbers and
  the honest red-to-green water history are in REWO_PLAN §15.
- **M14 per-biome color shipped 2026-07-24** — grass/foliage/water tint +
  biome-driven camera sky/fog. The Configuration registry decodes in raw wire
  order (**66 biomes, 4 dimension types**); section biomes are retained 4×4×4
  (index `((y<<2)|z)<<2|x`, strategy bits 0 single / 1–3 indirect / >3 direct at
  registry `ceilLog2` = 7 for 66), from both the level-chunk payload and the
  `chunks_biomes` replacement (changes/load dirty 3×3). Dynamic tint is the exact
  **radius-2 5×5 integer mean** over the fiddled `BiomeManager.getBiome` for
  grass/foliage/dry-foliage/water, with `dark_forest`+`swamp` grass modifiers,
  fixed spruce/birch constants, tall-grass UPPER sampling below; tinted faces use
  the **raw atlas layer** + `MeshVertex.color` (no ABI growth), and a no-biome
  world keeps the legacy pre-tinted layers so the demo stays byte-identical. A
  per-`mesh_column` **`TintCache`** (canonical key = sampled pos+resolver,
  GrassBelow→Grass@y-1, constants bypass) mirrors vanilla's `BlockTintCache`
  without a global lock/invalidation. Camera sky/fog is a **separate** path — the
  raw-quart 6³ Gaussian (kernel `[0,1,4,6,4,1,0]`, integer `ARGB.srgbLerp`,
  dimension base then biome override) feeding **per-frame GPU base uniforms** (no
  remesh); Rewo's existing gradient/timeline sky still renders it, so this is
  *not* a formula-exact whole-sky claim. **Load-bearing protocol fix**: the play
  login dimension holder is `holderRegistry`/idMapper **raw 0-based**, NOT
  `ByteBufCodecs.holder`'s inline/`id+1` — correcting that adjacent bug was
  required to select the dimension sky/fog base. Permanent gate:
  **`rewo tintshot --check`** (serverless, validation-required Vulkan readback of
  the production jar-bake + synthetic single/indirect/direct biome containers +
  `mesh_column`), pinning Temurin-25-verified vectors — boundary
  **[91,163,163]**, dark_forest **[147,26,5]**, swamp light/dark
  **[106,112,57]**/**[76,118,60]**, spruce/birch **[97,153,97]**/**[128,167,85]**,
  camera fog boundary **0xffac2d6d** (A inherits / B overrides), fully-fogged
  terrain green→blue under a red sky, **0 VUIDs**; it rejects constant-plains,
  axis transpose, wrong fiddle/radius/mean, spruce/birch-as-foliage, wrong
  modifiers, raw/legacy mixup, block-fiddle camera sampling, and dropped GPU
  plumbing. Final gates: **215/215** six-crate tests (world 81, net 67, gpu 37,
  data 9, mesh 10, proto 11) + **10/10** app; tintshot/lightmapshot/skyshot
  validation ON exit 0 0 VUIDs, mobshot 243/243, byte-identical demo, physics
  **CORRECTIONS 0** over 600 ticks (PLACE+DIG both verified), light **884,736
  cells / block 0 sky 0 EXACT**. Replay (no biome context — guards neutral
  rendering, does not measure the tint cache) GPU avg ~0.238–0.241 ms.
  **Scoped exclusions**: biome blend radius fixed at vanilla default 2;
  modifier-form custom-datapack sky/fog attrs not applied (26.2 uses bare
  overrides); probe per-tick history omitted (sampled per frame); respawn /
  dimension-transition / Nether-End base selection untested; redstone/stem/lily
  `BlockColors` explicitly out of M14. Honest history + exact numbers in
  REWO_PLAN §15 (incl. the first oracle rejected as insufficient and strengthened).
- **M15 exact packed ABI + conservative greedy cubes shipped 2026-07-24.**
  `MeshVertex` is **28 bytes**: position f32×3, exact UV f32×2, packed
  `layer16|block4|sky4|shade3|AO2`, and packed tint RGB. A 24-byte f16-UV
  candidate was rejected after changing 6 canonical-demo pixels (max Δ25).
  The final shader reconstructs the legacy shade×AO×tint formula exactly; the
  shader build parses optimized SPIR-V and requires float 255, `OpFDiv`, and
  `NoContraction`. Full cube faces greedily merge only across identical block
  state, packed light and tint with uniform AO. Models/fluids remain
  byte-identical. **Never merge +Y/top faces** without a new proof: enabling
  them changed 11 demo pixels (10 UV interpolation/nearest-sampling, one
  coverage); the other five directions are byte-identical. Permanent gate
  `rewo meshshot --check` expands rectangles to reference unit faces and pins
  direction/block/layer/light/AO/tint seams plus exact model/water/lava controls.
  Oracle fixture: 854→265 quads (−69.0%). Replay: 149.13→109.39 MiB
  (**−26.65%**), 3,723,192→3,373,772 vertices (−9.38%), arena 93.080→84.344%;
  final GPU avg 0.232 ms, but noisy tails mean no latency improvement is
  claimed. Gates: **237/237** six-crate + **19/19** app, all property gates
  green, demo exact, physics corrections 0, light 884,736/0. Full failure
  history and measurements: REWO_PLAN §15.
- **M16 dimensions shipped + verified 2026-07-24.** It is committed locally on
  branch `codex/rewo-m16-dimensions` and not pushed. The vanilla test server was
  stopped and port 25599 verified free after the final gates. The
  `minecraft:dimension_type` registry is now parsed once
  (`rewo-net/src/dimension_parse.rs`), kept in **raw wire order** — the vector
  index *is* the holder id, nothing selects by name — and actually consumed:
  per-dimension vertical shape (the Nether is 0..256, not −64..384; the stale
  Overworld shape mis-decoded every Nether chunk), `has_skylight`, `skybox`,
  ambient light, Nether cardinal face shade, sky/fog/ambient/sky-light colours
  and factor, `has_fixed_time`, and `has_day_timeline`. Plus the End sky pass
  (`rewo-gpu/src/end_sky.rs`), spawn info, and a transition that discards the
  old world and refences the mesh pool by generation.
  **Three load-bearing facts.** (1) `has_day_timeline` is **independent of
  `has_fixed_time`** — separate `DimensionType` members; deriving one from the
  other happens to be right for all four vanilla dimensions and is still wrong.
  It comes from the `timelines` holder set, expanded through
  `data/minecraft/tags/timeline/*.json`. (2) The Nether sets **no**
  `sky_color`/`fog_color`; absence must stay `None`, because the attribute's
  literal `0` default would read as opaque black to the biome colour stack.
  (3) A malformed entry is a connection error — never a substituted Overworld.
  Gates: **`rewo dimensioncheck --check`** (serverless) grades four independent
  inputs — a captured Configuration `registry_data` packet, the bundled
  transcription, the **real decompiled datagen JSON** read by
  `rewo-app/src/dimension_json.rs` (a `serde_json` reader sharing no code with
  the NBT parser), and a hand-written `EXPECT` table that grades all three and
  is itself graded by the JSON — then the world/mesh binding and the generation
  fence; it fails closed on a missing recording or decompile. **`rewo play
  --dimension-check`** is the live gate: 4/4 checkpoints, 3/3 transitions, 329
  columns discarded/requeued each, 0 decode failures, 0 settled corrections.
  Measured: **344 unit tests** (proto 11, world 93, data 9, net 102, mesh 38,
  gpu 44, app 47), `mobshot` 243/243, all Vulkan oracles green with validation
  ON / 0 VUIDs, demo SHA-256 byte-identical to M15, physics 600 ticks
  CORRECTIONS 0, light 884,736 cells / 0 mismatches, release build green.
  Replay GPU avg 0.240 ms with a system-noisy tail — **no** latency improvement
  claimed. Full detail: REWO_PLAN §15.
- **M16.1 — play gate build actions now fail closed 2026-07-24 (`f4b54d1`,
  local; not pushed).** M16 left one honest red deferred: `rewo play`
  (build-enabled) printed "PLACE verify … still air ✗" ~1 run in 4 yet exited 0,
  and `place:true`/`give:true` meant only "packet sent". **Not a protocol bug —
  packets were byte-exact vs the decompile; a pre-M16 (M3-era) harness + gate
  defect.** Root cause (decompile): the harness placed dirt at `(fx+1, fy)`, the
  cell beside the bot's feet; 26.2 `BlockItem.canPlace` gates on
  `isUnobstructed(state, clickedPos, placementContext(player))` and the player's
  0.6-wide AABB reaches east to `fx+1.3`, so its own body occupied the cell
  whenever fractional x ≥ 0.7 → server rejects → air (intermittent as resting x
  varies; dig never hits this). Fix: place two east (`fx+2`, past `fx+1.3`). The
  observation was always right — `handleUseItemOn` sends the acting player a
  `block_update` for BOTH `pos` and `pos.relative(direction)` on every
  use-item-on, accepted or not. The gate (`build_acceptance` +
  `evaluate_build_actions`) now reads the server's world at the recorded targets
  and proves the EXACT state (placed == `minecraft:dirt` default from the block
  table, dug == air), prints `ACCEPT …`, and returns exit 1 if unproven or
  never-run; `--no-build`/`--dimension-check` are exempt. Gates: 350 unit (app
  47→53), 4× live 30 s CORRECTIONS 0 + place=dirt + dig=air exit 0, fail-closed
  proven live (16 s run exits 1).
- **M17 exact model-visible entity events shipped + verified 2026-07-25.**
  Committed locally as `55388c8` on `codex/rewo-m17-entity-events` (base
  `f4b54d1`; not pushed). Before it, `ClientboundEntityEventPacket` fell off the dispatch chain
  as an unknown id — the Warden's ribcages never animated and a balled Armadillo
  never re-peeked. The packet is a **signed fixed BE-i32 entity id + signed byte
  event id** (not var-ints); the report resolves clientbound-play `entity_event`
  to **id 34** (looked up by name, so a renumber fails loud). Type ids resolve
  through production `EntityTypes::id_of`: **Warden 143, Armadillo 4**. Three
  mappings in `apply_entity_event`/`route_entity_event`: Warden 4 → durably stop
  the metadata roar `AnimationState` for that same ROARING episode, then
  unconditionally restart exact `WARDEN_ATTACK`; Warden 62 → restart exact
  `WARDEN_SONIC_BOOM`; Armadillo 64 → re-clock the shared metadata SCARED/PEEK
  `AnimationState` from age 0 (the final balled hold remains after it runs).
  Repeats restart the clock; missing/wrong-kind/unknown/excluded events are
  inert; state clears on entity removal and id reuse.
  **Load-bearing:** the two Warden rigs are exact generated defs from decompiled
  `WardenAnimation.java` via `tools/gen_anim_defs.ps1` (never hand-edited); the
  Warden ribcages were promoted from static folded cubes to **named body
  children** so `WARDEN_SONIC_BOOM` can swing them (neutral geometry unchanged →
  mobshot untouched); the generator now emits **deterministic LF** so
  `git diff --check` is clean and a re-run reproduces the file byte-for-byte (an
  EOL-ignoring semantic diff is exactly the two new defs, 222 lines). The
  renderer feeds a **production event-age input distinct from the metadata
  gesture ages**, sharing the session tick/partial epoch, through the same
  CEM/vanilla part pipeline.
  **Corrections/exclusions (recorded so they aren't re-derived):** Allay
  `handleEntityEvent(18)` is **heart particles only** — the dance is
  `DATA_DANCING` (metadata index 16, BOOLEAN serializer id 8) with client
  dancing/spinning counters + root/head formulas; that's separate future
  *metadata-animation* work, not an entity-event claim (the generic
  `(16,BOOLEAN)→baby` decode Rewo has is latent/inert for Allay only because
  `is_baby` isn't rendered for it). Warden tendril (event 61, needs tendril
  procedural/emissive modelling), generic `ClientboundAnimatePacket` arm swings
  (need handedness/equipment/CEM closure — a future combat-animation milestone),
  hurt/damage overlays, particle/sound-only statuses, and AI simulation are all
  excluded. **No live AI-triggered encounter was staged or claimed** — M17 is
  authoritative through exact raw-packet injection into the production dispatcher
  plus independent decompile literals; these client-receipt semantics don't
  depend on vanilla's server-authoritative (nondeterministic) AI timing.
  **Gate: `rewo eventshot --check`** — permanent serverless CPU-only,
  fail-closed **28/28 witnesses**, driving the whole production path (raw
  fixed-body packet → `route_entity_event` → `EntityTable::start_event` →
  `resolve_mob_anim` → `oracle_part_deltas`). Loads real
  `packets.json`/`registries.json`, proves id 34 + Warden 143/Armadillo 4; the
  targets are **independent decompiled literals** (it does NOT read `anim_defs`
  as its expectation; catmull-rom recomputed from four frame literals), ~1e-4
  tolerances, each with a mutation/sensitivity partner (wrong packet id,
  missing/wrong entity, event 61/unknown, repeat, remove/reuse, neutral parts).
  Two consecutive release runs: identical PASS 28/28. Measured: **360 unit
  tests** (world 95, net 110, gpu 44, data 9, mesh 38, proto 11 = 307 lib; app
  53 — M16.1 was 350; M17 adds world +2, net +8), release build green
  (pre-existing warnings only), mobshot 243/243, lightmapshot/skyshot/tintshot/
  meshshot/dimensioncheck green with Vulkan validation ON / 0 VUIDs, demo SHA-256
  byte-identical to M15, bench replay GPU avg 0.231 ms (no latency change claimed
  — M17 doesn't touch the replay entity path), physics 600 ticks CORRECTIONS 0,
  light `--no-relight` 884,736 cells / 0 mismatches, live dimension 4/4
  checkpoints + 3/3 transitions. Full detail: REWO_PLAN §15.
- **M18 exact Allay dance (DATA_DANCING metadata animation) shipped + verified
  2026-07-25.** Committed locally as `bb8be20` on `codex/rewo-m18-allay-dance`
  (base `6096bbd`, the M17 handoff; not pushed). The Allay dance is *metadata*,
  not an entity event (M17 proved event 18 is heart particles) — the first
  metadata-driven rig. `DATA_DANCING` is **SynchedEntityData index 16, BOOLEAN
  serializer 8** (`Allay` extends `PathfinderMob`, not `AgeableMob`, so slot 16
  BOOLEAN is dancing, whereas `AgeableMob`/`Zombie` put `DATA_BABY_ID` there — the
  byte parser can't disambiguate, only the **kind** can, at the routing layer).
  Resolved wire facts: `set_entity_data` id **99**, Allay type **2**, Zombie
  control **151**. Shipped, all decompile-exact: the `Allay.tick()` client
  counters in `rewo-world` `EntityTable` (dance-tick increments then reads the
  `%55<15` spin window; `spinning`/`spinning0` ramp ±1 clamped 0..15; false resets
  on the *next* tick; repeated true does **not** restart; cleared on remove +
  re-add), the `AllayModel` root/head formulas (`Anim::AllayRoot`/`AllayHead`:
  `danceSpeed = ageInTicks·8° + walkAnimationSpeed`; `root.yRot = 4π·spin` only
  while `isSpinning` else 0; `root.zRot = cos·16°·(1−spin)`; `head.yRot/zRot =
  cos·30°/14°·(1−spin)`; dancing suppresses the head-look; wings stay
  unconditional), the Allay model **restructured into the real
  `root→{head, body→{arms,wings}}` hierarchy** (rest geometry neutral, mobshot
  243/243 unchanged), and **vanilla missing-entity inertness** (`handleSetEntityData`
  drops metadata for `getEntity==null` — no state mutated). Production chain: raw
  report-resolved `set_entity_data` → `route_set_entity_data`/`apply_set_entity_data`
  (kind-aware routing) → `EntityTable` counters → `live_cmd::resolve_allay_dance`
  (shared by the collector and the gate) → GPU pose; `play_cmd` + `live_cmd` both
  resolve the Allay type id. **Senior review corrected**: missing-id baby fallback
  → decompile-exact inert; extracted the shared live resolver so the gate can't
  bypass the app mapping; `play_cmd` now resolves the Allay id; explicit
  wrong-index/wrong-serializer witnesses. **Gate: `rewo danceshot --check`** —
  permanent serverless CPU-only fail-closed **24/24**, two identical runs;
  independent counter sim + `AllayModel`/`AllayWing` transcriptions (nothing reads
  the production formulas as expectation), real `packets.json`/`registries.json`.
  Measured: **368 unit tests** (world 98, net 114, gpu 44, data 9, mesh 38, proto
  11 = 314 lib; app 54), release build green, plain `git diff --check` clean,
  eventshot 28/28, mobshot 243/243, lightmapshot/skyshot/tintshot/meshshot/
  dimensioncheck green (Vulkan validation ON), demo SHA-256 byte-identical to
  M15/M16/M17, replay GPU avg 0.220 ms (no latency change claimed), live physics
  600 ticks CORRECTIONS 0, light 884,736 cells / 0 mismatches, dimension 4/4 + 3/3.
  Exclusions: no live jukebox/AI encounter (raw-packet injection is the
  deterministic proof); the Allay's unconditional body flying-tilt / root
  idle-bob / arm idle-bob remain unimplemented (not the dance); no claim of
  exhaustive index-16 ownership. Full detail: REWO_PLAN §15.
- **M19 exact combat swings + the ArmPose hold baseline shipped + verified
  2026-07-25.** `ClientboundAnimatePacket` (id **2**, VarInt id + unsigned byte
  action) was falling off the dispatch chain, so nothing ever swung. M19 ships
  the exact `LivingEntity` swing state machine (accept/restart rule, `swingTime
  = -1` park, increment-then-end, the `getAttackAnim` `+1` wrap), item-driven
  duration (`tools/gen_swing_animations.py`: **7 non-default over 1,537 items** —
  the spears, STAB 13–23; everything else WHACK/6) with exact DIG_SPEED /
  MINING_FATIGUE adjustment, a machine-extracted living/swing-ticking split
  (`tools/gen_entity_classes.py`: **93 living / 36 swing-ticking of 158**), and
  `HumanoidModel.setupAttackAnimation` — **layered on the `ArmPose` hold
  baseline** `pose{Right,Left}Arm` writes first (`EMPTY` / `ITEM` / `SPEAR`).
  Two load-bearing facts: **`ITEM` is the fall-through for any ordinary held
  item**, so omitting the hold stage posed every armed player from an unarmed
  baseline (18° too high, walk swing unhalved) — and it is `AvatarRenderer`, not
  `HumanoidMobRenderer`, that produces it; and `SPEAR`'s `affectsOffhandPose`
  means a spear in the **off** hand leaves the main arm entirely unposed.
  Unknowable items suppress the pose and CEM `swing_progress` rather than guess.
  `ItemTags.SPEARS` is read as a *tag* from the client jar, not inferred from the
  swing component. Gate: **`rewo swingshot --check` 61/61**, serverless,
  fail-closed, with independent `ease`/`Mth`/pose transcriptions (the `Mth`
  witness: 0 bit mismatches over 60,003 samples vs 39,917 platform-sine
  differences). **404 tests**; demo PNG byte-identical to M15–M18; live
  `--swing-check` decodes server-sent equipment with CORRECTIONS 0. **Open:**
  `animateZombieArms` (the undead families have their own attack rig, so a
  swinging zombie shows no arm motion yet) and the eight use-driven arm poses.
- **M20 exact mob combat rigs shipped + verified 2026-07-25.** M19 gave the
  *player* an exact swing; M20 gives it to the mobs that attack you — four
  vanilla rigs that all run **after** `HumanoidModel.setupAnim` and overwrite
  it: `AnimationUtils.animateZombieArms` (zombie/husk/drowned/zombie-villager/
  zombified-piglin), `SkeletonModel`'s own override, and `IllagerModel`'s
  arm-pose switch with both its attack branches. **The sizing discovery:** the
  undead arms were a baked `Fold::rot(−π/2)` on `STATIC_PART` — frozen at −90°
  where vanilla rests at **−π/2.25 (−80°)** and deepens to **−π/1.5 (−120°)**
  when aggressive, so the pose was ~10° wrong *and* structurally unable to move.
  They are real animated parts now. Three vanilla quirks reproduced, each
  witnessed: a **STAB item skips the strike** (the humanoid pose survives) and
  then takes a **second bob** (`bobArms` sits outside the guard — observable as
  `zRot +0.2`); `animateAttackArms` **assigns rotations only**, so
  `setupAttackAnimation`'s pivot movement survives underneath; and **only a baby
  holding an item** drops its arms. One new wire input:
  `Mob.DATA_MOB_FLAGS_ID` — **index 15, BYTE**, bit 2 `isLeftHanded`, bit 4
  `isAggressive` — the same slot M19 reads as the player's main arm, separated
  by serializer and additionally gated on the type being a `Mob` (an
  `ArmorStand`'s client flags share the slot). It fixes mob handedness for free
  (`Mob.getMainArm()` *is* `isLeftHanded()`). Three more polymorphic slots are
  resolved by machine-extracted ancestry (`gen_entity_classes.py` `ANCESTRY_SETS`,
  fail-loud if empty): `MOB` 90, `RAIDER` 6, `SPELLCASTER_ILLAGER` 2, `ILLAGER`
  4 — note **`ravager` and `witch` are Raiders but not Illagers**, so their
  index-16 BOOLEAN is `IS_CELEBRATING`, previously misread as baby. Illagers
  assign their **own walk over both arms** (wiping the hold pose, attack and
  bob), then switch: empty-handed `ATTACKING` runs `animateZombieArms` with a
  **literal `true`**, armed runs `swingWeaponDown`, and `CROSSED` is a
  *visibility* switch (one model, both arm sets). Gate: **`swingshot --check`
  61 → 77 witnesses**, independent transcriptions throughout, metadata driven
  through the real `route_set_entity_data`. **410 tests**; demo PNG
  byte-identical to M15–M19; `mobshot` 243/243 even though undead neutral
  geometry moved. **Open:** held items are not rendered (mobs swing
  empty-handed); illager `CROSSBOW_HOLD`/`CROSSBOW_CHARGE` are derived but not
  posed (they need `ticksUsingItem`, unsynced for remote entities).
- **M20.1 + M21 shipped + verified 2026-07-25.** **M20.1** fixes the live build
  gate M20 recorded as flaky: it clicked the top face of `(fx+2, fy-1)`, which
  assumed the bot stood on *undisturbed* ground — an earlier run's own hole put
  the target on the grass surface and the server correctly rejected it. It now
  scans east for the first air-over-solid column and **fails closed** if there
  is none (5/5 green in the world that used to fail 1-in-4). **M21 consumes
  `ClientboundDamageEventPacket`**: the exact `hurtTime`/`hurtDuration` clock
  (10, one decrement per tick, re-armed not extended by a repeat), the
  `walkAnimation.setSpeed(1.5F)` limb kick with vanilla's render-side clamp to
  1.0, and the red damage flash. The packet's damage-type holder is
  `holderRegistry` — a **raw 0-based id**, not `holder`'s `id+1` — and the whole
  body is walked so a short read cannot desync the buffer; receipt is gated on
  the entity being tracked *and* living (`handleDamageEvent` is a `LivingEntity`
  override). **The flash forced a vertex-ABI split**: vanilla's `entity.fsh`
  mixes the overlay into `texture × vertexColor` and multiplies the lightmap
  *after*, so the CPU can no longer fold light into the vertex colour — a new
  `light_hurt` attribute carries light in `rgb` and the hurt flag in `a`. The
  overlay is `OverlayTexture`'s red row, **0xB3FF0000** (rgb (1,0,0), a =
  179/255), and the mix is done **in sRGB space**, not linear. Gate:
  **`rewo hurtshot --check` 18/18**, validation ON, 0 VUIDs, verifying the flash
  *by predicting the hurt pixel from the unhurt one* with sensitivity partners
  for linear-space mixing and post-lightmap application. **415 tests**; demo PNG
  byte-identical to M15–M20. The ABI change also exposed a latent bug — the
  upload path hard-coded `total * 36` beside `VERTEX_STRIDE`, so at stride 52
  only 36 of every 52 bytes reached the GPU (`mobshot` 223/243 until fixed).
  **Open:** held items are still not rendered (mobs swing and flash
  empty-handed); `deathTime` — the other half of `hasRedOverlay` — is the death
  animation and its own feature.
- **M22 held items shipped + verified 2026-07-25 — both geometry paths.**
  M19-M21 built the swing, the mob rigs and the damage flash; every one of them
  was swinging empty-handed. 26.x splits an item into a *definition*
  (`assets/minecraft/items/<item>.json`, a tree that chooses a model from stack
  state) and the parent-chained model. Surveyed on the real jar: **1390 of 1537
  are plain `minecraft:model`**, of which **750 point at `block/…`** and the
  rest walk `item/<n>` → `handheld` → `generated` → `builtin/generated`. The
  seam that unified them is **`append_model_quads`**, which takes a model *name*
  and emits quads carrying a texture-array layer index — so a block item reuses
  the block bake rather than needing a parallel resolver; the entity pass cannot
  sample that layer, so its pixels are copied out. Both paths converge on
  **quads in 0..16 model units with UVs in 0..1 of their own texture**, and the
  renderer never learns which source an item came from. The sprite path is
  `ItemModelGenerator`'s extrusion (two faces across the 7.5..8.5 slab + one
  thin quad per alpha edge, UVs inset 0.1); a diamond sword bakes to **82
  quads**. **Two invertible details** transcribed deliberately:
  `SideDirection::Left` is `Direction.EAST` (the names describe the sprite edge,
  not the world axis) and `isTransparent` is **true out of bounds**, which is
  the only reason a sprite border extrudes. **The trap:**
  `ItemTransform.Deserializer` multiplies translation by **0.0625** and clamps
  (±5, ±4) *before* `apply` runs — storing raw JSON puts every item **16× too
  far from the hand**. Shading uses the **rotated** normal, not the baked `dir`,
  because an item is turned on its side in the hand. 1233 textures do not fit an
  atlas band, so items got the demand-filled pool player skins already have; the
  atlas grew 1024→1280 while the shelf packer still stops at 896, leaving **mob
  packing byte-for-byte unchanged** (mobshot 243/243). Gate:
  **`rewo itemshot --check` 18/18**, validation ON, 0 VUIDs, verifying placement
  *against the hand* — sprite centroid (90,151) and block centroid (87,156) land
  together, proving one transform chain serves both sources, and a suppressed
  item differs from an empty hand by **0 pixels**. **435 tests**; demo PNG
  byte-identical to M15-M21. **Open:** the 147 state-dependent items
  (select/special/composite/condition/range_dispatch) suppress rather than
  guess; first-person/GUI/ground contexts, the spear attack-item animation,
  enchantment glint and per-layer tint are all out.
- **M23–M25 + the block-entity arc shipped 2026-07-25/26.** M23 item-use state
  (retiring the blocker three earlier milestones blamed — `useItemRemainingTicks`
  is *derived* by the client, not synchronised) and the eight use-driven
  `ArmPose`s; M24 the death animation and item entities; M25 block-entity decode
  plus a fail-closed type registry and a *measured* statement of the gap (96
  blocks bake to no geometry, 86 of them real block entities). Then the
  rendering half: chests, chest lids driven by `block_event`, double chests,
  17 shulker boxes, and **world-space text** so signs are legible — which
  turned out to be a small addition rather than a new pass, because a nametag
  is already world-space glyph quads and sign text is the same emitter with the
  basis taken from the surface instead of the camera.
- **M26 shipped + verified 2026-07-26 — `block_event` reaches the right block
  entity, and a shulker box opens.** `b0 == 1` is **not one opcode**: it is a
  chest's viewer count, a shulker box's open/close pair, and a bell's
  `Direction.from3DDataValue`, selected by the block entity's type exactly as
  vanilla's virtual `triggerEvent` call is. Reading it as "a chest lid" — which
  this client did — meant a bell rung from any side but below opened a phantom
  lid at the bell. Also: the shulker's rule is `b1 == 0` / `b1 == 1` with **no
  else** (a second viewer changes nothing), not the chest's `b1 > 0`; the
  animated part group became a matrix so one emitter expresses both a hinge and
  a slide-plus-spin; the classification caught up with the four types that had
  quietly started rendering (**seven** still invisible, not eight); and
  `BlockEntityRegistry` runs in the client rather than only in the gate. Two
  process lessons recorded in REWO_PLAN §0.0: a witness that asserts a *moment*
  ("nothing is Rendered yet") is not a guard, and several source files carry
  mixed CRLF/LF endings that an editor will silently normalise into a
  3,400-line diff. Gate `rewo blockentityshot --check` **88/88**; **479 tests**
  (424 lib + 55 app); demo PNG byte-identical to M15 onward.
- **M27/M28 shipped + verified 2026-07-26 — sign text, and the invisible block
  entities.** Five commits took `blockentityshot` from 70 to **125** witnesses
  and the still-invisible block-entity set from **eleven types to two**.
  - **M27** dyed and glowing sign text plus the line break. *Glowing text is
    not "the same colour, brighter"*: unglowing is the dye at 40%, glowing is
    the dye at FULL strength lit fullbright, with the 40% version demoted to
    its eight-copy outline. A sign does not wrap — `getRenderMessages` keeps
    fragment 0, so a long line is truncated at a word boundary.
  - **M28** skulls (7 types, 14 blocks) + the conduit shell. Skulls are
    **entity** models, authored y-down, so both transforms end in
    `scale(-1,-1,1)` — a chest has no such flip. Forced four generalisations of
    the box machinery: rest rotation, `CubeDeformation` grow, mirror, and a
    **per-model texture size** (a mob head's sheet is 64×32, not 64×64).
  - **M28b** the decorated pot — the first block entity that is not one model
    (base + four sherd-textured sides). Needed a second form of `visibleFaces`:
    `EnumSet.of(NORTH)` builds only one face where `allOfEnumExcept` omits one.
  - **M28c** banners (32 blocks) — the first whose texture carries no colour. A
    pattern sprite is a greyscale **mask**, so `BlockEntityDraw` grew a `tint`
    rather than baking 16 dyes × 43 patterns. The banner dye table is
    `getTextureDiffuseColor`, **not** the sign's `getTextColor` (red 0xB02E26
    vs 0xFF0000), and a wall banner's yaw is the facing's *own* toYRot where a
    wall skull's is its opposite.
  - **M28d** the spawner's `block_event` — the third meaning of `b0 == 1`.
    Resetting `spawnDelay` is the whole client effect and shows only through
    the spin: `1000 / (spawnDelay + 200)` makes a spawner **accelerate** toward
    its next spawn.
  - **Three gate witnesses caught real bugs pre-ship**: a pot side baking six
    quads instead of one (coincident, z-fighting), a banner base texture path
    (`entity/banner/banner_base.png`, not `entity/banner_base`) that baked no
    pole while every pattern still loaded, and an existing witness that had
    quietly started measuring skulls as shulker boxes.
  - **M28e/M28f** the copper golem statue and the two end portals — the last
    two. The statue's four poses are **separate** nested layers where a child's
    offset rides through its parent's *rotation*; they are machine-extracted by
    `tools/gen_copper_golem_poses.py` because 38 rotated boxes fail **silently**
    when hand-copied, and `k25` proves the hierarchy by comparing each box
    against a naive offset-sum (must agree in STANDING, must differ in
    RUNNING). The end portals were half-misdescribed as "a shader, not a
    model": the geometry is an ordinary cube (portal = horizontal faces only, a
    slab from y 0.375 to 0.75), and only the render *type* is a shader,
    approximated by one static layer of `end_portal.png`.
  - **M25's Invisible list is now EMPTY — eleven types measured, eleven
    rendering.** `blockentityshot` 21 → **133** witnesses across the arc.
  - **M29 the block-entity animation clock** — banners sway, pots wobble,
    piglin ears move, dragon jaws open. Not ONE clock: what each animates
    *from* differs (position+gametime / an event+start tick / an accumulating
    counter), and grouping by that is what made it tractable. A pot's wobble is
    a **fourth** meaning of `b0 == 1` (`b1` = a WobbleStyle ordinal, and the
    arrival tick is the start). **It exposed two rest poses that were already
    wrong**: `SkullModelBase.setupAnim` ALWAYS runs, so a piglin's ears belong
    at ∓0.7 rad (not the mesh's ∓30°, ~10° off on every head) and a dragon's
    jaw rests 0.2 rad OPEN (Rewo drew it shut) — a wrong *rest* pose is
    invisible precisely because nothing moves to contradict it.
  - **M30 the active conduit** — that world scan. A conduit decides its own
    activation from the blocks around it (the server sends nothing), so
    `updateShape` was the whole prerequisite. **The shell is 42 positions, not
    48** — the three axis rings share their axis ends — **and 42 is also the
    hunting threshold**, so a conduit opens its eye exactly when its frame is
    COMPLETE. `isWaterAt` counts waterlogged blocks, so the bake grew a
    per-state `water` table. Ships the cage (tumbling about the tilted axis
    `(0.5,1,0.5)`, not plain Y), the wind shroud twice (the second at 0.875,
    counter-rotated), and a camera-facing eye — the one input in this path
    that's a property of the VIEW not the block. The deg→rad round trip in the
    renderer is an exact no-op; don't "fix" half of it.
  - **M31 the spawner's caged mob.** M29 called this "an entity model composed
    into a block-entity draw" — one word off: the mob belongs in the **ENTITY**
    path, just positioned differently. Every other entity STANDS (`pos` = its
    feet); this one is *mounted*, so `EntityDraw` gained an optional `mount`
    affine applied to the feet-relative position. Same models, rigs and
    animations as every other mob. Display entity is `SpawnData→entity→id` (two
    levels down); empty/absent/unregistered → NO mob, never a default. Scale is
    `0.53125 / max(bbW,bbH)` **only if > 1.0**; render spin is the stored one
    **×10**; `scale_mul` stays 1 (the fit scale is in the mount — applying both
    shrinks it squared).
  - **A witness disproved my own comment** (3rd time this arc): I claimed the
    inner `translate(0,-0.2,0)` makes the mob orbit — it lies **along the spin
    axis**, so it commutes and the translates could be swapped with no effect.
    The `-30°` X tilt is the load-bearing part. **The claims that survive
    unchallenged are the ones nothing in the render moves against.**
  - **M32 the end-portal shader** — the last item. It samples in **SCREEN
    space** (`texProj0 = projection_from_position(gl_Position)`, vertex format
    POSITION-only), which is why the mesh UVs were never used and why it needed
    its own pipeline; the two portals leave the block-entity resolver entirely,
    or they'd draw twice. `PORTAL_LAYERS` 15 (portal) / 16 (gateway) — a shader
    *define* in vanilla, a push constant here. Sampler0 is end_sky, Sampler1 is
    end_portal (the opposite of the name). `GameTime` is a **daily fraction**,
    not a tick count. **Trap:** vanilla's `mat4(...)` literals are
    **column-major** GLSL — the translate lives at `m[0][3]`, and it works
    because the sampling is `texProj0 * matrix`, a ROW-vector multiply. Copy
    them verbatim; "tidying" them into the slots they look like they belong in
    silently breaks every layer.
  - **The block-entity arc is complete**: 11 invisible types measured, 11
    rendering, `blockentityshot` 21 → **172** witnesses. Its real lesson is
    the **five** times a witness corrected something already written as fact
    (see REWO_PLAN §15 "The block-entity arc, in one place"). **The claims that
    survive unchallenged are the ones nothing in the render moves against.**
  - **M32b closed the portal pass's read-back gap**: `rewo portalshot --check`,
    serverless, validation-required, **12/12**, 0 VUIDs. Two properties make an
    exact prediction possible without reproducing a single matrix — **uniform
    textures collapse them** (the frame is then
    `sky*COLORS[0] + portal*sum(COLORS[0..layers])`, computed on the CPU), and
    **one layer isolates one sample**, at which point the sampled `u` is an
    affine function of the screen UV alone and the column-major reading is
    directly observable. Mutating the shipped shader to the transposed multiply
    drops that witness 21/21 → 9/21 while every uniform-texture witness still
    passes.
  - **The portal's sample is welded to the SCREEN, not the model.** Sliding the
    quad through the world or rolling the camera leaves a screen-covering
    portal's pixels identical (measured: ≤175 of 65,536 bytes, all at delta 1).
    The first version of that witness asserted the opposite and failed.
  - **M33 weather and clouds** — rain, snow and a cloud deck, gated by
    `rewo weathershot --check` **27/27** (validation ON, 0 VUIDs). Three facts
    that read backwards: **`START_RAINING` sets the rain level to 0 and
    `STOP_RAINING` to 1** (the names describe the server's transition; the
    client sets the value its `RAIN_LEVEL_CHANGE` ramp starts *from*); the
    client **never interpolates** the level (`setRainLevel` writes both slots,
    so the smoothing is entirely server-side); and clouds are absent **by
    attribute, not by dimension check** — `CLOUD_COLOR` defaults to a
    transparent 0 and the pass is skipped on zero alpha, which is exactly how
    the Nether and End have none. A cloud carries no texture: `clouds.png` is a
    map, one texel per 12×12×4 cell, and the mesh is three bytes per quad the
    vertex shader expands from a fixed table. Weather forced `MOTION_BLOCKING`
    to stop being decoded-and-discarded. **Two witnesses caught real
    bugs**: a front-face convention that looked right from below alone (hence
    grading the deck from both sides), and — from the first live frame, not the
    gate — that vanilla's weather and cloud geometry is **camera-relative**
    while Rewo's `view_proj` already carries the camera, so the relative form
    draws every storm around the world origin. The gate had rendered at
    `[0,0,0]` where the two coincide; it now renders 2,500 blocks out. Wired
    into `rewo live` (both paths) with `REWO_FORCE_WEATHER=<rain>[,<thunder>]`
    as the headless knob.
  - **M33b — the rainy sky greys through `WeatherAttributes`, not
    `applyWeatherDarken`.** M33 shipped the latter and the sky stayed blue.
    26.2 puts weather's visuals in the **environment attribute system**
    (`world/attribute/WeatherAttributes.java`): RAIN/THUNDER layers rewrite
    SKY_COLOR (`BLEND_TO_GRAY`), FOG_COLOR (`MULTIPLY_RGB`), CLOUD_COLOR,
    SKY_LIGHT_LEVEL/COLOR/FACTOR, STAR_BRIGHTNESS (`set 0` — stars are removed,
    not dimmed) and SUNRISE_SUNSET_COLOR before any renderer reads them.
    `applyWeatherDarken` is a secondary touch-up on the SKY colour only; M33 had
    also applied it to the fog, double-darkening it. **The lightmap does darken
    in rain** — a `client/`-only grep for `getRainLevel` misses it because it
    arrives through the attribute system. The levels **partition**
    (`rain - thunder`), and THUNDER applies to RAIN's output. The **rain fog
    ramp** shipped with it — stateful (eases at `deltaTicks * 0.2`), gated by
    sky light (a cave is clear in a storm), half-strength in a dry biome. It
    needed a second, **environmental** fog band in the world pass: Rewo's
    existing band is a render-distance fade, vanilla's `total_fog_value` is the
    `max` of that and an environmental term, and only the latter is what rain
    thickens — applying the offsets to Rewo's tight band half-fogged the air ten
    blocks out. The new band lives in the `LightmapExtra` UBO (the push block is
    exactly at its 128-byte budget) and defaults to disabled. The `max` of the two
    bands is pinned by four pixel witnesses in **`lightmapshot`** (its camera is
    a known 16 blocks from the quad, so the fog fraction is exact) —
    mutation-verified against `min` and a sum.
  - **561 tests** (500 lib + 61 app); demo PNG byte-identical to M15 onward.
    Fourteen serverless gates, all green with Vulkan validation ON and 0 VUIDs:
    `mobshot` 243/243, `blockentityshot` 172/172, `swingshot` 97/97, `hurtshot`
    38/38, `weathershot` 35/35, `eventshot` 28/28, `itemshot` 28/28, `danceshot`
    24/24, `portalshot` 12/12, plus `skyshot`, `lightmapshot`, `tintshot`,
    `meshshot`, `dimensioncheck`. Live: `play --light-check` 884,736 cells / 0
    mismatches, `play --dimension-check` 4/4 + 3/3, physics CORRECTIONS 0.
- **M34 the inventory, and icons in the hotbar (2026-07-27)** — the client now
  knows what it is carrying and draws it. **Two coordinate systems** meet here
  and never line up: the wire's 46 **menu slots** (hotbar from 36, offhand 45)
  against the game's **inventory indices** (hotbar 0..8), and the three packets
  are split across them — `container_set_*` speaks the first, `set_held_slot`
  the second. Three non-obvious decode rules: an out-of-range held slot is
  **ignored, not clamped**; `container_set_slot` carries its index as a
  **signed short** among var-ints; any container id but 0 is an open screen
  this client hasn't got, so it's dropped whole. Icons needed `display.gui` —
  **absent for a sprite, which is correct** (identity maps 0..16 model units
  onto exactly the 16 px slot), `scale 0.625` + `rotation [30, 225, 0]` for a
  block (reaches 8.37 px against the slot's 8). GUI lighting is a **third**
  model, neither the world's `Direction` shade nor the hand's. Building the
  gate found two bugs first — `init_gui_items` leaked an image/sampler/pipeline
  per hotbar change, and the atlas was repacked every frame. Then the gate's
  own first measurement counted "non-black" pixels **against a painted sky**
  and measured exactly zero while the PNG showed both icons rendering
  perfectly; and one witness had its reasoning backwards ("a sprite covers more
  of its slot than a block" — a sword is mostly transparent), replaced by a
  mutation rendering the same block with an identity transform. Gate
  `rewo inventoryshot --check` **16/16**; **578 tests** (517 lib + 61 app);
  demo PNG byte-identical to M15 onward. **Open:** no inventory *screen* (the
  other 37 slots are held, never shown), no stack counts or durability bars.
- **M35 the inventory screen (2026-07-27)** — the panel, all 46 slots, the
  hover highlight, the stack on the cursor, and clicking. **The click is a
  prediction the server grades**: the packet carries the client's belief about
  every changed slot as a `HashedStack`, and the *only* resync trigger is
  `packet.stateId() != menu.getStateId()`. The first live click was rejected for
  exactly that — a harness bug, not a code one: it clicked while `/give` was
  still advancing the id. `tools/gen_item_props.py` extracts the two per-item
  facts the arithmetic needs, neither on the wire — `max_stack_size` (295 of
  1537 differ from 64) and `equippable`'s slot (83 items). Layout facts that are
  not guessable: `isHovering` is an **18x18** box (`left - 1 .. left + w + 1`),
  so slots tile without a dead column; the hotbar row is a named `top + 58`, not
  3x18; highlights are drawn at `slot - 4` at 24x24, **bracketing** the icon;
  the panel is centred by integer division. The backdrop is a **gradient**
  (0xC0101010 → 0xD0101010), not a fill. The one honest approximation:
  `isSameItemSameComponents` — Rewo knows *whether* a stack carried components,
  never what, so a patched stack swaps rather than merging (one-directional by
  construction; a wrong merge would fuse two tools, a missed one is corrected).
  **Measured, not squinted at**: the panel looked washed out with a black hole —
  six of seven probes are byte-identical to `inventory.png` (the seventh is the
  F3 overlay) and the black is the texture's own window, which vanilla covers
  with the 3D player. Gate `rewo inventoryshot --check` 16 → **39/39**, plus a
  live `REWO_CLICK` knob that counts container resyncs (the container
  `CORRECTIONS`); **586 tests**; demo PNG byte-identical to M15 onward.
  **Open:** the player preview is not drawn (the most visible gap); no
  shift-click, drag, number-key swap, Q-drop, tooltips, recipe book or
  durability bars; armour icons stay blank (their `select` trim definitions are
  among M22's 147 suppressed).
- **M36 the player preview (2026-07-27)** — the black rectangle M35 left in the
  inventory is `inventory.png`'s **own** window (vanilla paints it so the model
  has something to stand against), and this fills it. Transform =
  `PictureInPictureRenderer.prepare` then `GuiEntityRenderer.renderToTexture`:
  `T(w/2,h/2) . S(s,s,-s) . T(0,bbH/2+0.0625) . Rz(pi) . Rx(yAngle)`, with
  `s = guiScale * 30`. **The step that is easy to miss is on the CAMERA** —
  `orientation.rotateY(PI)` — and it is load-bearing: `bodyRot = 180 + xAngle`
  already points the model away from an unturned camera, so the first build
  rendered Steve's **back**. Rewo's entity pass takes no camera state, so the
  half turn goes on the model instead. The preview owns a **second
  `EntityPass`** (two `set_draws` into one vertex ring would cross the draws),
  built on first open, with its own atlas — hence its own skin upload, since a
  UV from the world's atlas would land on some mob's texture. It **clears depth**
  over its window (`vkCmdClearAttachments`, to **0.0** — reversed-Z; vanilla's
  `Projection.getMatrix` swaps `near`/`far` for the same reason) or the model
  comes out sliced by the terrain behind the panel. **Measuring beat squinting
  again**: the render looked too large and mispositioned; the measured feet
  (191.6 px down a 210 px window) and head (29.6) matched the decompile exactly
  — the size was right and the eye was wrong, and what *was* wrong was the
  facing. Gate `inventoryshot --check` 39 -> **44/44**; headless knobs
  `REWO_PREVIEW_SKIN=<username|url>` and `REWO_MOUSE=x,y`. **Open:** the model
  stands still (no local-player animation state); lighting is Rewo's entity
  shading, not vanilla's `ENTITY_IN_UI` rig; armour is not shown on it.
- **M37 particles (2026-07-27)** — the milestone REWO_PLAN §16 refused to
  propose, because every gate here is geometry-based and particles looked
  stochastic. They are not: **`Particle.tick()` contains no randomness at all**,
  every generator is `java.util.Random`'s 48-bit LCG, and a fixed seed turns
  spawn offset, velocity, lifetime, colour, quad size and sprite index into
  assertable numbers. Two anchors stop that being circular, and they retire
  different failure modes — the JDK's own `Random` is genuinely independent
  ground truth for the generator (MC's `BitRandomSource` reimplements its
  formulas), and a Java harness of **verbatim decompile source** grades the
  physics, which is the only thing that can catch a *misreading* rather than a
  mistranslation. It caught `+ 0.1` where vanilla writes `+ 0.1F` on its first
  run — a ~1.5e-9 error, invisible in any screenshot, that shifted every
  subsequent tick. `nextGaussian` is the one primitive graded to a **ULP bound**
  instead of to the bit, because `Math.log` is a JIT intrinsic spec'd only to
  1 ULP, so vanilla's own spawn scatter is not bit-reproducible between two
  JVMs and a zero-tolerance gate there would assert more than vanilla
  guarantees. Six kinds (block, smoke, flame, splash, crit, poof); a block-break
  shard samples the **block** texture and a flame the particle strip, unified
  into one `sampler2DArray` so both share a pipeline; `BakedAssets::
  particle_layer` resolves each state's model `#particle` slot, which is why a
  broken grass_block throws *dirt*-coloured shards. Gate **`rewo particleshot
  --check` 34/34**, mutation-tested against five breakages. Verified live: the
  shard colour **tracks the block state** (redstone red / lapis blue / gold
  yellow), and a real `/setblock … air destroy` spawns exactly **64** shards —
  the 4×4×4 grid the gate asserts from the other direction. **610 tests**; demo
  PNG byte-identical to M15 onward.
- **A frame-diff witness must hold everything but the subject constant, and a
  world-mutating trigger cannot** (M37, not particle-specific). Measuring the
  block-break shards' colour by frame-diff gave 0.04 chromaticity agreement with
  an explicit `block{grass_block}` particle on one run and 0.16 on the next, so
  the first figure was **retracted rather than defended**. `/setblock … air
  destroy` changes the world: the removed block covers thousands of pixels, the
  shards spawn inside the volume it vacated, and the two frames differ in
  lighting *history* (one relit incrementally from a client edit, the other given
  the server's light at chunk load). A same-world control removes the largest
  term but not all of them. **The diagnostic tell, both times, was that
  restricting to strongly-changed pixels made the discrepancy WORSE** — that is
  the signature of a contaminated control, where edge-blending would have
  improved. Measure such a path by a property that does not need a clean frame
  diff (here: the spawn *count*, and the texture resolution the non-mutating
  `/particle block{…}` rows already exercise).
- **M38 the first-person hand (2026-07-28)** — the blocker §0.0 named was M34's
  inventory model, and with it gone the hand went in: the held item through
  both geometry paths, the swing, the equip dip, the view sway. **Two bake
  rules are invertible** — an absent `firstperson_lefthand` falls back to the
  **right** entry and *only* in first person (`ItemTransforms`' builder has that
  line; the third-person pair has none), and the left/right **mirror is applied
  at draw time, not baked** (`ItemTransform.apply` negates `translation.x`,
  `rotation.y`, `rotation.z`; `handheld` authors its left pre-mirrored so the
  two cancel, and baking it would double it). **The swing clock is not a new
  machine**: `LocalPlayer` is an ordinary `LivingEntity` and `Player.aiStep`
  calls `updateSwingTime`, so it takes an id in M19's swing table —
  `tick_swings` iterates the swing map, not the entity map — and M34's
  inventory supplies the held item the duration needs, which is the real join
  between the two milestones. **Two clocks, easily conflated**: `attackAnim` is
  the entity's; the equip height is `ItemInHandRenderer`'s own and ticks per
  *tick*, not per frame. **The hand has its own projection** —
  `calculateHudFov` returns a hard-coded **70** vertical — and vanilla
  **clears depth** before drawing it, without which a wall a block away slices
  your arm off. Three things measured not assumed: the arm chain's translates
  are block units with cube vertices divided by 16 (with it the arm lands
  1.1 blocks below the eye; without it, ten blocks away), the item quads really
  are 0..16, and the pass is the GUI-item pass with two differences (a
  view-projection push constant, the world's flipped viewport). **The 1.36x was
  not there**: the render was first committed with that unexplained width
  discrepancy, bisecting showed the geometry matched a hand derivation to a
  tenth of a pixel, and the fault was the **detector**, which was also counting
  the hotbar's dirt icons — re-measured cleanly, every edge lands within a
  pixel. That was the **third detector error of the milestone**, all the same
  shape (non-black against a painted sky, brown against a brown hotbar, cyan
  against a blue sky), so `handshot` is built around avoiding the class: a
  synthetic **magenta** cube, with an empty frame asserted to contain none.
  Gate `rewo handshot --check` **22/22** (two of its own witnesses were wrong
  first — the fallback check used a *stick*, which parents `item/handheld` and
  authors both hands; and the fail-closed count caught 19 declared as 17).
  **The bare arm** draws for the main hand only, from one named part with
  `resetPose()` plus a fixed `zRot` of ±0.1 rad. It rendered as *nothing* at
  first because **the model's UVs are texels, not fractions** — an arm's span
  16..56 of a 64 px skin, so remapping without dividing by the skin size sends
  them outside the atlas, where the sampler clamps to a transparent edge. The
  geometry was there the whole time (72 verts, uploaded), so looking proved
  nothing and printing the UV range settled it in one run. **623 tests**; demo
  PNG byte-identical to M15 onward. **The use-driven poses** landed with the plumbing they
  needed: right-click became a hold (`use_item` + `RELEASE_USE_ITEM`), and the
  local use clock needed **no new machine** — `startUsingItem` sets shared-flag
  bit 0, which `set_living_flags` already decodes, so the local id goes through
  the same door M23 built. Three invertible details: **`hasCustomArmTransform`
  moves a transform rather than adding one** (true for EAT/DRINK/SPEAR — the
  resting offset applies *after* the pose), the **brush cycles on
  `remaining % 10`** rather than on progress through a duration, and **BLOCK
  excepts a real shield** (it carries its own display transform and would be
  posed twice). Spyglass is the absence of a pose — vanilla guards all of
  `submitArmWithItem` on `!isScoping()`. Two gate witnesses failed first for one
  reason: **`transform_point3(ZERO)` sees only translation**, so a trailing
  rotation (the brush sweep) measures as motionless — sample an offset point.
  Gate **29/29**. **Open:** `SPEAR`'s use rig and the crossbow charge need
  inputs the wire does not carry; the arm wears the default skin.
- **M39 shift-click, the quick-move (2026-07-28)** — `ContainerInput.QUICK_MOVE`
  is a **different input**, not a modifier on PICKUP. **The routing is not "the
  other half of the inventory"**: `quickMoveStack` checks armour and the
  off-hand *first*, for an item that fits and whose target is **empty** — which
  is why shift-clicking a helmet equips it, and why a second helmet does not
  swap the first out. The crafting result is the one destination walked
  **backwards**, so a craft fills the hotbar from the right.
  **`moveItemStackTo` is two asymmetric passes**: the merge pass runs the whole
  range, the placement pass takes one empty slot and **breaks** — so a stack
  tops up a partial one before taking an empty, but never scatters across
  several. `doClick`'s outer `while` is what repeats it. Gate
  `inventoryshot --check` 44 → **49/49**, plus a live check: shift-click and
  plain click both accepted with **0 container resyncs**. **Open:** tooltips
  (needs `en_us.json` + text layout), drag/quick-craft, number-key swap,
  Q-drop, durability bars, armour icons.
- **M40 the rest of the inventory screen (2026-07-28)** — armour icons,
  tooltips, and every remaining interaction. **The suppressed items were
  suppressed for the wrong reason**: M22 called the five non-`model`
  definition types state-dependent, but **all 71 `select`s carry a
  `fallback`** and every `condition` an `on_false`, so for a component-free
  stack those *are* the answer, not a default. The rule is suppress the
  **property** you cannot evaluate, not the type — **1,390 → 1,438 resolved,
  147 → 99 suppressed**. The reduction must recurse (a bow is a `condition`
  whose `on_true` is a `range_dispatch`), and `display_context` selects
  different **geometry**, not a transform (a spear is a flat sprite in a slot
  and a 3D model in the hand), hence `HeldItemModel::gui_quads`. A witness
  caught the diagnostics naming the definition's *root* type rather than the
  node the walk stopped at. **Tooltips** are one line — the display name from
  the jar's `en_us.json`, preferring `block.minecraft.<id>` because
  `BlockItem` overrides `getDescriptionId` (`item.minecraft.dirt` does not
  exist); everything vanilla adds beyond it comes from a component Rewo cannot
  read. Layout traps: the height starts at **-2 for a single line**; the
  horizontal recovery is a **flip** whose `x` is the **already-offset** one
  (my witness expected 306, the answer is 318); the vertical is a clamp using
  `h + 3`. **The interactions**: `SWAP`'s button is a **third coordinate
  system** (an inventory index, 0..9 or a literal 40) and its range **rejects
  rather than clamps**; `THROW`'s trailing `while` never runs twice;
  `PICKUP_ALL` is two passes whose first **skips full stacks**, gated on the
  clicked slot being empty; `QUICK_CRAFT` packs `type << 2 | header` into one
  byte, is three packets, and **a one-slot drag collapses into a `PICKUP`**.
  Gates `inventoryshot` 44 → **70**, `itemshot` 28 → **33**; all four
  interactions live-verified with **0 container resyncs**. **Blocked, not
  skipped:** durability bars and enchantment/lore tooltip lines need the
  *contents* of a `DataComponentPatch`, which Rewo does not decode.
- **M41 the `DataComponentPatch` decode (2026-07-28)** — the blocker every
  milestone since M35 named. **The patch has no length prefix**: each entry's
  value uses that component's own stream codec, so an untranscribed one cannot
  be *skipped* — the reader parks mid-value and the rest of the packet is
  garbage. That is why M19 knew 3 of 111 codecs and treated the rest as fatal.
  Nearly all 104 syncable codecs compose from a dozen primitives, so
  `rewo-net/src/component_wire.rs` writes them as **data** (a `Shape` tree per
  component) and one interpreter walks them: **97 of 111 transcribed**, and 7
  of the 14 remaining are **never network-synchronised**, so there are 7 real
  gaps. Wire facts that read backwards: **a chat component is one NBT tag**
  (`fromCodecWithRegistries` — which makes `custom_name`/`item_name`/`lore`
  walkable with no chat codec at all); **`Unit` is zero bytes**; **`holderSet`'s
  var-int is `count + 1` and a literal 0 means a *tag name* follows**, not an
  empty set; `holder` is `id + 1` with 0 = inline while `holderRegistry` is
  raw; `either` writes **true for the left**. A sorted digest of every entry's
  (type id, raw bytes) makes **`isSameItemSameComponents` exact** — M35 could
  only ask "carries components at all", so every patched stack swapped and two
  identically-enchanted books could not stack; a *removal* folds in its id
  because `getOrDefault` answers it with the type's default, not the item's
  prototype. **Durability bars**: `round(13 - damage * 13 / max)` **counts
  down**, colour is `hsvToRgb(health / 3, 1, 1)`, the draw is a 13x2 black bed
  under a 1px bar, and `isBarVisible` is `isDamaged()` so a pristine tool has
  none; only the numerator is on the wire, so `gen_item_props.py` grew a
  `max_damage` column (84 items). Tooltips gained the name override, lore and
  `Unbreakable`. **Two witnesses caught real bugs** — the tooltip box was drawn
  in panel space while its text was in screen space (and `t4` had agreed with
  the implementation until rewritten to bracket the *text*), and `swingshot`'s
  "unwalkable" fixture named `enchantments`, which M41 transcribes, so it
  silently stopped testing its claim (now an impossible id). Gate
  `inventoryshot` 70 -> **79**; **628 tests**; live: named stacks **merge** and
  differently-named ones **swap**, 0 container resyncs. **Open:** the
  enchantment registry (a datapack registry Rewo does not decode) blocks the
  enchantment tooltip lines and the glint.
- **M42 the enchantment registry (2026-07-28)** — M41's other half. The
  registry **has to come from the wire**: `minecraft:enchantment` is a
  **datapack** registry, so its contents *and its id order* are the server's,
  and it arrives in Configuration's `registry_data` (kept in wire order — the
  index **is** the protocol id, the same rule M16 records for dimension types).
  `max_level` is **top-level in the entry compound, not nested under
  `definition`**, because `EnchantmentDefinition.CODEC` is a `MapCodec` whose
  fields inline into the parent. The **strings and tags come from the client
  jar** — `en_us.json` plus `data/minecraft/tags/enchantment/{curse,
  tooltip_order}.json`, the vanilla datapack the jar carries (where M19 already
  reads `ItemTags.SPEARS`). Three `getFullname` rules: the level numeral is
  suppressed **only when `level == 1 && maxLevel == 1`** (so a level-1 Mending
  has none and a level-1 Sharpness does — suppressing on `level == 1` alone
  loses it from every single-level enchant applied); a curse is **red**; and
  the order is the **`tooltip_order` tag**, then the rest. An unsynced id
  yields **no line**. **The render caught a bug**: `SlotText::is_empty` gates
  whether a stack's text is recorded at all and had not been taught the new
  field, so a stack carrying *only* enchantments looked empty and was dropped.
  Gate `inventoryshot` 79 -> **85**; **629 tests**; live, a four-enchantment
  sword renders curse-first-and-red, `Sharpness V`, `Unbreaking III`,
  `Mending` with no numeral. **Open:** the glint (a second render pass, not a
  tooltip concern).
- **M43 the enchantment glint (2026-07-28)** — a **second pass over the same
  geometry**, and almost all of it is state rather than new maths.
  `setupGlintTexturing`: two offsets on **110 s and 30 s** periods (so the
  pattern never visibly repeats), u **negative** and v positive (which sends
  the sheen diagonally), and the cast to `long` **before** the modulo. **JOML
  post-multiplies**, so `translation().rotateZ().scale()` reads as
  scale-then-rotate-then-translate on the coordinate — the reverse of the call
  order. Scale **8.0** for an item, 0.5 entity, 0.16 armour. **The UV fed in is
  the quad's own `0..1` coordinate, not its atlas position** — otherwise the
  pattern depends on where the packer put the item. Three pieces of pipeline
  state, each load-bearing: `BlendFunction.GLINT` is `(SRC_COLOR, ONE, ZERO,
  ONE)` so a dark texel adds nothing and alpha is left alone (the headless
  gates read it back); depth **EQUAL with no write**, which lands the sheen on
  the item's own fragments and nowhere else; and **REPEAT + LINEAR** sampling,
  because scale 8 samples far outside `0..1` and the `.mcmeta` sets
  `blur: true`. The phase is **wall-clock**, not the tick. **The render caught
  a bug**: `hasFoil()` is *not* `isEnchanted()` — `ENCHANTMENT_GLINT_OVERRIDE`
  wins **both ways**, so a golden apple can glint and a Sharpness V sword can
  be told not to. **And three `item_stack` fixtures rotted the same way
  `swingshot`'s did in M41** — they named a real-but-uncovered component id as
  their "unknown codec" and M43 gave it one; both now use an *impossible* id.
  Gate `inventoryshot` 85 -> **91**; **630 tests**; live, all four `hasFoil`
  cases correct and the sheen moves (311 of 2,500 slot pixels differ across
  seven seconds). **Open:** the glint on the first-person hand, on ground /
  mob-held items (scale 0.5) and on worn armour (0.16).
- **M44 the glint on the first-person hand (2026-07-28)** — M43's transform,
  blend, depth rule and sampler unchanged (the item scale is 8.0 in both
  contexts), so the milestone is about where the second pass hangs. **The
  glint geometry has to be the item geometry to the bit**: the pass
  depth-tests `EQUAL` against what the hand pass just wrote, so a vertex a
  fraction of a unit away is rejected fragment by fragment and draws nothing —
  the glint builder repeats the pose derivation (use branch, swing branch,
  display transform, left-hand mirror) rather than re-deriving it a second,
  subtly different way. **Only items glint** — the bare arm is skin, and
  `submitArmWithItem` takes the arm branch before any foil. `hasFoil` comes
  from the **inventory**, not the equipment feed, because a server never sends
  a player their own equipment. **The bug**: the first build drew nothing
  because `init_hand` *destroys and rebuilds the pass*, and the glint was
  installed **before** it, so every rebuild threw it away — no error, no
  warning, no validation message; a rebuilt pass with no glint is perfectly
  valid, and the only signal was two frames that should have differed and did
  not. Gate `handshot` 29 -> **34**; **635 tests**. **Open:** ground and
  mob-held items (scale 0.5) and worn armour (0.16), both through the entity
  pass.
- **M45 the glint on world-space items (2026-07-28)** — ground stacks and
  mob-held ones. `ENTITY_GLINT_TEXTURING`'s scale is **0.5** against the item
  contexts' 8.0, a factor of sixteen, so a dropped sword wears broad bands
  where an icon wears a fine weave. **Worn armour is the fourth surface and is
  not reachable**: Rewo renders no armour on any entity, so the 0.16 scale has
  nothing to apply to — the glint is complete for everything Rewo draws. The
  glint quads are pushed **from inside the two item emitters**, beside the
  vertex they shadow: the pipeline depth-tests `EQUAL`, and a dropped stack
  carries a death topple, a bob, a spin and a per-copy jitter, so a parallel
  derivation would have four more chances to disagree. It is a **third vertex
  range** (solid, text, glint) drawn after the solid pass and before the
  translucent ones, with **no lightmap term** — vanilla's glint shader
  multiplies by `GlintAlpha` and the fog fade and nothing else, so a dropped
  enchanted sword shimmers as brightly in a cave as in daylight. `hasFoil`
  rides in with the stack (on `HeldItem`, and in the `DATA_ITEM` metadata
  tuple) because it exists only in the component patch. **The gate measured
  zero and was right to**: `itemshot` calls `init_entities` directly rather
  than through the app's helper, so it never installed the glint — the same
  shape as the `swingshot`/`install_shapes` gap M41 hit, and the general rule
  is that *a gate reimplementing a slice of the app's setup will miss whatever
  the app adds to it*. `entities.rs` is also one of the **mixed CRLF/LF** files
  §0.0 warns about (1,969 CRLF against 3,763 LF), so the scripted edits had to
  match either ending. Gate `itemshot` 33 -> **37**; **629 tests**.
- **M46 worn armour (2026-07-28)** — M45 called this "the fourth surface and
  not reachable"; this makes it reachable. An item names an **asset**
  (`Equippable.assetId()`, in the prototype, never on the wire — so
  `gen_item_props.py` extracts it), and the asset names **layers** whose
  textures are **64x32** sheets, not 64x64 skins. Only two humanoid layers
  exist because `usesInnerModel` is `slot == LEGS`: the leggings sit *inside*
  the chestplate at deformation 0.5 against 1.0, which is what stops them
  z-fighting. **The body is in two pieces at once** — CHEST covers
  `{body, both arms}` and LEGS covers `{both legs, body}` — and the leg boxes
  are a **replacement**, `texOffs(0,16)` at `extend(-0.1)`. The armour is posed
  from the **same `xf` the body just used**, since it is a render layer over a
  model whose angles are already set. **The layer follows the RENDERER, not the
  mesh**: all eight `HumanoidArmorLayer` sites are player/zombie/skeleton/
  piglin families, so an **allay** (arms, no legs), an **illager** and a
  **creaking** — each with enough humanoid mesh to pass a geometric test — wear
  nothing in vanilla. **Only the player has a `body` part** (M19 gave it one
  for `setupAttackAnimation`); every mob's torso cube is on the static root, so
  a chestplate's body box resolved to nothing and mobs wore armoured arms over
  a bare chest — **and the witness passed anyway, because it asked the player
  model**, the one humanoid with the named part. A **trace beat four
  screenshots**: several rounds of squinting at crops (one of which was a husk,
  another comparing two live runs whose scenes had drifted) never settled
  whether the arms were armoured; logging which part each box resolved to
  answered it in one run, and the bare green mass every crop had been read as
  "arms" was the **torso**. An armoured zombie also rendered with a villager's
  texture, which looked exactly like an atlas collision from the fifteen new
  sheets — it is **pre-existing** (a stashed pre-M46 build reproduces it), needs
  more than one entity in the scene, and `mobshot` is structurally blind to it
  because its check substitutes per-face debug colours and so verifies UV/face
  correspondence rather than which *sheet* is sampled (recorded in §0.0). Gate
  `itemshot` 37 -> **42**; **629 tests**. **Open:** leather is undyed (a layer
  is a *list* — dyeable base plus overlay — and Rewo takes the first, so the
  greyscale base is never tinted by `dyed_color`), no trims, the inventory
  preview does not wear its armour, and baby mobs use the adult parts.
- **M47 the leather dye (2026-07-28)** — M46 shipped leather grey and called it
  "the dyeable base drawn untinted"; both halves were wrong. **Zero is not a
  black tint, it is "do not draw this layer"** — `renderLayers`' guard is
  `if (color != 0)`, and that is the entire implementation of
  `Layer.onlyIfDyed`, whose `Dyeable` carries *no* `color_when_undyed`. Three
  states hide behind one `Optional<Dyeable>` (absent = untinted always,
  present-with-a-colour = tinted always, present-without = only when dyed), so
  it survives as `Option<Option<u32>>`. **An undyed leather piece is brown, not
  grey**: `LEATHER_COLOR` is `0xA06540`, and the sheet is authored greyscale
  *because* it is always tinted — there is no path that draws it untinted. A
  layer type maps to a **list**: surveyed on the jar, 20 humanoid lists of one
  and 3 of two, all three of them leather's (a dyeable base plus an untinted
  overlay, which is what keeps the studs their own colour on a dyed piece).
  `DyedItemColor`'s stream codec is **`ByteBufCodecs.INT`** — a fixed
  big-endian i32 among the var-ints, M34's trap again — holding an **RGB**,
  which is why `getOrDefault` is the thing that calls `ARGB.opaque`, and why an
  absent dye is `0` while a *black* dye is `0xFF000000`. The tint is a **vertex
  colour** (`submitModel(..., color, ...)`; `entity.fsh` does
  `texture * vertexColor`), riding the same channel as the directional shade,
  so untinted is exactly `tint = 1`. **The pixel witness caught a key-format
  break**: `d4` measures red/green and red/blue over the armour's own pixels,
  and its first run measured **zero** — correctly, because M47 changed the
  atlas key to `<layer>/<texture>` while the renderer's slot filter still
  looked for `"/humanoid"` as a substring, so **all** armour had gone
  invisible, not just leather. Gate `itemshot` 42 -> **46**; **631 tests**.
  **Open:** no trims, the glint-order rule is transcribed but unreachable until
  armour glints, and `usePlayerTexture` (the elytra cape) is read as data and
  never honoured.
- **M48 armour trims (2026-07-28)** — the third armour layer, and the one that
  is **not a texture in the jar**: `armor_trims.json` declares a
  `paletted_permutations` source and the client generates every
  `pattern x material` sprite at load by swapping colours through a palette
  pair. Two invertible details — the match is on **RGB with alpha masked off**
  (so a half-transparent pixel of a palette colour still maps, taking
  `pixelAlpha * valueAlpha / 255`), and an **unmatched pixel is not dropped**,
  because `getOrDefault` returns `opaque(pixelRGB)` whose alpha 255 leaves it
  untouched. Working in RGBA bytes sidesteps whether `NativeImage.getPixels` is
  ARGB or ABGR. `trim_material` and `trim_pattern` are two more **datapack**
  registries (M42's rule: contents *and* id order are the server's; index = id),
  and their `MapCodec`s inline, so `asset_name`/`override_armor_assets` are
  top-level fields. **`assetId(equipmentAsset)` is what stops a trim
  disappearing**: it is `overrides.getOrDefault(equipmentAsset, base)`, keyed by
  the *equipment asset*, and iron/gold/diamond/netherite/copper each override to
  `<material>_darker` for their own armour — else an iron trim paints iron onto
  iron. The trim draws with **depth EQUAL, no write**
  (`ARMOR_DECAL_CUTOUT_NO_CULL`), M43's glint trick, and it is the only sane
  option: Rewo's reversed-Z `GREATER` would reject a coplanar redraw outright.
  Vanilla's two pipelines (`decal` vs not) **collapse to one here** because the
  trim's geometry is the armour's to the bit. It is a **fourth vertex range**
  (`solid | text | glint | trim`), drawn under the foil as vanilla does. 612
  possible sheets means a **demand-filled pool** (M22's item-pool arithmetic
  again): 64 slots, keyed by sprite path; `ATLAS_H` grew 1280→1408 with the
  pool at the **top** and the skin/item pools redefined downward, so every
  existing address is unchanged and `mobshot` stayed 243/243. **A leak the
  gates caught**: the new pipeline was never destroyed, and
  `VUID-vkDestroyDevice-device-05137` fired in three gates with **zero failed
  witnesses** — the 0-VUID bar caught what a green witness count could not.
  Gate `itemshot` 46 → **51**; **633 tests**. **Open:** trims are not on GUI
  icons (M40 suppresses the `select` property it cannot evaluate), no
  `humanoid_baby` layer, and a trim does not glint.
- **M49 trims on GUI icons (2026-07-28)** — the blocker M48 named. The icon is a
  **`select` on `minecraft:trim_material`** whose `when` values are material
  **registry ids** (not the `asset_name` suffix the worn sheet uses), each case
  naming a different model — 337 of them, each an ordinary two-layer
  `item/generated`. Its layer1 sprite comes from a **second** paletted-
  permutations atlas (`items.json`, four 16x16 sheets, same key palette and
  same sixteen permutations as `armor_trims.json`), so M48's `apply_palette`
  was already the whole generator. **The bake refactor**: `ItemModels` was
  keyed by item name and baked once, so a variant goes in under
  **`"<item>#<material id>"`** rather than the key becoming a pair — every
  existing lookup is untouched, and `HeldItems::any` falls back from a composed
  name to the base, which is required and not a nicety (an item can be trimmed
  with a material its own definition names no case for, and vanilla's answer
  there is the `fallback`). Variants come from the definition's own `cases`,
  not the material registry, because this is a bake of the **jar** and the
  registry is the **server's**. **The bug that hid the whole feature**:
  everything resolved and the icons still rendered plain, because a multi-layer
  sprite is **coplanar by construction** (`ItemModelGenerator` puts every layer
  in the same `z 7.5..8.5` slab) and the GUI pipeline depth-tested strict
  `GREATER`, rejecting layer1 at exactly layer0's depth. Vanilla tests `LEQUAL`;
  the reversed-Z counterpart is **`GREATER_OR_EQUAL`** — one word. That is the
  third time this arc a depth *comparison* was the whole story, so it is worth
  reaching for first when geometry is provably present and provably invisible.
  Gate `itemshot` 51 → **54** (`u1`: a variant bakes one more sprite layer than
  its base; `u2`: an unnamed material falls back to the base's single layer);
  **633 tests**.
- **M50 the worn-armour glint, and the glint's colour space (2026-07-28)** —
  M45 called worn armour "the fourth surface and not reachable"; M46 made it
  reachable and this draws it. **Two of the facts gathered in advance were
  wrong.** `VIEW_OFFSET_Z_LAYERING` is not the foil's mechanism: all three
  armour render types carry it (`ARMOR_CUTOUT_NO_CULL`,
  `ARMOR_DECAL_CUTOUT_NO_CULL`, `ARMOR_ENTITY_GLINT`), each with the same bias
  on a fresh `getModelViewMatrixCopy()`, so it **cancels within the stack** —
  what it separates is armour from *body* — and `RenderPipelines.GLINT` is
  `DepthStencilState(CompareOp.EQUAL, false)`, so the foil is the same
  depth-EQUAL pass Rewo had shipped three times. And the foil is **untinted**:
  `POSITION_TEX` has no Color element, `glint.vsh` declares no colour
  attribute, and `writeDynamicTransforms` passes `ColorModulator` as WHITE, so
  `submitModel`'s colour is dropped. The fact that held is the headline —
  **the trim must not glint**, because `renderLayers` clears `renderFoil`
  inside the layer loop and submits the trim after it. **Then the real
  finding**: the foil went in structurally correct and rendered a byte-delta of
  **exactly 0**. `BlendFunction.GLINT` is `(SRC_COLOR, ONE)`, so the
  contribution is `src²` — and **squaring is not invariant under the sRGB
  transfer function**. Vanilla evaluates it in gamma space (no sRGB framebuffer,
  no sRGB texture views); Rewo was blending in linear, where a mid texel adds
  +0.9/255 against vanilla's +16/255 and quantises away. **The item glint had
  the same error since M43** and hid it, because a dropped stack sits against a
  *dark* background where the sRGB curve is steep enough to show a tiny linear
  increment (measured on one frame: item 137, armour 0). No fixed-function
  blend can bridge it — every candidate needs to read the destination — so the
  glint now renders through a **UNORM view of the same image**
  (`MUTABLE_FORMAT` + format list on the offscreen image and the swapchain,
  `world::draw` reopening its scope around each glint draw, sheets uploaded
  UNORM), and both glint shaders are vanilla's line verbatim. **Without
  `VK_KHR_swapchain_mutable_format` no glint is drawn at all** — check that
  first if glints ever go missing. Structure: one `EntityGlint` per sheet over
  one shared pipeline, a fifth vertex range
  (`solid | text | glint | trim | armor_glint`), and the foil drawn **before**
  the trim because `SubmitNodeStorage` drains its phases in ascending `order`.
  Gate `itemshot` 54 → **62**; **633 tests**; demo PNG byte-identical to M15
  onward. **Three detector errors, all mine** (M38's pattern again): a `> 8`
  threshold built for the item glint read a real 5/255 sheen as nothing; a
  per-channel linear comparison sat below the 8-bit quantisation step; and the
  first fixture used two *bright* dyes whose red and green pinned at 255. The
  fix moved the measurement into the space the blend now works in — vanilla's
  add is base-independent **in bytes**, and the byte delta between two opposite
  dyes comes out **0**. **The live frame-diff was rejected as an oracle**: a
  same-item control differed in 41,284 pixels against the test's 16,329, so the
  wire path was verified by a *property* instead (`ench=[(28,4)]` decoded,
  `foil=true` at the renderer). And the first live run failed on the
  **harness**: the summon used the pre-1.21.5 `enchantments:{levels:{…}}`
  wrapper, so 26.2 silently produced an unenchanted piece — same shape as M35's
  stale state id and M20.1's build gate.
### The Velvet type stack, the visual freeze, and four headless subsystems (2026-07-28)

Pushed as `4c0fd6b..f7901f2`. Everything here is **headlessly verified** — the
demo PNG stayed `2cc56b4acbfb92cb` through all of it, which is the check that
none of it changes a rendered pixel.

**The module port (`M52a`).** The survey's top-ranked item. Full Bright, FOV
Control, Zoom, Toggle Sprint, Toggle Sneak, in `crates/rewo-app/src/modules.rs`.
The catalog is **not** redefined — `ewo_core::modules::REGISTRY` already calls
itself the single source of truth, so Rewo is its third reader; only `rewo-app`
takes the dep and `rewo-gpu` keeps taking plain floats. Config reads the **same
`profiles/<active>/modules.toml` the launcher writes**, so Settings → Modules
applies to a Native instance with no new contract.

Three invertible details: Full Bright pins vanilla's **maximum gamma** rather
than bypassing the lightmap, so night vision and darkness keep composing (a
bypass would silently defeat both); Zoom **divides** whatever FOV is in effect
rather than setting one, so it composes with FOV Control; Toggle Sprint/Sneak
guard on `!event.repeat`, or a held key flips the state dozens of times a
second.

**Two modules are vacuous in Rewo** — `no_view_bob` and `no_damage_tilt`
disable behaviours Rewo never implemented. They are absent from `RenderModules`
rather than wired to a no-op, with a test asserting toggling them changes
nothing. To port the disable you must first build the thing being disabled.

**The Velvet type stack (`M52b`).** See `REWO_VELVET_UI_PLAN.md`. Glyph cache
(`swash`, quantized key, shelf atlas, variable axes, blurred shadow glyphs),
text pass, SDF chrome pass, one widget (Coords), and `rewo hudshot --check`
(41 witnesses, mutation-verified). Load-bearing facts:

- **Rasterize-and-cache, not MSDF.** The fidelity target is pixel-faithful
  against the Skia originals and SDF reconstruction approximates the outline.
- **The key is quantized** (1/8 px, 1/2 axis unit) because an unquantized size
  mints a scaler per frame of a scale drag — the shape of the 2026-05-31 leak.
- **`swash`'s `linear_scale(s)` multiplies by a FACTOR; `scale(ppem)` divides
  by units-per-em.** Using the former returns font units: Fraunces' cap height
  read 25200 instead of 12.6 and every advance was ~1400× too wide. An
  assertion of `> 0.0` accepted it; only a two-sided bound caught it.
- **Six Skia `draw_rrect` calls collapse to one fragment shader**, because a
  mask blur over a rounded rect is a smoothstep over the SDF — no blur pass.
- **The Velvet passes must be built with `world::unorm_of(target_format)` and
  drawn inside `with_gamma_space`.** EwoClient's `rgba()` has no transfer
  function, so Skia composites in gamma space; an sRGB attachment blends in
  linear. The half that actually bites is the **pipeline format**: a mismatch
  is a validation error, not a subtle colour shift.

**The visual freeze.** Four steps in, the scope was cut back: the type stack
lands, the widget transcription **stops at one**, the editor is not started,
and the palette is de-baked **now** while it is one shader and one widget.
Reason: the HUD is getting a visual overhaul and anything transcribed now
would be redone. The music terms deliberately stayed structural — `border.a`
is the *resting* alpha and the drive gains scale from it, so a new palette
recolours without flattening the reaction.

**Tooltips through the Velvet pass (`M52b`).** The tooltip line went from
`(String, [f32;3])` — one string, one colour, nowhere to put "italic" — to
`tooltip::Line = Vec<Span>`. The fidelity gain is lore: `ItemLore.LORE_STYLE`
is `withColor(DARK_PURPLE).withItalic(true)` and Rewo had the colour right and
dropped the slant, because the type could not hold it. The `Span` type is
**font-agnostic** on purpose, so it outlives the visual direction.

The half that is easy to skip is **measurement**: once the tooltip draws in
Newsreader, sizing its box with the bitmap advances measures a font it no
longer uses. Also: vanilla's tooltip `y` is the line's **top** and Velvet lays
out from the **baseline**, and the atlas sync must run *before* the draw and
*outside* the rendering scope. The headless path deliberately passes `None`
and keeps the bitmap tooltip, so the gates' golden images are not moved by a
typeface change unrelated to what they test.

**Ping (`M52c`), and a correction.** The spec claimed the client could time a
keep-alive round trip. **It cannot** — `keep_alive` and `ping` are
*server-initiated* probes; the server sends, the client echoes, and the
**server** times it. A client cannot measure RTT from a packet it did not
initiate, and the play protocol gives it nothing to initiate. Vanilla's tab
list does not compute a ping, it displays one it was told. So the only source
is `UPDATE_LATENCY` on `player_info_update` — which Rewo was already decoding
and discarding as `let _latency = r.varint()?;`. One line was the whole gap.
A **negative latency is a state**, not a decode error (`PlayerTabOverlay`
buckets `< 0` into the no-connection icon), and `None` ≠ `Some(0)`.

**Chat styling (`M52d`).** `chat_style.rs` — legacy `§` codes and component
trees into styled runs, renderer-agnostic. (**It lived in `crates/rewo-net/`
until M126 moved it to `crates/rewo-world/`**, because `rewo_world::chat` has
to name `ChatSpan` and the dependency runs net → world; `rewo-net` re-exports
it, so the old paths still resolve.) Six rules a
plausible implementation gets silently wrong, each pinned: a **colour code
clears the five format flags** (`§c§lX` is bold red, `§l§cX` is plain red);
**`§r` resets to the enclosing style, not white**; an unrecognised code
consumes **both** characters; an explicit `false` beats an inherited `true`;
a `#` colour is `Integer.parseInt(_, 16)` **not CSS**, so `#f00` is `0x000F00`;
and a top-level list makes element 0 the **parent** of the rest.

**Component codecs + a latent bug (`M52e`).** The last 7 syncable
`DataComponentPatch` codecs. The gaps were fatal rather than cosmetic because
**the patch has no length prefix** — an untranscribed component cannot be
skipped. `can_place_on` needed a new primitive: it reaches
`TypedDataComponent`, which is the patch's own rule a second way.

It exposed a **latent bug in M41**: `MAX_DEPTH` charged the budget for every
combinator including static ones, free only because nothing was deep enough to
notice. `can_place_on` is — a legitimate adventure predicate would have
reported `Stuck` and cost the rest of its packet. Only recursive shapes charge
depth now. The 7 non-syncable components are **named in a test, not counted**,
so a version that starts syncing one fails as a missing codec.

**Tab list (`M52f`) and chunk cache (`M52g`).** Both model-only, nothing wired.
*(The tab list stopped being model-only in **M151** — see the entry at the end
of this section. The chunk cache is still unwired.)*
The tab list transcribes `PlayerTabOverlay` — cap 80, `MAX_ROWS_PER_COL` 20,
the four-key comparator (with `wrapping_neg`, because `-Integer.MIN_VALUE`
wraps in Java), the column-search loop, and the ping buckets. The chunk cache
is a Bobby-style store with a **version check by equality, not `>=`**, so a
downgrade cannot misread a newer file; `Container`/`Section`/`Column` fields
went `pub(crate)` so the encoder **destructures** — adding a field breaks the
build rather than silently writing an entry that decodes into a plausible
column missing the new state.

**Known limits, all recorded:** none of the four subsystems is wired to
anything; `ChunkCache` is not thread-safe and nothing decides when a cached
column is stale; ~~`TabEntry::team` is always `None` because Rewo does not
decode the scoreboard-team packet~~ — **wrong within days: M62 decoded
`set_player_team` (`teams.rs:344`) and M151 populates the field**, and the
sentence sat here uncorrected for four months because nothing consumed
`TabEntry` at all; and `TOOLTIP_TEXT_GUI_PX = 9.0` is an unverified calibration
guess awaiting one eyeball.

### Three headless wire subsystems (2026-07-28, second batch)

All three chosen by one test — **no eyeball, no design decision** — and each
completes something already built. Nothing is wired to a renderer.

**`bundle_contents` (committed as M61 — see the numbering caveat above; the
same number also names the wavy cape).** `container::bundle_chrome` and
`tooltip::bundle_image` were built and graded by `inventoryshot` but wired to
nothing, because `walk_item_template` discarded the id, count and nested patch
it read.

The design choice that matters: **capture and walk consume the same bytes by
construction, not by two implementations agreeing.** `walk_item_template` is
now `Ok(read_item_template(..)?.is_some())`. The patch has no length prefix,
so a capturing reader that drifted from the walking one would corrupt every
packet carrying a bundle.

Three states, not two: `None` is absence (resolves through
`BundleContents.EMPTY`), `Some(vec![])` is an *explicitly empty* bundle
(vanilla draws the empty-bundle blurb, not "no image"), and a removal resolves
like `None`. `selectedItem` is **not on the wire** — the codec maps through the
one-arg constructor, so a selection is client-side screen state.

**The blocker moved rather than closed.** It is no longer the decode, it is the
carrier: `ItemSlot` is `Copy` on purpose (the click arithmetic moves it through
a dozen struct-update expressions) and `SlotText` would need its `is_empty`
taught the new field, or a bundle carrying *only* `bundle_contents` is recorded
as textless and dropped — exactly the bug M42's enchantments hit. Also,
`getWeight` needs to know whether an element is itself a bundle or holds bees,
which needs the nested patch's *contents*; `patched` is one bit and cannot
answer it, so the grid and counts are drawable and the weight bar is not.

**M62 — the tab list's wire inputs.** `tab_list.rs` transcribed vanilla's
four-key comparator and three keys were inert. Now decoded: `tab_list_order`
(action 6), `gamemode` (action 2), and `set_player_team` (new
`rewo-net/src/teams.rs`, plus the `Scoreboard` state machine).

**It found a drift M52c introduced.** Extracting a pure parser so tests could
drive the real walk had created *two* copies of the entry walk, and they had
already diverged: the test copy capped a profile signature at 32767 where
`GAME_PROFILE_PROPERTIES` says **1024**, which the production copy had right —
so the tests were validating a walk the client does not use. They are now one
function that `apply_player_info` also runs.

Facts worth keeping: `GameType.byId` is `ByIdMap.continuous(ZERO)`, so an
**out-of-range mode is Survival, not an error** (same for visibility, collision
rule, team colour); every field is `Option` because the packet is a **delta**
and an unset action bit means *unchanged*, so defaulting would report a
spectator returning to survival on every latency-only update; a team packet
naming an **unknown team returns early and discards its roster**; and
`shouldHavePlayerList` includes method 0, so an ADD carries parameters *and* a
roster — mis-reading the parameters by one byte silently eats the roster.
Team-by-name → uuid is a **lazy two-step** lookup, which is what vanilla's
`PlayerInfo.getTeam` does and matters because the two packets have no ordering
guarantee.

**M63 — the sound packets, decode only.** Rewo has no audio at all, and the
survey puts ~117M downloads of demand behind that one prerequisite. Decoding a
packet needs no listening; making a noise does — that split is the task.
**No audio crate, no device, no mixer.** `sound`, `sound_entity`, `stop_sound`,
ids resolved by name and all `req!`. `custom_sound` does not exist in 26.2.

Four details where the wrong answer is plausible, all mutation-tested:

- Position is `(int)(coord * 8.0)` on the wire and the accessor is
  `this.x / 8.0F` — an **int/float** divide, so Java rounds to `f32` *before*
  widening to `double`. An `f64` divide agrees near spawn and drifts past
  ~2²¹ blocks; dividing by 16 puts every sound at half its true distance,
  audible as wrong attenuation and never as an error.
- The sound event is `ByteBufCodecs.holder` — `id + 1`, `0` meaning an inline
  definition follows. Reading it raw shifts every sound by one *and* then reads
  the inline body as the next field.
- `stop_sound` reads source **first**, then name, and only when its flag is
  set. Name-first works for flags 1 and 2 and corrupts flags 3.
- `sound_entity`'s id is a var-int where `sound`'s coordinates are fixed i32s.

The model lives in `rewo-net`, not `rewo-world`: `ParticleEvent` is in
`rewo-world` because `rewo-world` *simulates* particles, whereas a sound has no
client-side state, so filing it there adds a hop through a crate that only
forwards it.

**Integration hazard, recorded because it nearly bit twice.** These agents
branched from the same base and shared `play.rs`, `ids.rs` and `lib.rs`, so
each was applied as a **3-way patch, not a file copy** — a copy would have
compiled cleanly and silently deleted the previous one's work. Verify the
earlier symbols are still present afterwards rather than trusting the build.

**Mutation-testing found three decorative tests across the batch**, none of
them findable by reading. The sharpest: a depth witness sized as
`MAX_DEPTH + 2` is *self-calibrating* — raising the bound raises the payload,
so it passes at 8 and at 64 alike and only ever witnesses "recursion
terminates".

### The sound registry and the server-driven display packets (M64, M65)

Two more headless subsystems, same test as the batch before: no eyeball, no
design decision. Nothing is wired to a renderer.

**M64 — the `sound_event` registry table.** M63 named it as step 1 toward
playback. Parsed at load from the datagen report, matching
`particle_types.rs`; 1,968 entries, dense ids, both-direction lookup.

**The alphabetisation trap here is the sharpest "invisible to every gate" case
in the project so far.** `serde_json`'s default `Map` is a sorted `BTreeMap`,
so iterating `entries` hands you the registry **alphabetically**. The real 26.2
registry is not: ids 0–6 are the seven `entity.allay.*` events and
`ambient.cave` is id 7, where sorted order would put
`ambient.basalt_deltas.additions` at 0. An `enumerate()`-based table therefore
gives **a different wrong name for every one of 1,968 sounds** — and no decode
gate can catch it, because the ids still round-trip and the strings are still
real sound names. It is visible only to someone *listening*. Read `protocol_id`
off each entry; never derive an id from position.

Two resolution rules: an **inline** sound event returns its own identifier
*without* consulting the table, because it may name a resource-pack sound with
no registry id anywhere; and an unknown registry id returns `None`, never a
substitute — a wrong sound is harder to notice than a missing one.

**M65 — scoreboard objectives/scores/display, boss bars, tab header/footer.**
Six packets Rewo decoded none of. `Scoreboard` now **owns** M62's `Teams`
rather than sitting beside it, because vanilla's `Scoreboard` is one object and
the halves touch; `PlaySession.teams` became `PlaySession.scoreboard`.

**Two enum-decoding conventions sit one field apart, and only the decompile
distinguishes them.** `RenderType`, `BossBarColor`, `BossBarOverlay` and the
boss `OperationType` are `readEnum` — an array index, so out-of-range is an
**error**. `DisplaySlot` is `ByIdMap.continuous(…, ZERO)` — out-of-range is
**`LIST`**. Assuming either convention globally is wrong half the time.

Other findings, each witnessed: `NumberFormat`'s body length depends on its
registry id (`blank` is **zero bytes**), and an unnameable id is not skippable —
M41's no-length-prefix rule again — so it is a decode error; `reset_score` with
**no** objective name means *every* objective, not none; `set_display_objective`
naming an unknown objective **clears** the slot rather than being ignored, or a
stale sidebar is stranded; `removeObjective` keeps an emptied holder while
`resetSinglePlayerScore` drops one; a repeat boss-bar ADD replaces **in place**
(`LinkedHashMap::put` keeps insertion order), so re-pushing the id would
reorder bars on screen; and `tab_list`'s "no header" is a component whose
*flattened text* is empty, not an absent field.

**The mutation survivor was a real gap, not an equivalent mutant** — the first
in this project's batches where that turned out to be true. Deleting
`display.retain(...)` in `remove_objective` survived because the witness
asserted `display_objective(Sidebar).is_none()`, which resolves *through* the
objective map and so reports `None` for a stale entry too: **the witness was
measuring the wrong thing.** Fixed with an accessor that sees the stored name,
plus a behavioural test that re-creating a removed objective of the same name
must not resurrect its old sidebar — the actual bug, since servers run
remove/re-add cycles constantly.

**Integration.** Both agents branched from the same base and both added a
module, a `GameData` field and a load call to the same three regions of
`crates/rewo-data/src/lib.rs`. The 3-way patch **conflicted**, which is the
correct outcome — a file copy would have deleted the other silently. Both sides
were purely additive, so the resolution was the union.

**`crates/rewo-data/src/lib.rs` is the third file to hit the mixed-CRLF trap**
(after `entities.rs` and `chunk.rs`), and both agents hit it independently: an
editing tool normalised its 95 CRLF / 58 LF into a 60–123-line diff for a
7-line change. Both recovered byte-precisely. A `.gitattributes` policy would
retire this class of problem, but it touches line endings repo-wide and is a
decision, not a cleanup.

### The audio asset layer and the packet coverage audit (M66, M67)

Same test as the batches before — no eyeball, no design decision.
**`REWO_PACKET_COVERAGE.md` is the important artefact here**; read it before
planning any protocol work.

**The audit measured something nobody had.** Rewo's packet handling grew
milestone by milestone, so what it decodes was a *historical accident, not a
decision* — twice recently a whole family turned out to be simply absent
(M63's sounds, M65's scoreboard set), each found by noticing it was not in
`ids.rs`.

**141 clientbound-play packets: 56 consumed, 0 resolved-but-ignored, 85 never
resolved.** The zero is a real negative finding — the `cb_play_*` fields and
the dispatch chain agree exactly, so the whole gap is *names never resolved*.
The 85 split 31 pure-state / 20 needs-rendering / 23 needs-a-subsystem /
11 not-applicable.

**It also undermined a claim this file and REWO_PLAN both lean on.**
`rewo play`'s **`CORRECTIONS 0` proves less than it has been cited as
proving**: the harness walks on flat ground and is never knocked back,
exploded at, or mounted, so `explode` (whose `playerKnockback` is
`addDeltaMovement` on the local player), `set_entity_motion`, `move_vehicle`
and `set_passengers` are **structurally outside what it can test**. The number
is real and the physics port may well be right; the evidence is narrower than
the phrasing suggests. Treat it as "no correction *on the paths the harness
exercises*".

Two more gaps worth knowing: **`set_player_inventory`** is
`container_set_slot`'s index-addressed twin and only one of the pair was ever
handled — M34/M35 built a *predicting* inventory whose sole correction path is
a full state-id resync, and these exist to correct it without one. And
**`update_tags`**: Rewo reads `ItemTags.SPEARS` (M19) and the enchantment tags
(M42) from the **jar**, so a datapack that retags an item yields a wrong swing
duration or a missing tooltip line **with no error anywhere** — M64's
alphabetisation trap one layer up.

**"Handled" is not "complete"** (audit §4): six consumed packets decode less
than their body carries, and the greps the audit uses would call every one of
them handled. Sharpest — `game_event` consumes 4 of 14 types, so
`CHANGE_GAME_MODE`, the local player's own gamemode change, is matched and
dropped.

**M66 — the audio asset layer.** `sounds.json`'s weighted-variant index and
`level_event`'s id→sound table. **The data is not in the client jar** — it
arrives through the *asset index*, and so does every `.ogg`, which makes
`validateSoundResource` real rather than assumed: a variant whose file is
absent is dropped and the event's weights move with it. 1,968 events, 8,024
variants, 61 of them `type: "event"` **redirects** — and a redirect contributes
the **target's** total weight, not its own declared `weight`.

`forLocalAmbience(sound, **pitch**, volume)` takes pitch *second* — reading the
argument list left to right makes 1032's portal three times too loud at a fixed
pitch. `globalLevelEvent` and `levelEvent` are **disjoint switches**, so a
mismatched global flag is silence in vanilla too. Three ids are deliberately
unresolved rather than guessed, each with its derivation recorded: 1010
(jukebox song), 2001 (per block-state `SoundType`), 3008 (`BrushableBlock`).

**Still no audio.** No crate, no device, no mixer — that is the part needing a
human to listen.

**Two process facts from this batch.**

*A build passing proves the working tree is good, not the commit.* A disk-full
error during `git add` produced a commit that declared two modules whose files
were not staged — `cargo build` passed locally because the files were on disk.
Only `git show --stat` showed 2 files instead of 4. After any git operation
that errors, re-check what actually landed.

*The M-number ladder is now unusable as an index.* M52, M61, M64 and M66 each
name two unrelated pieces of work, because concurrent sessions numbered
independently. Read `git log --oneline` subjects.

### The three gaps the coverage audit ranked first (M68, M69)

The first work chosen *by the audit* rather than by what was next in a plan —
all three from `REWO_PACKET_COVERAGE.md`'s "pure state, no rendering" class,
all headlessly verifiable.

**M69 — the server's authoritative writes.** `set_player_inventory`,
`set_cursor_item`, `update_tags`. M34/M35 built a *predicting* inventory whose
only correction path is a full state-id resync; these are how the server fixes
it without one.

**`update_tags` exists in BOTH states — configuration 13 and play 134 — and the
audit listed only the play one, because the audit surveyed clientbound-play
only.** The *configuration* copy is what a vanilla server sends on join, right
after `registry_data`; the play copy is the `/reload` case. Resolving only play
134 would have looked like it worked until someone reloaded. That is a limit of
the audit's scope, not of the packet, and it is the first thing a whole-protocol
sweep would catch.

**The two inventory coordinate systems are worse than a different origin.**
`InventoryMenu`'s `SLOT_IDS` is `{HEAD, CHEST, LEGS, FEET}` at backing index
`39 - i`, so **the armour ranges run in opposite directions**: inventory 36 is
FEET / menu 8, inventory 39 is HEAD / menu 5. Subtracting a constant — the
obvious reading of "36 here, 5 there" — puts boots on the head, and produces
output of the right type, in the right range, for the right item category. No
decode gate can see it. There is now one conversion
(`menu_slot_of_inventory_index`) and `Inventory::hotbar` routes through it.

It returns **three** outcomes, not two: `Applied`, `NoMenuSlot` (indices 41/42 —
`SLOT_BODY_ARMOR` and `SLOT_SADDLE` are real `EntityEquipment` slots the 46-slot
menu does not expose), and `OutOfRange`. Collapsing the middle makes "Rewo has
nowhere to put this" look like a decode failure. Same instinct one layer up:
`TagOverrides::contains` returns `Option<bool>` so silence is distinguishable
from a negative — a bare `bool` reads every unsent tag as "not a member", which
poses every spear as `ArmPose::Item` against a server that omits the item
registry.

`set_player_inventory` carries **no state id** and bypasses the container menu,
so the write must not touch `state_id` — advancing it would make the next click
echo a number the server never issued, which is the exact resync the packet
exists to avoid. Its slot is a **VarInt**, not the `i16` its sibling
`container_set_slot` uses: M34's recorded trap does not generalise.

The tag override is modelled and **deliberately not wired**. M19's `SPEARS` is
one `ItemTag::from_ids` away but needs ~8 call sites plumbed and no gate would
grade it. M42's enchantment tags are **blocked, not unplumbed** — `rewo_data`
stores names where the packet carries ids, and bridging needs the wire-order
registry read at a moment the two packets have no ordering guarantee about.

**M68 — the four packets that move the local player.** `explode`,
`set_entity_motion`, `move_vehicle`, `set_passengers`.

**My brief for this one was wrong, and verifying it was the most valuable thing
the agent did.** I said velocity was fixed point, thousandths of a block per
tick, as a short. **That encoding does not exist in 26.2.**
`ClientboundSetEntityMotionPacket` composes `Vec3.LP_STREAM_CODEC` →
`net/minecraft/network/LpVec3.java`: three 15-bit mantissas against **one shared
integer scale**, a **one-byte zero sentinel**, and an optional continuation
VarInt. No `8000.0` exists anywhere in the protocol tree. Implementing the brief
as written reads 6 bytes of a body that can be 2.

Two more, both verified: `move_vehicle` carries **no entity id** (the client
resolves `getRootVehicle()`), and `explode`'s `blockCount` is a **fixed
big-endian i32 between `radius` and the knockback**, so a VarInt reading
silently reports "no knockback" from a packet carrying one. And `explode`'s
knockback is `Vec3.STREAM_CODEC` (full doubles) where `set_entity_motion` is
`LP_STREAM_CODEC` — **two different `Vec3` encodings in adjacent packets**.

**The gate is the point, not the decode.** `rewo play --motion-check` drives a
paced command stream (boat → `ride mount` → `ride dismount`; then resistance →
TNT → `/damage … by <zombie>`), fail-closed on **observation** — a command the
server ignored leaves a counter at zero and turns it red.

**A live mutation found a bug in the gate itself**, and the important half is
not the timing slip: **the correction meter structurally cannot catch a dropped
knockback.** Vanilla's move check flags a client that moves too *much*; one
ignoring a shove moves too *little*. The witness is now the measured change in
the client's own velocity. So, precisely — `CORRECTIONS 0` over a flat walk is
unchanged and still true; the knockback path is now exercised; but *correct
handling* rests on the |Δv| witness, not the meter. **Riding accuracy is
unprovable by any correction count** — `ServerGamePacketListenerImpl` skips move
validation entirely for a passenger — so mount-phase corrections are reported
and explicitly not graded.

`move_vehicle` is **structurally unreachable** and the gate says so rather than
passing quietly: both send sites are inside `handleMoveVehicle`, the server
*rejecting* a serverbound vehicle move, which a passenger-only client never
provokes.

**The collision that the build would not have caught.** A concurrent session
landed **M70** (entity-label visibility) between M68's base and `main`, and it
decodes `set_passengers` too. The 3-way patch applied **cleanly** and left a
duplicate struct field *plus a silently unreachable second dispatch arm* — the
field fails the build, the arm does not. The two effects are disjoint (M70
builds the riding graph that suppresses a ridden entity's floating label; M68
applies the local player's mount state to physics), so the resolution is one
field and one arm doing **both**, not a winner. `body` is a `&[u8]`, so the
second read is safe and deliberate — folding either decode into the other would
couple two milestones with no reason to share a walk. **The general lesson: a
clean 3-way apply is not evidence of no collision.** Grep for the symbol.

`motion.rs` also arrived 991/991 CRLF while every other file in its crate is LF;
normalised rather than left to become the fourth file in the mixed-endings trap.

**Gates:** rewo-net 390, rewo-world 291, rewo-app 80; `inventoryshot` 143 →
**152**, `mobshot` 243/243, `swingshot` 97, `eventshot` 28; demo PNG
`2cc56b4acbfb92cb` byte-identical. **27 mutations across the two milestones, 26
caught**; both survivors were real — M69's was a `a != b || { true }` tautology
the agent found in its own witness before the battery ran, and M68's was a test
asserting only `is_err()` where both the intended and the mutated path error and
only the error's *shape* distinguishes them.

### M71 — the ten `game_event` types, and what "handled" was hiding

The audit's §4 claim — **"handled" is not "complete"** — worked as a closed
example. `game_event` passed every grep as handled and consumed **4 of 14**
types: M33 took the four weather ids and the other ten were matched and thrown
away, `CHANGE_GAME_MODE` among them.

**There are two params, not one.** Vanilla computes
`int param = Mth.floor(paramFloat + 0.5F)` at the top of `handleGameEvent`, and
**only** `CHANGE_GAME_MODE` and `GUARDIAN_ELDER_EFFECT` use it. Every other
branch reads the **raw float** — `DEMO_EVENT` compares exact literals
(`101.0F`, `102.0F`…), `IMMEDIATE_RESPAWN` is `p == 0.0F`, `LIMITED_CRAFTING`
is `p == 1.0F`. Using one where the other belongs is invisible for every
integral param a server actually sends.

**`IMMEDIATE_RESPAWN` is inverted, and the name is what inverts it:**
`setShowDeathScreen(paramFloat == 0.0F)`, so param **0 shows** the death screen.
Transcribe from the event name rather than the setter and you get a client that
shows the screen exactly when it should not.

**An unknown type id is a silent no-op, not an error.** `Type.TYPES.get(id)`
returns null and every `==` against it is false. The instinct on a decode task
is to make an unrecognised discriminant an error; here that would disconnect a
client from a server sending a type it merely does not care about.

**`ClientLevel.playSeededSound` reads backwards from the server.** Its body is
`if (except == this.minecraft.player)` — the client plays the sound **only**
when the "except" argument *is* the local player, which is the opposite of what
the parameter means server-side. `handleGameEvent` passes the local player, so
all three of its sounds are audible; any other reading is silence.

Three more, each verified: the join-time values of `IMMEDIATE_RESPAWN` /
`LIMITED_CRAFTING` ride the **login packet**, so ids 11/12 are only the
mid-session gamerule change; `setLocalMode` guards the previous-mode write on
the mode actually changing, so a repeat must not clobber it; and `handleRespawn`
copies `showDeathScreen` to the new player but **not** `doLimitedCrafting` —
that asymmetry is vanilla's, and is why nothing is cleared on a dimension
change.

**Applied 7, modelled 3, one deliberately left homeless.** Applied:
`CHANGE_GAME_MODE` (reusing M62's `GameMode::by_id` — `ByIdMap.continuous(ZERO)`,
so out-of-range is Survival), `IMMEDIATE_RESPAWN`, `LIMITED_CRAFTING`,
`NO_RESPAWN_BLOCK_AVAILABLE` (queued as a **translation key** and resolved
against `baked.lang` at the edge, which is what `Component.translatable` does),
and the three sounds into M63's queue. Modelled only: `WIN_GAME`, `DEMO_EVENT`,
`LEVEL_CHUNKS_LOAD_START` — screens and a load tracker Rewo has no equivalent
of, and Demo's hints need keybind names Rewo cannot supply, so the hint is
recorded rather than fabricated. **Homeless on purpose:**
`GUARDIAN_ELDER_EFFECT`'s particle, because `ELDER_GUARDIAN` is not one of M37's
six transcribed kinds and M37's own rule is that an unknown kind is dropped
rather than rendered as something else.

**Gamemode is modelled, not acted on.** `rewo-world::physics` has no flight,
no-clip or invulnerability concept and neither `player_abilities` packet is in
`ids.rs`; the four-step job is written into the coverage doc's new §4.1 rather
than half-started.

**Two structural findings, both from mutation testing, and both bigger than the
milestone.** `PlaySession`'s fan-out was **entirely unwitnessed** — it owns a
socket and there is no test module for it anywhere in the repo, so dropping the
weather branch, the state branch or the eye-height all survived the whole suite.
The logic moved into a tested `game_event::apply` behind a 6-line adapter, with
a signature taking `&PlayerState` rather than loose coordinates so a transposed
axis or an `eye_y`-for-`feet_y` swap is *unrepresentable*. And the first
refactor left `weathershot` grading a path the client no longer took — **M45's
`install_shapes` failure exactly**: a gate that reimplements a slice of the
app's setup misses whatever moves out from under it. Caught by mutating the
weather branch and watching the gate drop 35 → 32.

**Gates:** rewo-net 418, rewo-world 291, rewo-app 80; `weathershot` 35/35,
`inventoryshot` 152, `particleshot` 34; demo PNG `2cc56b4acbfb92cb`
byte-identical.

### M72 + M73 — the two halves other milestones had to stub

Both landed from concurrent sessions and both close a gap an earlier milestone
*recorded rather than faked*, which is the pattern working as intended: M70 and
M68 each wrote down what they could not evaluate, and someone later read the
note and evaluated it.

**M72 — where a rider actually sits.** M70 decoded `set_passengers` into a
riding graph and consumed it for `Entity.isVehicle()` alone, so a rider still
rendered at its own stale synced position. **The seat is entity-type DATA in
26.x, not a constant** — `getPassengerRidingPosition` reads
`EntityDimensions.attachments()`, declared by the `EntityType` builder
(`tools/gen_entity_attachments.py`, 158 types, 57 declaring seats, 24 a vehicle
point). Three builder conventions invert if assumed: a bare float in
`passengerAttachments` is a **Y offset**, `ridingOffset(r)` is **negated** into
the VEHICLE point, and PASSENGER's fallback is **AT_HEIGHT** (the top of the
bounding box), not AT_FEET.

**There are two tables, keyed by two types.** `positionRider` subtracts the
*rider's own* VEHICLE point, rotated by the *rider's* yaw — a player's is
`(0, 0.6, 0)`, which is the whole reason a mounted player sits in a saddle
rather than standing on the horse's head.

**A passenger does not interpolate**, and the pre-M72 error was never a constant
offset — **it was a lag**. `ClientLevel.tickEntities` skips passengers outright;
a rider is reached only via `tickPassenger` → `rideTick`, which ticks it and
*then* overwrites its position. So Rewo derives into `cur` at the end of
`tick_lerp`, after every entity's own step has moved `prev = cur`.

Overrides dispatch on the **Java class**, most-derived-first, because that is
what `super` does. Gate: `rewo rideshot --check`, 24 witnesses. **18 mutations,
17 bit first time**; the 18th repeated M70's `b4` in a new shape and its named
partner was *unreachable by construction* — **a named mutation partner that
cannot be reached is not a partner**, so the witness was rewritten to name the
detach that is load-bearing.

**M73 — the entity raycast, and the label clause M70 stubbed.** `shouldShowName`
is `entity.shouldShowName() || (hasCustomName() && entity ==
crosshairPickEntity)`, and Rewo's raycast was voxel-only, so M70 transcribed the
second disjunct, graded it both ways, and fed it a hard `false` live.

**It is not a second, label-only raycast.** `Minecraft.pick` assigns
`crosshairPickEntity` from *the* hitResult — the same one that decides which
block you are mining. In 26.2 that lives in a private static `LocalPlayer.pick`,
not in `GameRenderer`.

The inflation is `entity.getPickRadius()` — **0.0F for everything but a
Projectile** — and **not** the `DEFAULT_ENTITY_HIT_RESULT_MARGIN = 0.3F`
declared beside it, which belongs to the projectile overload. **`isPickable()`'s
default is `false`**: a dropped item, an experience orb and a text_display are
invisible to the crosshair, and so is the ender dragon, which overrides it back
to false and delegates to unregistered `EnderDragonPart` hitboxes.

**Neither range is hard-coded** — both are RangedAttributes, so creative mode's
`+2.0` entity-range modifier applies by itself. That exposed a real gap:
`apply_update_attributes` opens with `getEntity(id) == null` and **the local
player is not in the EntityTable**, so every snapshot addressed to it was being
dropped. `PlaySession` now keeps `local_attributes`.

**A mutation survived and found something.** `g5` claimed a dead heat goes to
the block via `>=`, and mutating it to `>` left the gate green — vanilla
enforces that precedence **twice** (the sweep is truncated at the block hit
*and* the survivor is compared against it), and because the truncation feeds
`maxValue`, whose test is strict, the tie is already excluded by the sweep
bound. **Neither half alone is observable.** Also: two witnesses hand-computed a
`0.3` half-width and landed a hundred-millionth off the bound they claimed to
sample, because **vanilla halves the width as a float** — a mob's near face sits
at `x - 0.30000001192`.

Both sessions updated `REWO_PLAN.md` and the coverage doc but **not `CLAUDE.md`**
— these entries are that catch-up. **Merged state: 1247 tests** (net 420, world
318, gpu 205, data 175, mesh 38, proto 11, app 80); `rideshot` 24, `labelshot`
47, `weathershot` 35, `inventoryshot` 152, `mobshot` 246/246; demo PNG
`2cc56b4acbfb92cb` byte-identical.

### M54–M86 — the fidelity arc, the coverage sweep, class B, and the bug eighty milestones of gates could not see (2026-07-28 → 07-30)

Twenty milestones, caught up here on 2026-08-02 after a session found this file
still ending at M73. **Verified from a cold start rather than read off a doc:**
`origin/main` `0ddbc66`, tree clean, **no branch anywhere holds a commit not on
`main`**; release build clean; **1623 tests, 0 failures** (net 565, world 489,
gpu 249, data 179, app 85, mesh 45, proto 11); `mobshot --check` **246/246**;
**32 serverless gate commands** green, 0 VUIDs; demo PNG still
`2cc56b4acbfb92cb`.

**`REWO_PACKET_COVERAGE.md` is at 107 consumed / 0 ignored / 34 absent, and
classes A and B are both empty** — every clientbound-play packet Rewo *can*
render is rendered. The 34 remaining are 23 needing a subsystem Rewo lacks
(container/menu screens ×6, recipe book ×5, chat input ×4, advancements ×2,
resource-pack fetch ×2, dialog ×2, map + transfer ×2) and 11 not applicable.
**Picking work there now means choosing a subsystem, not a packet.**

**M54–M60 — data and fidelity.**

- **M54 the language map.** `en_us.json` **is not the language map**; it is step
  1 of three. `loadFromJson` rewrites every unsupported format specifier
  (`%d`/`%f` → `%s`, which is why `decomposeTemplate` only understands `s`;
  inert on 26.2 and transcribed anyway, because its absence is invisible until
  a pack carries a `%d` and the whole line collapses to its raw pattern), then
  `deprecated.json`'s `applyToMap` applies **383 removals and 146 renames**
  — remove first, then rename, and the order is load-bearing. **105 of the 146
  rename targets do not appear in `en_us.json` at all**, and 41 that do are
  overwritten, changing **27 item display names** (the eighteen smithing
  templates stop reading "Smithing Template") — every change *toward* vanilla.
- **M55 entity attributes.** `MAX_HEALTH` is not metadata, it is an
  **attribute**, and `update_attributes` (131) was falling off the dispatch
  chain. The holder is `holderRegistry` — **raw 0-based**, third time this has
  bitten (M16 dimensions, M21 damage types) — and here the failure is *quiet*:
  `max_health` is 23 and `max_absorption` 22, both real syncable attributes on
  the same entity, so an off-by-one clamps against the wrong range rather than
  throwing. The operation is a **VarInt, not a byte**, and an out-of-range id
  is `ADD_VALUE`, not an error. `ADD_MULTIPLIED_BASE` reads the *post-*`ADD_VALUE`
  base and every such modifier reads that same base — they do not compound.
- **M56 the tooltip's image pass.** `GuiGraphicsExtractor.tooltip` walks its
  components **twice with `localY = y` between**, and the two loops advance
  identically — so the split is a **layering device, not a layout one**: it
  guarantees every image draws after every text line whatever the component
  order. Run as one cursor and the grid drops below its box by the height of
  all the text (57 px in the fixture). `lines.size() == 1 ? -2 : 0` counts
  **components**, not text lines. Three brief errors the decompile settled: the
  `+N` badge is the **bottom-right** cell; thirteen stacks show **eight** items,
  not twelve; and the badge counts hidden **items**, not stacks (thirteen full
  stacks badge `+320`, never `+1`).
- **M57 entity fidelity — emissive, ETF, the dye tint.** Eight mobs have
  emissive layers in vanilla and none glowed; a warden, whose whole visual
  identity is bioluminescence, had none. Both `RenderLayer` shapes re-render the
  mob's **own model** with a second texture at full brightness, so the geometry
  is the same quads re-pointed at another texture. The warden's tendril layer
  samples the **base** warden texture, not an overlay.
- **M58 the bundle grid's chrome.** `container/bundle/slot_highlight_back` is
  **not** the `container/slot_highlight_back` M35 already loads for the
  inventory hover box — both exist, both 24×24, and reusing the inventory's
  renders something that looks approximately right. The badge cell gets **no
  chrome at all**: `extractCount`'s entire body is one `centeredText`.
- **M59 the health bar's render half — the first Rewo feature with no vanilla
  oracle.** Vanilla renders no health bar over any entity, so there was nothing
  to transcribe; the numbers were written down first as a *decision*
  (`REWO_HEALTH_BAR_SPEC.md`) and the gate grades against that. **The gate
  re-declares the spec's constants rather than importing the implementation's**
  — importing them asserts only that the implementation equals itself, which is
  M41's `t4` failure mode exactly. Two spec witnesses are unobservable from
  outside and say so in their detail strings rather than being quietly dropped.
- **M60 the vanilla cape.** Scoped as needing "the milestone's one structural
  change" (`Rx·Rz·Ry`, which Rewo's `Rz·Ry·Rx` parts cannot produce) and needed
  **none**: `rotateBy`'s leading `rotateY(-PI)` exists to **cancel the pose**,
  so the net rotation *replaces* the `PartPose` rotation — while the pose's
  **translation still applies**, which is the asymmetry that makes it easy to
  get wrong in either direction.

**M74–M78 — the coverage re-audit, then the class-A sweep.**

- **M74 the re-audit.** Ten of 141 rows were wrong, **all in one direction**
  (`absent` about code that was present). The mechanism is not neglect: M67
  wrote the table by grepping a moving tree and four packets landed the same
  day. M67 *saw* it happening and worked around it twice, both of which made it
  worse — a predictive "After §7" column describing a moment that never
  existed, and milestone markers written into the **status** column, putting
  four rows outside any grammar a future check could read. **Annotating decay
  is not fixing it.** The fix is a unit test in `ids.rs`, deliberately *not* a
  `*shot` gate, because it must fire on the event that **causes** the drift
  (someone editing `ids.rs`). It also found a **live flow-control divergence
  hiding as a missing decode**: Rewo answered every `chunk_batch_finished` with
  a hard-coded `64.0` where vanilla's seeded opening bid is **3.5**, so it
  over-bid the server ~18× on every batch of every session and never adapted.
- **M75 abilities and flight.** The flags byte is `1/2/4/8` then two floats —
  nine fixed bytes; **the serverbound twin is one byte**, so writing the
  clientbound body there desyncs the stream by eight. An unauthorised flying
  claim is **ignored, not kicked**. **Flight does not go through
  `travelFlying`** — that was the central misdirection, and the method is for
  mobs and swimming. `Player.travel`'s flying arm captures `originalMovementY`,
  delegates to the *ordinary* `travelInAir`, then **overwrites** the Y it just
  computed with `originalMovementY * 0.6`: so flight has **no gravity term**,
  vertical drag **0.6**, and flying into a ceiling does not zero your upward
  velocity. `walkingSpeed` is **not** the client's walking speed (its only
  client consumer is the FOV modifier's divisor). `SPECTATOR` sets `flying =
  true` while `CREATIVE` only sets `mayfly`.
- **M76 rotation and world spawn.** The brief *and* this project's own coverage
  doc were wrong about the headline: `player_rotation` carries **no relative
  bitfield**. It is four fields with each `BOOL` sitting **after** the float it
  qualifies — and a reader written from the wrong description **decodes every
  packet without erroring**, because the arity happens to work out. The
  `Set<Relative>` is real one layer up, so the two teleport packets **share
  their semantics and not their layout**. The clamp is on the **sum**, not the
  step; the yaw gets neither clamp nor wrap.
- **M77 the minecart's own interpolation.** Framed as replace-or-feed; the
  answer is **neither** — it overrides the generic lerp at the *render* seam and
  leaves it running, and four separate places in the decompile have to agree for
  that to be true. Vanilla **measures one against the other**: a passenger's
  offset is literally the schedule minus the generic lerp. Mirror image of M72,
  where the rider's own lerp is computed and thrown away.
- **M78 session, server metadata, chat.** `bundle_delimiter` is a **pipeline
  instruction**, not an inert packet — its `handle` throws `AssertionError` if
  it ever reaches a listener, so decoding it as a no-op is the one way of being
  wrong that leaves no trace. A bundle is applied **all at once on close**;
  **the coverage doc's "in one tick" was wrong** — nothing defers a bundle to a
  tick boundary, the guarantee is that no *frame* renders part-way through. An
  unterminated bundle is **withheld**, neither dropped nor applied. There is **no
  nesting**, so a depth counter — the natural implementation — never closes.

**M79–M85 — class B, everything that needed a renderer.** The recurring finding:
**the class letter changes the gate, not the standard.** Each of these has an
exact vanilla oracle, so decode *and* render are transcribed line by line and
graded, with a pixel read-back half on top of the model half.

- **M79 titles, XP, cooldown.** A subtitle on its own **shows nothing** (only
  `setTitle` arms the clock). A negative animation field means *leave
  unchanged*, and the packet **re-arms a live title at its full duration** —
  `/title times` mid-title hands the title its whole life back. `/title clear`
  and `/title reset` differ in what the *next* title does, not what is on
  screen. **`set_experience`'s wire order is not its declaration order**, and
  reading top-to-bottom swaps two var-ints, decodes without erroring, and puts
  lifetime XP in the level display.
- **M80 the world border — six packets, one object.** Splitting the decode from
  the wall would have left the state machine with nothing to test against. The
  lerp's clock is **ticks**, not wall-clock, and the `gameTime` argument is
  **inert** — but the wall's *texture scroll* really is wall-clock milliseconds,
  the only such quantity in the feature, so the instinct was right one layer
  over. **`getMinX()` is the previous tick's size**: every non-rendering
  consumer (collision, the vignette) measures against the previous tick's box
  while the renderer alone passes a real partial.
- **M81 the hurt tilt, block cracks, item pickup.** Packet 42 is what made
  `no_damage_tilt` real — the Velvet batch's *"to port the disable you must
  first build the thing being disabled"* named the condition and this was it.
  It drives vanilla's own `damageTiltStrength` accessibility slider to its off
  end rather than branching around the tilt, so toggling mid-animation cannot
  strand the camera at an angle. **The server already subtracted the camera
  yaw** before sending, so the tilt direction is **frozen at the hit** and does
  not track subsequent turning.
- **M82 the screen framework and the death screen.** The coverage doc called the
  screens "a design decision rather than a transcription" — **half right**. The
  decision was real and *smaller* than it sounded: **vanilla has one screen
  slot, not a stack**; the nesting that looks like a stack is a replacement
  carrying a `BooleanConsumer`. The rest was ordinary transcription with the
  usual inversions: a hovered **disabled** button draws the plain disabled
  sprite (the three-arg `WidgetSprites` makes `disabledFocused` *be*
  `button_disabled`); `isHovered` and `isMouseOver` disagree **on purpose**, and
  because `getChildAt` uses the latter an **inactive** widget is not found at all
  and the click falls straight through; **`Esc` does nothing** on a death screen
  and `setScreen(null)` *re-opens* it.
- **M83 the locator bar.** `writeEither` writes **`true` for the left** (the
  UUID). The identifier is the **colour of last resort**, and a live vanilla
  server sends `colour=None`, so on a real connection the hash **is** the
  colour. The self-skip is gotcha 13 in both directions at once: the observer is
  never in `EntityTable`, so it must come from the session's own UUID — and a
  client that dropped the check **looks perfectly correct on vanilla**, because
  the server never sends you your own waypoint.
- **M84 the statistics screen — the packet that closes class B.** `Stat`'s
  two-level dispatch would normally be the `DataComponentPatch` hazard in
  miniature (an untranscribed variant cannot be skipped), and **here it cannot
  happen**, structurally: every `StatType`'s second level is a single VarInt, so
  what the first level selects is *which registry resolves the id*, not a
  different wire shape. Resolution is deferred, so an unresolvable value costs
  one dropped row rather than a dropped packet. **`StatsScreen.isInGameUi()` is
  false**, so it does not dim the world the way the inventory does.
- **M85 server links.** Three of the four things the brief said about the packet
  were corrections. The pause screen shows **one button, not a list** (it opens
  a separate dialog screen — three screens, not one). The disconnect screen
  shows **at most one link and only ever `BUG_REPORT`**, filled only on the
  client's *own* error paths — so **a server that kicks you politely shows no
  link however many it advertised, and one whose packet crashes your client
  shows exactly one**. And the packet exists in the **configuration** state too;
  third time (M69 `update_tags`, M78 `custom_payload`), so the rule is now
  reliable: **if the handler is on `ClientCommonPacketListener`, look for the
  configuration copy.**

**M86 — the bug eighty milestones of gates could not see.** `LiveApp::resumed`'s
init closure did `self.baked.take()` and **dropped the bake at its closing
brace**, so `self.baked` was `None` for the whole windowed session and every
`if let Some(baked) = self.baked.as_ref()` in `LiveApp::frame` was dead code.
**Nine shipped features had never once rendered in `rewo live`** — item icons
(M34), the inventory screen (M35), the player preview (M36), the first-person
hand (M38), clouds and precipitation (M33), the rain-fog band (M33b), particles
(M37), the world border (M80) and block-breaking decals (M81). Live since M3.
All of them are honest *headlessly*, because `run_headless` owns the bake as a
plain value — **which is exactly why eighty milestones of gates never saw it**.
The restore is four lines.

It was **not landable alone**: turning the paths on took a 10-second windowed
run from 0 validation errors to **40,532**, every one
`VUID-vkDestroyBuffer-buffer-00922`. **Eight** passes opened their `set_*` with
`free_buf(gpu, self.vbuf.take())`, destroying a buffer submitted command buffers
still reference — unobservable before only because none was ever constructed in
the windowed client, and unobservable headlessly because a one-frame oracle
never overlaps itself. The rule that came out of it, now in
`crates/rewo-gpu/src/buf_ring.rs`:

> **`ring >= fif + 1` for a ring written before `render`; `ring >= fif` for one
> written inside it** — because a `set_*` runs in the app's frame loop *before*
> `render`, so the most recent fence wait was the *previous* frame's.

**The gate it left behind is the one to remember: `rewo live --render-check`.**
It is the only check that drives the **windowed** client, and therefore the only
one that can see a render path the windowed client never reaches. **Run it after
any milestone that adds one.** It does not stage its own hotbar and **fails
closed when you don't** — `REWO_PRECMD="give @s minecraft:diamond_sword 1;give
@s minecraft:dirt 64"` against an opped username; 17/18 bare, 18/18 staged.
