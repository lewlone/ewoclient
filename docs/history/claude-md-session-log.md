# The CLAUDE.md session-update footers (2026-05-26 → 2026-08-25)

<!-- Moved verbatim from CLAUDE.md lines 1501–2147, 6937–7192 on 2026-09-26, when CLAUDE.md
     was cut from 7,192 lines to an always-loaded index plus path-scoped
     rules in .claude/rules/. Everything below the rule is the original text,
     unedited. It is history: read it for the reasoning behind a rule. -->

> **Superseded statements in this file** (kept verbatim below; the current truth is in `CLAUDE.md` or `.claude/rules/`):
>
> - Every test count, gate count and render-check number in these footers is a snapshot. Current numbers live only in `REWO_PLAN.md` §0.0.

---

*Last meaningful structural change to this file (2026-05-26 session):
**Post-ban refactor — the legit/pvp split.** An anticheat ban on CatPvP
landed with the macros switched off, pointing at class-name
fingerprinting as the surface. The catalog now ships in two halves:
12 legit modules in the default build (zero packet synthesis), and 14
assist modules behind `--features pvp` / `build.ps1 -Pvp` (the
packet-touching helpers — Auto Tool, Auto Totem, …, Swing Cadence).
Java assist sources live in `dev.lewlone.ewohud.assist.*` and are
filtered out of the legit jar entirely so their class names never enter
the runtime. Deleted outright: `auto_crit`, `mace_combo`,
`wind_charge_mlg`. Renamed + humanized: `triggerbot` →
`swing_cadence` with min-interval-cap + jitter + reaction-delay knobs.
The previous Phase G "legit-client features only" rule is now
explicitly the rule for the **default build**; the assist set is the
opt-in for semi-anarchy use. Build + Rust tests pass in both
configurations; in-game smoke-test of both builds is the open
verification step.*

*Update (2026-05-30 session): two parts. **(1) Reconciliation** — documented
Phase H (Social) + the two undocumented in-game features (custom crosshair,
SMTC media controller) after finding ~8.4k lines of working-but-uncommitted
launcher work plus a fully-built-and-committed bot side; checkpointed it
(commit `61855ed`) and committed the ChickenLink `/launcher-link` plugin
(`14326ba`). The bot is **live + wire-verified** (`/bot/api/links/by-uuid`,
`/launcher/link`, `/friends`, auth middleware all respond to contract).
**(2) H6 built** — Roblox-style join: `--quickPlayMultiplayer` launch
plumbing via a shared `start_launch` helper, `in_game` presence, a 15s
server-status poller, the main-menu network widget, and a friend "Join"
button; the bot's `GET /api/server-status` was made public (needs redeploy).
Both legit and `--features pvp` compile and `ewo-core` tests pass. Open:
visual eyeball of the main-menu widget, the in-game FRIENDS tab, H7
WebSocket, and the live in-game `/launcher-link` → presence → join test.*

*Update (2026-05-31 session): in-game **FRIENDS overlay tab** (read-only,
file-bridge) + a long **launcher polish + window** pass — see the new
"**Launcher window: transparent floating card**" section above (it supersedes
the Step 1/2 app-window chrome notes). Headlines: the window is now a
transparent per-pixel-alpha **floating 22px card that fills the window**
(`CARD_INSET = 0`, no bevel, no outer shadow, `DWMWCP_DONOTROUND`); the
**freeze-on-exit bug is fixed** (`process::exit(0)` after the loop);
**minimize/close buttons** added top-right; **release builds are GUI apps**
with an **embedded app icon** (`assets/icon.ico` via `build.rs`/`winresource`)
and a **portable `package.ps1` → `dist/EwoClient/` bundle** (fonts resolve
next to the exe). Plus broad visual polish (hover glow everywhere, tofu-arrow
→ vector-chevron fixes, dropdown corner fix diagnosed via a new
PNG-render harness `examples/dropdown_shot.rs`, slider/scrollbar/main-menu
tweaks). All committed; verified live except the open Phase H items above.*

*Update (2026-05-31 session, perf): **Memory + performance pass** — see the new
"**Memory + performance pass**" section above. Root-caused + fixed the
focused-idle RSS leak (per-frame `fractal_noise` filter chain in `velvet_folds`
+ an uncached `newsreader_italic` variable-font clone on the main menu — both
foreign C++/FreeType churn, both zero-pixel fixes), gave the backdrop a 20 Hz
"slow clock" offscreen cache for the three full-screen blurs, baked the
pearl-dust halos to a sprite + the inner glow to a cached image, and added an
idle frame throttle (120fps after 0.5s untouched, full rate on interaction).
Verified live: **RSS stable ~108 MB** (was climbing ~2.6 GB/hr), renders
identically. `LEAK_HUNT_INSTRUMENT` diagnostics intentionally left in pending a
final strip.*

*Update (2026-07-22 session, all Rewo — one long session, 13 commits): **M7
online-mode** (login encryption + signed chat, verified on the user's real
account against an enforce-secure-profile server), **M7c real player skins**
(slim + wide, live-fetched into a runtime atlas pool), **metadata mob detail**
(slime/magma size + baby, both at SynchedEntityData index 16, polymorphic by
serializer type), and the whole **M9 native CEM stack** (the EMF/ETF-
equivalent): Rewo now loads an OptiFine resource pack and renders both the
custom mob **models** and their **animations** — verified end-to-end on the
user's real Fresh Animations pack, all body plans walking, no mod loader. The
two load-bearing CEM conventions (top-level translate = pivot; invertAxis:"xy"
= a 180° Z-rotation → negate the animation's X/Y) are documented in the Rewo
section below and REWO_PLAN §15. Everything is in the `rewo-*` crates and the
launcher `Native` arm; none of it touches the `ewo-jni`/mixin/launcher-GUI
machinery.*

*Update (2026-07-23 session, Rewo): **M9d — CEM polish, the Fresh Animations
detail rig.** Closed the M9c "polish left" list (foot-submodel pivots,
per-face `uvNorth`, scale channels), which all shared one root cause: the
parser only made top-level parts bones, so a FA rig's nested detail (head,
eyes, feet) was flattened onto its parent and its animation channels skipped.
Fix = a bone per `.jem` node. Three verified pieces (headless via
`mobshot --pack` vs the real FA creeper/pig/zombie; no-pack gate stays
243/243; 26 rewo-gpu tests): **per-face UVs** (`cube_f_faceuv` — the FA
eyes/snout, previously box-UV garbage), **scale channels** (+ bone-channel
reads + file-order via a new `indexmap` dep — serde_json's sorted `Map` broke
FA's mirror expressions), and **submodels-as-bones** (head-look, eye blink,
foot articulation now animate; box rest positions unchanged). Two more
load-bearing asymmetries surfaced, both empirical-from-FA + vanilla-verified:
a submodel's pivot is its accumulated *position* (`to_model(boxOff)`, e.g.
creeper head2 → the neck), not −translate; and OptiFine translation
**replaces** a bone's translate (subtract a per-bone rest baseline, else the
pig head flings ~12u off the body — baseline is `invertAxis(pivot)` for
top-level, own translate for submodel). Open: ETF random/emissive textures
(M9b). Detail in REWO_PLAN §15 (M9d entry).*


*Update (2026-07-27 session, all Rewo): **M32b, M33 and M33b — the portal pixel
oracle, then weather and clouds.** `portalshot` closed M32's recorded read-back
gap (uniform textures collapse the fifteen layer matrices, so the frame is a
number the CPU can compute; one layer then isolates one sample and makes the
column-major reading observable). Then rain, snow and a cloud deck, wired into
`rewo live`. Three separate corrections came out of it, each caught by something
different: a **gate witness** caught a cloud front-face convention that looked
right from below alone; the **first live frame** caught that vanilla's weather
and cloud geometry is camera-relative while Rewo's `view_proj` already carries
the camera (the gate had rendered at the origin, where the two coincide — it now
renders 2,500 blocks out); and **eyeballing the sky** caught that the rainy sky
greys through `WeatherAttributes`, an environment-attribute layer system, not
through the `applyWeatherDarken` formula M33 had transcribed — which also
corrected two earlier claims of mine, that rain does not darken the lightmap and
that stars merely dim. The rain fog ramp needed a second, *environmental* fog
band in the world pass, pinned by four mutation-verified pixel witnesses in
`lightmapshot`. **561 tests**, fourteen gates green, demo PNG byte-identical to
M15 onward. All of M10–M33b is now **pushed** to
`codex/rewo-m19-combat-swings`; **merged to `origin/main` on 2026-07-27** as a
clean fast-forward, closing the long-standing unmerged-branch risk.*

*Update (2026-08-02 session, docs): **caught this file up from M73 to M86** —
twenty milestones (M54–M60, M74–M86) that shipped without a CLAUDE.md pass, now
one grouped section at the end of the Rewo part rather than twenty essays. The
session opened by verifying rather than reading: build clean, **1623 tests / 0
failures**, `mobshot` 246/246, `HEAD == origin/main == 0ddbc66`, no branch
holding an unmerged commit, and the fourteen dirty agent worktrees confirmed to
be litter (every file already on `main`). **The numbers in the docs were exact;
two of the plans were stale** — `REWO_PLAN.md` §0.0 still said "M0–M57 at
`aadd8e9`" and still offered the health-bar render half and the bundle grid's
chrome as the cheapest pickups, both shipped (M59, M58). §0.0's "Where it is"
and "What to do next" are rewritten, and its second, 2026-07-27 "What to do
next" is now explicitly marked HISTORICAL, because it still recommends worn
armour (shipped M46–M50) and a fresh session reading top-down could act on it.
**The headline state change: `REWO_PACKET_COVERAGE.md` is at 107 / 0 / 34 with
classes A and B both empty** — every packet Rewo can render is rendered, so the
next unit of work is a **subsystem**, not a packet.*

*Update (2026-08-07 session, Rewo): **M105–M107 — closing the recipe book.**
The four items the M104 handoff listed, plus two bugs found on the way. **M105**
the page counter (`gui.recipebook.page` is `%s/%s`, no spaces; only the FIRST
argument is converted to 1-based; the five-argument `graphics.text` delegates
with `dropShadow = true`; a one-page book shows no counter at all). **M106a**
the recipe cell's tooltip, whose extra line is `"Right Click for More"` and
carries **no count** — the handoff called it "the +N more recipes line", which
it is not — and which **loses to the menu's own tooltip even though vanilla
calls it afterwards**, because `setTooltipForNextFrameInternal` is
`if (deferredTooltip == null || replaceExisting)` with `replaceExisting` false
on every path: the FIRST tooltip of a frame wins. **M106b** the menu
displacement in the last two consumers that never learnt about it — the hover
highlight and the item tooltip both converted the cursor against a panel 77 GUI
px from the one they were drawn against, M89's "a per-call-site choice is how
they come to disagree" failing a third time in one file; the fix is one
`book_open` binding that five consumers read, and the highlight's derivation
turned out to have had **no witness of any kind** because every `set_container`
call in the app passes `hovered: None`. **M106c** the ghost's tooltip, the one
place first-wins is observable (a ghost sits ON a menu slot, so a filled slot
asks both producers at once and the real item wins). **M107**
`tryPlaceRecipe`'s guard — both halves load-bearing, since "not twice" breaks
bulk crafting and "not uncraftable" breaks the click that fills the ghost —
plus `useMaxItems`, and the finding that
`FurnaceRecipeBookComponent.isCraftingSlot` switches on the slot's **container**
index and never asks which container, so **three of the player's hotbar slots
are "crafting slots" of a furnace**. Four witnesses were wrong before any code
was; three composition roots (`apply_screen`, the winit handler) still have no
test seam and their mutations are named rather than hidden. `live
--render-check` gained **r25** and therefore a **second caller requirement** —
the staged invocation now needs `recipe give @s *`, verified by meeting a fresh
player after a reused server directory made the first attempt look green.*

*Update (2026-08-07 session, Rewo): **M104 — the which-of-these overlay**, which
finishes the recipe book bar four small items, plus a **doc-staleness pass**.
The milestone's own entry is in the Rewo section below; the docs half is the
part worth flagging here, because it is the second time in six days that the
prose around a machine-checked number was the thing that lied.
`REWO_PACKET_COVERAGE.md`'s §2 table is verified by a unit test in `ids.rs` and
was exact at **114 / 0 / 27**, while its §0 handoff prose still said 107 / 34,
its class table's caption said "the 32 gaps" over rows summing to 27, and its
class-C definition still listed "a recipe book" as a subsystem Rewo lacks —
three milestones after M93y decoded it. All corrected. `REWO_PLAN.md` had two
"What to do next" blocks in flat contradiction (the older one telling the
reader to prefer it) and a section headed "The current numbers" containing only
historical ones; both are now labelled by precedence rather than by date.
CLAUDE.md's own top-level Rewo status block was three milestones behind at M93 /
1860 tests / `containershot` 49-of-49. **The rule this keeps re-teaching: a
number with a test behind it stays true and the sentence next to it does not.**
Repo hygiene: the twenty stale `claude/rewo-m93*` branches and the two leftover
agent worktrees were verified fully merged and clean, then pruned.*
*Update (2026-08-10 session, Rewo): **the audio plan, and the first two items
of it.** [`REWO_AUDIO_PLAN.md`](REWO_AUDIO_PLAN.md) is the decision §0.0 had been
asking for: cpal + symphonia in a new `rewo-audio` crate, a pure caller-driven
`Mixer::render` that never names cpal, four shippable steps M138a–d, then a
loopback oracle and breadth. It records why kira and rodio lose — **kira
interpolates attenuation in DECIBELS where Minecraft's curve is
amplitude-linear**, a 24 dB error at half the radius, and rodio's specified voice
cannot play a stereo source while all the music is stereo — and it carries three
corrections to its own winning design. Its most important section says what the
gate does NOT assert: no gate opens a device, so a client that mixes perfectly
into a stream nobody opened passes every witness, which is why the milestone
requires an owned human listening pass. **M138a shipped in full.** `Entity.DATA_SILENT` is metadata index 4,
parsed and discarded since M1 while `entity_silent` answered a hardcoded
`false` — and the witness that matters is the one asserting the sound world
READS it, since every other test passes against a decode stored where nothing
looks. `build_sounds` now fails closed under `--render-check` rather than turning
a missing `sounds.json` into an empty index, which is indistinguishable from
totally broken resolution and green because it asserts nothing. The battery also
found that **both mutation harnesses proved "nothing left on disk" with `git diff
--quiet`, which cannot tell a leftover mutation from uncommitted work**; they
compare file bytes now. The listener seam then closed the gap the survey called
structural — nothing in Rewo carried a listener, so every sound was panned
against ears at the origin facing -Z. **`listener_basis`'s forward vector turns
out to be exactly `Entity.calculateViewVector`**, reached by a different route,
which is what makes the transcription checkable instead of self-consistent; and
**`ListenerTransform::INITIAL` is not a camera at yaw 0** — `setRotation` opens
with a half turn, so yaw 0 faces +Z while the record's default faces -Z, and a
test asserting they agree looks obviously right and puts the ears backwards.
**r45 asserts `pushes == frames` rather than `> 0`**, because a per-tick client
would still be non-zero; verified by deleting the call site, which drops it to
0 of 5783 while every unit test stays green. **M138b then started the audio crate**
— `rewo-audio`, with no dependencies, holding the two pieces that have an exact
vanilla answer: the `32767.5 / -0.5 / truncate-toward-zero` quantisation (whose
every failure mode is inaudible rather than obvious — a floor puts a DC offset on
every silent sample of every sound) and `SoundBufferLibrary`'s caching, where
**statics are cached permanently, a FAILURE is cached with them**, streams are
never cached, and **the loop flag rides with the stream rather than the channel**,
because `SoundEngine.play` tells a streamed source explicitly not to loop.
Then the decoder: symphonia, ogg and vorbis only. **Rewo cannot grade a Vorbis
decode bit-for-bit** — the format defines the bitstream, not the exact float — so
the witnesses pin measured vectors from three real assets while the audio stays in
the user's install, and two documented claims became measurements: `goat_horn/call3`
is stereo where `call0` is mono, and the store is mixed-rate. **The battery caught a
witness blind to its own subject** — dropping the `-0.5` bias moves a real sound's
PEAK by one and its SUM by 812, so the assertion moved to the sum. The other
survivor was proven genuinely equivalent: symphonia's probe rejects every truncated
ogg before the `channels == 0` guard can be reached, so it is recorded as an
expected survivor rather than left looking untested. **M138c then shipped the
mixer** — caller-driven like `alcRenderSamplesSOFT`, with the module doc splitting
transcription (the OpenAL attenuation curve, exact) from **stated approximation**
(the pan law and the resampler, which live in a DLL Rewo cannot read) and HRTF
absent entirely. Its finding is what the listener's up vector is FOR: `right =
forward x up` is `(-cos yaw, 0, -sin yaw)` at every pitch, so almost every way of
breaking the basis is invisible — except pinning up to `(0,1,0)`, which at pitch 90
makes `forward x up` the **zero vector** and collapses the stereo image to centre.
The battery also caught two weak fixtures: a rate witness whose sources outlasted
its render window measured the window, and witnesses rendering into freshly-zeroed
buffers cannot see a missing clear. **M138d then shipped its testable half** — the SPSC command ring, whose discipline
reads backwards on purpose: a full ring drops the NEWEST command and never blocks,
because a full ring means the callback has stopped, and blocking the render thread
on a dead device trades a silent sound for a frozen client. **The cpal binding is
deliberately NOT shipped** — it is the first thing in Rewo no gate can check, and
it ends with the listening pass a machine cannot perform. Its battery also fired
both harness hazards this file records at once: a mutant that hangs took the
battery down so its `finally` never ran and **left the mutation on disk**, and the
hung test binary then held the link output so the next build failed with linker
error 1104 and looked like a broken tree. Both are fixed in the harness — a
per-run timeout makes a hang a KILL rather than an outage, and a reaper clears
strays. **M138 then completed**: cpal, the mixer's `ChannelCall` interpreter, and
`examples/listen.rs` — the first code in Rewo that can make a noise, and the first
that **no gate checks or can check**, since an absent, muted, exclusive-mode or
unplugged device all look identical from inside the process. Two containments hold:
**cpal is not wired into `rewo-app`**, so the binary and all 34 gates do not link
an audio stack, and **no test opens a device**, so `cargo test` stays silent —
`cargo run -p rewo-audio --example listen` is the only path to a sound, and the
listening pass is the user's. A voice is created SILENT and waits for its `Play`
(vanilla's order is properties, attach, play, and `alSourcePlay` before an attach
is a no-op), and `AttachStaticBuffer` is counted-and-ignored at the callback
because resolving an asset key is a syscall plus a large allocation. **One stated
deviation**: `retire_finished` runs in the callback, so a finished voice's last
`Arc<Pcm>` deallocates there, which can drop out as a sound ends. **M140 then gave
`level_event_sounds` its first production caller** — the 83-id table has been
complete and partition-tested since M66 and nothing called it, so a dispenser, an
anvil and a wither breaking a block were silent regardless of the device. The
position is the block **CENTRE** (`Level.java:475` delegates to `pos.getX() + 0.5`
on all three axes), volume is carried and spans 200x, and a `data` gate outside its
branches is silence rather than a fall-through. A structural fact found while
writing the witness: **all three global rows are camera-placed**, so that path
never fires for a global packet — pinned over the whole table. And one of my own
assertions was a tautology (`assert!(… || true)`) that also contradicted a test two
functions down. **M140b then took `MusicManager`'s gain ramp**, whose finding is
that `calculateVolume`'s third factor `gainBySource` has **exactly one writer in
the entire client** — the music crossfade, not the options sliders, which arrive
one factor earlier. Its two branches have different shapes: fading up **the step
IS the current gain** clamped to `[0.0005, 0.005]` (so a rise from silence
accelerates, and a constant step agrees only above the clamp), while fading down
is an exponential blend; and the floor **stops** the track rather than clamping,
because a silent track would hold one of only two to eight streaming channels. A
surviving mutation was **proven equivalent** — the down branch's second disjunct
cannot fire, since the blend never crosses its target — so it is dead code in
vanilla too, kept and recorded.*

*Update (2026-08-13 session, Rewo): **M149c–g — the End flash, finished.** The
clock map and all three consumers, so **all eleven `TickableSoundInstance` ramp
variants now have a construction site** (`Directional` was the last).
**M149c** the clock map: `ClientClockManager` is a map, its `lastTickGameTime`
is **shared** (a clock minted between ticks gets the whole delta), and
`getTotalTicks` is **`computeIfAbsent`** — the read *creates*, at rate 1.0 — so
`getDefaultClockTime()` has three outcomes that look like one number, of which
two are indistinguishable on any single read and diverge forever after.
**M149d** the lightmap, where the flash is **added** and the comment directly
above it says "multipliers": a multiply is fatal in the one dimension that has
a flash, because `the_end.json` sets `sky_light_factor: 0.0` and the flash is
the only thing lighting its sky. **M149e** the quad — the sun's, with a
different sprite, **no** base rotation, **subtracted** angles and an
`(i,i,i,i)` tint — whose witnesses failed first because the fixture had never
attached a celestial pass (M45 again). **M149f** the sound, where
**`playDelayed` ticks a tickable BEFORE playing it**, so the bearing is taken
against the camera thirty ticks later and capturing it at queue time is both
natural and 1.5 s stale. **M149g** the battery (17: 14 killed, 2 expected
survivors, 1 named) found the login path building its level where no test can
reach — two call sites, one covered — closed by M97's fix and a named survivor
for the composition root itself. 3172 tests, 34 gates, demo PNG
byte-identical.*

*Update (2026-08-13 session, Rewo): **`soundshot` — the audio gate the plan
specified and nobody built.** `REWO_AUDIO_PLAN.md` §4 spells this gate out in
full and M138c's own status block concedes it was never written, so audio —
M138 through M149, ten milestones and the largest subsystem in Rewo — had **no
dedicated serverless gate** and its ~86 `rewo-audio` tests plus the sound tests
in `rewo-net` were crate-local units that could be deleted without anything
objecting. `rewo soundshot --check` is the **35th** gate: 48 witnesses across
the plan's five layers, mutation-verified **45/45**. It corrects three of §4's
claims, all because §4 predates the code — **two locks rather than one** (M143
made `rewo-app`'s audio dependency optional and off by default so a default
build of the one `rewo` binary does not link cpal and symphonia into every
gate, so the default lock is 28 and the audio one 48, with the stated cost that
ordinary verification does not grade decode or the mixer); the counts; and an
end-trim claim that belongs to **symphonia** rather than to Rewo. **Its two
findings are both about witnesses, which is this project's most repeated
lesson.** `s6` failed on its first run and the code was right: with a variant
dropped the total weight is 4, a **power of two**, so `nextInt` takes its
shortcut branch — the **top** bits of one draw — and `LegacyRandomSource`'s
scramble leaves the top bits of the *first* draw of sequential seeds nearly
constant; measured over seeds 0..199 at bound 4 it is **entirely** constant, so
a sequential fixture reaches one variant whatever the weights are and cannot
tell "the variant was dropped" from "the pick is broken". And two mixer
witnesses demanded a zero f32 cannot produce: `f32::cos(FRAC_PI_2)` is
**-4.371139e-8**, so hard LEFT is bit-silent on the far ear (`sin(0)` is exactly
0) while hard RIGHT is only ~155 dB down — **the two extremes of one pan law are
not symmetric in f32** — and the same constant makes the degenerate
listener cross product 4.4e-8 rather than the algebraic zero. Then the battery
found §5's self-skip trap **inverted**: a mutation disabling `build_sounds`'s
fail-closed panic SURVIVED, because on a machine that HAS the asset store the
error arm is never reached, so the fail-closed half of the claim is
unobservable precisely on every machine where the gate is green. `s1b` closes it
by asking for a version with no manifest. **A green `soundshot` is still not
evidence that this client makes any sound**, and §4's paragraph saying so is in
the gate's module doc verbatim — the listening pass remains the user's. 3172
tests, 35 gates, demo PNG byte-identical.*

*Update (2026-08-13 session, Rewo): **M149a/b — the End flash's schedule, and
a fact that was right in one file and wrong in another.** §0.0 offered
`EndFlashState` as the last unconstructed audio ramp; the decompile says it has
**three** consumers and two are visual (the lightmap's `skyFactor += intensity`,
the sky's flash quad, and only then the sound). **Two halves of it were already
in the tree, unread**: `Skybox::has_end_flashes()` decoded and tested since M16
with zero production readers, and `default_clock` spelled in our own bundled
fixtures since M16 while the parser read past it. The schedule is a pure
function of one `long`, so it grades exactly — which matters for a feature whose
audible third no gate can touch. Its findings: **the first 600 ticks of a clock
never flash** (Java's `flashSeed` field default is 0 and the draw is guarded on
a *change*, so an `Option`-shaped "not yet computed" invents a flash vanilla
lacks), tick 0's intensity is genuinely `NaN` and survives only because
`Mth.sin` is a table lookup, `flashStartedThisTick` fires **one tick after** the
offset because `Mth.sin(0)` is exactly 0 against a strict `>`, and the Nether is
the only vanilla dimension declaring **no** `default_clock` — which is a
permanent zero, not a fallback. **`Mth.lerp(1.0, a, b)` is not a select**: at
the flash tail it cancels `1.2e-16` against `0.0101` to exactly 0, so the raw
field and every renderer disagree about whether a flash exists. 22 mutations,
19 killed, 2 proven equivalent; two survived first and both were witness gaps —
`600 -> 601` was invisible because every witness measures *in units of* the
constant (M93r's sweep recurring). **The process finding is about these files:**
§0.0 gotcha 9 has carried the correct line-ending measurement since M126 —
including the broken `grep -c $'$'` detector — while **CLAUDE.md carried the
opposite**, and this session read the nearer one, "confirmed" it with exactly
that detector, and turned a 50-line change into a 3,256-line diff. Measured by
byte count: **378 all-LF, one all-CRLF, four mixed**. The same-fact-in-two-places
hazard landing on the documentation of a hazard. 3157 tests at that point, merged to `main`; the clock
map and all three consumers followed in M149c–g above.*

*Update (2026-08-12 session, Rewo): **M147 + M148 — the Overworld played no
music, and the battery that would have caught it.** M146 shipped music
selection with 3137 tests, 34 gates and 45 render-check witnesses green, and it
**played nothing anywhere in the Overworld**. `situational_music` hard-coded an
EMPTY base below the biome, justified by a measurement in M146's own commit
message — *"grepping every writer of `BACKGROUND_MUSIC` finds only biome
builders"* — whose grep ended in **`head -20`**. There are 32 writers across
four files; `DimensionTypes.java:39` sets `BackgroundMusic.OVERWORLD` on the
overworld type and `:124` sets `Musics.END`, while an ordinary biome declares
none. **The doc error and the bug were the same error**: the claim was not
decoration beside the code, it was the code's justification, and it reached a
commit message, a merge commit, REWO_PLAN §15 and this file before anything
ran. **What found it was running the client** — `REWO_AUDIO=1 python
tools/render_check.py` with sound debug on, showing 159 tick lines every one
`+0 started`. Not an error: a number that should not have been zero. The audio
counters read perfectly healthy throughout. **r46 locks it in and needs no
device**, because music selection runs against whatever device is attached — in
the default build the silent device stands in, and it names
`minecraft:music.creative`, which is `select()`'s creative arm reached through
`instabuild && mayfly`. `dimensioncheck` then demanded the rest, correctly, and
all four of its independent inputs now carry `background_music` — including a
captured wire registry that parses `music.game` + `music.creative` off a real
server. **M148 then ran the battery M145–M147 never had** (26 mutations, 23/26
first time) and found the witness that did not exist: a **budget-refused** music
track had none, and since `startPlaying` parks the delay at `MAX`, a manager
that kept believing it was playing would never start another song for the rest
of the session. Two new harness hazards: a **gate-routed mutation leaves the
mutant's BINARY on disk** (the restore does not rebuild, so the next gate grades
a mutant against a clean tree), and **`git commit -m` in bash runs backticks as
command substitution**. 3141 tests, 34 gates, `live --render-check` 46/46.
**The lesson is not about music: a truncated grep reported as a measurement is
worse than no measurement, because a measurement gets quoted.***

*Update (2026-08-12 session, Rewo): **M145 + M146 — music: which track, when,
and the ownership split the wiring forced.** M145 is the model — `Music`,
`Musics`, `BackgroundMusic`, the `audio/background_music` attribute (the
sibling of M142's ambient one, same compound, same biome-replaces-dimension
rule) and the whole of `MusicManager`. M146 calls it. **The interesting part is
where the state machine had to live**: no one object has both halves of
`MusicManager.tick` — the selection needs the world (biome attribute,
abilities, eyes under water, boss bars, dimension key) and the timers need
`soundManager.isActive(currentMusic)`, which only the engine can answer — so
`PlaySession` names the situation through a new `SoundEvent::Music` and
`SoundSystem` runs the machine. That is the seam M142d built for the biome
loop, **reached independently by the same constraint**. Four transcriptions
that invert: the stop and the clear happen on the **same** tick (vanilla's
`stop()` is synchronous, so the replace path draws TWICE, and deferring it
changes the whole song sequence for a seed); `Mth.nextInt` is inclusive at both
ends and **does not draw** when `min >= max`, which `FREQUENT` reaches in
ordinary play; `getNextSongDelay` checks null **before** the `CONSTANT` case,
inverting `CONSTANT` to 0; and the `maxDelay` cap sits **outside** the
`currentMusic` guard, which is what makes `startPlaying`'s `MAX_VALUE` park
safe. `select()` inverts twice too — **underwater beats creative**, and each
arm falls through to the default only on its own absence. Two arms of
`getSituationalMusic` are absent and neither is a gap: the screen arm needs a
title screen Rewo has not got, and the `player == null` arm is unreachable
because a `PlaySession` exists only after login. **The End test comes before
the boss bar**, which is what stops a wither in the Overworld playing the
dragon's music. Adding the field to `BiomeDef` broke **eight** construction
sites across four crates — intended — and reproduced the runner trap: `cargo
build -p rewo-net` was clean while three other crates failed to compile their
tests. 3137 tests, 34 gates green, `live --render-check` 45/45. **Nobody has
listened, and music is now the most audible thing no gate can grade.***

*Update (2026-08-12 session, Rewo): **M144 — streaming, so music and the Nether
beds play.** M143 declined every streamed attach; this opens them, with an
incremental Ogg reader, a buffer queue on the mixer's voices, and a producer
that keeps it fed on `ChannelAccess.scheduleTick`'s clock. **The measurement
came first and corrected the docs**: 344 of 8,024 variants are streamed and
**six are not music** — the five Nether ambient beds and
`ambient.underwater.loop`, which M142's handlers resolve — so M143 had silenced
those too, and the doc that said "so there is no music" stopped short. It also
closed the tempting shortcut by arithmetic: `music.end` is 806 s, i.e. **142 MB
in one PCM buffer**, so decoding streams fully and reusing the mixer's own loop
flag is unavailable on memory alone. **`LoopingAudioStream` restarts one read
LATE** — its guard is on the inner read coming back *empty*, and a short
non-empty read is the ordinary end of a file — so a looping stream hands out one
short buffer at the loop point and a full one after it, which the witness pins
as the exact sequence `[1000, 728, 1000, 728, 1000]` against a real asset.
**The queue invariant is a BUFFER count and not a duration**: vanilla refills
when one buffer is *fully* played, so its queue oscillates between three and
four seconds, while "top up to four seconds" refills on the first tick after
playback starts — the same slack, different behaviour, caught by a witness
asserting nothing is queued for nineteen ticks. `stopped()` for a stream is
neither of the cases beside it (run out **and** drained), and `state.looping` is
no help because `setLooping(isLooping && !isStreaming)` leaves the channel flag
false for a bed that loops forever. **An underrun is not a death** — the
producer decides when a stream is over, or a music track dies on the first
hitch. **The battery's two important survivors were one mistake**: the
strongest-sounding witness used constant buffers of equal length, and every
position in a DC buffer holds the same number, so a cursor that reset at a join
and a swap that kept the old length both rendered identically. A ramp with
unequal chunks kills both — and revealed that the two renders are *not*
bit-identical, because of the join approximation the code already documented, so
the witness now measures it instead of hiding it. Three survivors are proven
equivalent and kept with the proof. 3113 tests, 34 gates green,
`live --render-check` 45/45, demo PNG byte-identical, battery 25/25. **Music
still needs its selection logic, and nobody has listened yet.***

*Update (2026-08-12 session, Rewo): **M143 — `rewo live --audio`, and the one
method that turns every sound into a click.** M138 built an audio stack and left
it unreachable on purpose; this wires it in, behind a `rewo-app` feature that is
**off by default** so a default build still links neither cpal nor symphonia
(verified with `cargo tree`, not asserted) and the 34 gates are unchanged.
**`LiveSounds.device`'s own doc invited a device milestone to swap the field,
and that is wrong twice over.** The channel pools and the listener record are
`Library`'s bookkeeping — identical behind any device — so `SilentDevice` keeps
them, every witness reading it keeps working (r45 among them), and a backend
implements a three-method `ChannelSink` instead. And `SilentDevice::stopped`
answers `true` unconditionally, which is right for something that makes no noise
and catastrophic for something that does: `schedule_tick` turns a `true`
**straight** into `device.release(channel)` on the next tick —
`MIN_SOURCE_LIFETIME` gates the *instance* reclaim, not the release — and
vanilla's `release` destroys the source, so inheriting it makes **every sound a
50 ms click**, with correct-looking code and a green suite. `stopped()` is
therefore modelled from the buffer's own length on the producer side rather than
asked of the mixer: the truthful alternative is a flag the callback publishes
back, which would move the one method that decides whether the client plays
sounds or clicks into the region **no gate can reach**. Four cases sit before
the arithmetic and each inverts if guessed — a looping source never stops,
acquired-but-unplayed and played-with-nothing-attached are both `AL_INITIAL`,
and a failed attach is the module's one judgement (vanilla leaks that channel
forever, which on a partial asset store means the 26th missing sound exhausts
the pool and the client goes **permanently** silent). **A device-dependent witness was deliberately not
added** — it needs a device, so it can only self-skip on the machine where it
matters, which is the trap `REWO_AUDIO_PLAN` §5 names; what shipped instead is
that claim with the device removed, a test driving a decoded packet through the
real engine, tee, sink, ring and mixer to non-zero samples, with exact silence
asserted first. (*That witness was reserved as `r46` and never built; **M147
later spent the number on a different, deviceless claim** — see the M147 entry.
`REWO_AUDIO_PLAN` §M138d carries the correction.*) **Both mutation survivors were weak fixtures and both were
hidden by the ordinary call sequence**: the attach's `AL_INITIAL` reset is
overwritten a moment later by the `Play` that always follows it, and the
declined-stream witness never asked `stopped()` — so a declined stream held its
channel for the session, and the streaming pool is **five**. Streams are
declined and counted — **and it was not only music**: measured against the real
`sounds.json`, 344 of 8,024 variants are streamed and **six of them are ambient
loops** (the five Nether beds and `ambient.underwater.loop`), which M142's
handlers resolve and M143 dropped. **M144 then shipped the streaming path**, so
all of that plays; what music still needs is its selection logic. A full decode
was never the escape — `music.end` is 806 s, i.e. **142 MB in one PCM buffer**. 3088 tests, 34 gates
green, demo PNG byte-identical. **Nobody has listened yet, and no number above
is that claim.***

*Update (2026-08-10 session, Rewo): **M136 and M137 — two fixes recovered from
worktrees that a handoff called litter.** The claim rested on their branches being
0 commits off `main`, which was true; **`git branch --merged` says nothing about a
dirty working tree**, and two of the five still held uncommitted work. **M136**: a
spectator's tab-list name is `-1862270977` = `0x90FFFFFF`, **white at alpha 144**,
where M52f wrote a grey `0x9099_9999` — and its doc comment restated the same
wrong value, so code and prose agreed with each other and neither agreed with
vanilla. Nothing consumed the constant (the tab list was still model-only until
M151), which is exactly why it survived — the same shape as M135 the same day. **M137**: a
mutation rendering a styled run style-blind survived `deathshot`'s m20, and it was
a weak fixture, not an equivalent mutant — **nothing follows a last span**, so a
styled span placed last has an advance that moves nothing, and bold is charged per
character. The styled span goes first now, and the identical hole turned out to
exist at a different call site in `titleshot`, found by asking where else the shape
could occur rather than by a mutation. Plus a unit test for the one label a pixel
gate can never grade: every label on the pause/disconnect/dialog screens is white,
and white is 1.0 in both colour spaces, so only an INACTIVE button (`0xA0A0A0`)
can show a colour-space error — and none of those screens builds one. 2853 tests,
34 gates, demo PNG byte-identical.*

*Update (2026-08-10 session, Rewo): **M135 — the chat fills were drawn off the
bottom of every screen**, a real shipping bug rather than a feature. `HudFill` is
in GUI pixels and the pass multiplies by the GUI scale; four producers multiplied
by it first, so the chat rows' backdrops, the input bar, the scrollbar and the
suggestion popup have been **absent rather than misplaced** since M109 — no
artefact to report, which is how they lasted eight milestones. **The contract was
documented correctly in two places and it did not help**: the producers were
written against the function beside each of them, and `OwnedTextLine` takes
SCREEN pixels. Two passes, two conventions, one file. **The fix is not a deleted
multiply** — a chat pixel really is `opts.scale` GUI pixels, and dropping both
factors is a third wrong answer that no `scale == 1` fixture can tell from the
right one, which is exactly why every existing fixture was blind (all of them
pass `px = 1.0`). The generalisable finding: an agreement witness already
compared `hud_fills` against `chat_lines` and **passed at every scale with the
bug in place**, because both sides were in the same wrong space — **an agreement
witness has to model whatever sits between the producer and the screen.** One of
the new witnesses was wrong before the code was (the fourth such instance):
vanilla's own chat backdrop is `maxWidth + 12` wide against a screen-independent
320, so on a 320-GUI-px window it really does run past the right edge. Hardening:
`rewo_gpu::hud::gui_scale` already existed, its doc already warned about
recomputing it, and **two of the three sites did not call it**; they do now, and
the two producers whose inputs were already GUI pixels lost their `px` parameter
outright. Also shipped `tools/render_check.py`, so the one gate needing a server
is one command with every recorded trap turned into an assertion. 2851 tests, 34
gates, `--render-check` 44/44 twice, demo PNG byte-identical, 9/9 mutations.*

*Update (2026-08-09 session, Rewo): **the M127–M134 integration** — eight
milestones that had been built in parallel on six branches off the M126 merge,
none of them merged, integrated in one pass. M127 the chat decoration
(`boundChatType.decorate`, so a message renders as its chat type formats it),
M128 clickable chat, M129 the disconnect reason, M130 the linear-colour
correction (**the text pass wants LINEAR and nine of twenty-two callers were
handing it the sRGB byte**) plus the title's and death screen's style flags,
M131 the sound-instance model and a device seam, M132 the scoreboard sidebar,
M133 the recipe book's widget tooltips, M134 the command line's exception
messages. Merge order was chosen so the trunk landed first and M130's
`TextLine::color` → `color_linear` rename hit a settled tree once.
**The integration's own findings are the part worth keeping.** A witness-number
collision — three branches each minting an r42 — that git merges silently,
because `--render-check` ends `pass == rows.len()` with no declared count and
no uniqueness check. Two breaks invisible to a textual merge: `ChatStyle` lost
`Copy` (M128 put an `Arc` on it) and broke five by-value uses written against
the `Copy` version, and the `color_linear` rename E0560'd every literal on the
other branches — both fail loud, which is the good outcome. The dangerous one
was the `usage_box` conflict, where **one side compiled and silently reverted
M134b**. And a real regression no branch's own gate could see: r42's click was
being eaten by the suggestion popup, because M128 branched before M127c added
its decoration witnesses to the same injection block and the clickable row —
the newest message — ended up drawn under the popup. **A branch being green is
not evidence about the merged tree.** 2846 tests, 34 gates,
`--render-check` 44/44, demo PNG byte-identical.*

*Update (2026-08-08 session, Rewo): **M126 — the styled chat pipeline**, which
§0.0 recommended taking before the chat decoration precisely so the decoration
could ship complete. `GuiMessage::content` was a `String`, so the chat store
flattened whatever it was handed; it is a span list now, `StringSplitter` walks
the **part list** vanilla always had (`FlatComponents` / `splitAt` /
`ComponentCollector`, all of which degenerated to a substring while there was
only ever one part), and `TextPass` draws all five `Style` flags. The types had
to move down a crate first — `rewo_world::chat` must name `ChatSpan` and the
dependency runs net → world — which is proved pure by a **conservation** rather
than a reading: world +58, net −58, app unchanged. Findings: **the width
provider takes a style**, because `getBoldOffset()` is 1.0 charged PER
CHARACTER, so a style-blind measure wraps a bold line late rather than merely
drawing it differently; **`position` restarts at 0 for every part**
(`fromList` chains `accept` without renumbering), so an underline's one-pixel
lead-in belongs to each span and a multi-colour underlined line overlaps by a
pixel; and the **deleted marker is GRAY + ITALIC**, drawn plain white until now
because the store could not hold a style. Obfuscation transcribes the
same-width bucket, the unstyled advance and the never-a-space rule, and
diverges deliberately on the source — vanilla's is nanotime-seeded and
reproduces nothing, so Rewo uses a frame-seeded SplitMix64 whose `run_seed`
reads the **unoffset** origin, or the drop shadow would shadow different
characters. **The mutation battery caught a witness lying** (r38 counted
colours across the whole chat box, which a flattening client satisfies; it
counts within one row now) and left one survivor that is neither equivalent nor
a weak fixture: `splitAt`'s `position > contentsSize` read as `>=` genuinely
diverges, and is invisible only because production always pairs it with
`getSplitStyle()` — the test proves agreement under that pairing and divergence
under any other. 2615 tests, 33 gates, `--render-check` 39/39, demo PNG
byte-identical.*

*Update (2026-08-08 session, Rewo): **M125 — translatable components resolve**,
plus a full docs pass. §0.0 offered the chat decoration and said to verify its
blocker first; both halves were reachable, and the survey found what the
decoration sits on and what is far more visible than it — **every `translate`
component Rewo received rendered as its raw KEY with its arguments dropped**, so
a real server's join messages read `multiplayer.player.joined`, every death
message `death.attack.player`, and every command's feedback
`commands.give.success.single`. Both walkers said so in their own doc comments;
nobody had read them. Its finding is not about chat at all: a live trace showed
`/give` rendering as "Gave  [Diamond Sword]" with the count gone, because **NBT
lists are homogeneous**, so a mixed one is written as compounds with every
non-compound element boxed as `{"": value}` and unwrapped on read — and
**Rewo's reader had never unwrapped, from M1 to M125**. It does not fail; it
yields a plausible wrong tree, which is why 124 milestones missed it. Two
witnesses were also wrong before any code was, which is the fifth and sixth
documented instance: r37's premise (that a server announces a joining player to
that player) is disproved by `PlayerList.placeNewPlayer`, and a surviving
mutation turned out to be a weak fixture whose argument overrode the one field
the test observed. The docs pass fixed a rotting file count in §0.0's gotcha 9
(replaced with the stable claim — **five** files under `crates/` are not pure
LF, named), a broken sentence and a stale end-of-line list in
`AGENT_LOOP_BRIEF.md`, and the M-range in `REWO_FEATURE_SURVEY.md`'s staleness
note.*

---

*Update (2026-08-14 session, Rewo): **M150 — the listening pass could reach two
of the six things only a human can grade, and M139 measured the rest.** The
handoff named the listening pass as the one outstanding item; the finding is
that the only code path in Rewo that can make a noise could not reach most of
what the pass is for. `CpalSink::play_once` pushes exactly one configuration
(`cpal_sink.rs:152-164`) — centred, unattenuated, unpitched, relative — which is
**the one configuration in which the pan law, the distance curve, the pitch
resampler and the listener basis are all inert**, so of `REWO_AUDIO_PLAN.md`
§4's six human-only properties it reached two. `examples/listen.rs` is now
thirteen staged stages driving `ring().push(Command::…)` directly, each naming
**what a failure sounds like**, with a preflight that decodes every clip before
playing any of it. **Four of its errors were mine and all had one shape — a
claim taken from the plan without opening the code beside it**: sources placed
against −Z (`Camera.setRotation` opens `rotationYXZ(PI − yRot,…)`, so
`listener_basis(0,0)` faces **+Z** while `ListenerTransform::INITIAL` faces −Z —
both right, 180° apart, and a stage written against the wrong one is audible but
backwards); "straight up" at pitch +90, which is straight **down**;
`mob/chicken/step3.ogg`, which does not exist (the chicken has two step
variants, and the pass aborted correctly but two minutes in); and "Rewo treats
stereo uniformly", when `pan_gains` returns `(1.0,1.0)` for `channels >= 2` and
so **matches** OpenAL. **M139 then shipped and settled the one question that
stage had left open, and the answer was a real divergence**: OpenAL does not
*attenuate* a multi-channel buffer either — its `stereo.d1p0` and `stereo.d8p0`
captures are byte-identical across an eightfold distance change — while Rewo's
`render()` applies `linear_gain` with no channel gate (`mixer.rs:294-298`,
unlike `pan_gains` one function below), so **Rewo fades a stereo source vanilla
holds at full level, −6.02 dB at 8 of 16 blocks, to silence at the radius.**
Recorded, not fixed. M139's other numbers: the distance curve **exact** against
the implementation rather than only against the OpenAL 1.1 spec; hard left/right
exact; and a pan gap that is **structural** — OpenAL puts a front source at
0.5957 and a rear one at 0.4043, summing to exactly 1.0000, where Rewo's pan
input `dot(dir, right)` is **zero for both**, so no curve-fitting separates them
(front +1.49 dB, behind +4.85 dB, overhead +3.01 dB). Its two hardest traps only
appeared on running it: **`26.2.jar` is signed**, so the same-package trick
`Channel.create()` seems to require dies in `ClassLoader.checkCerts` naming a
*vanilla* class, and the first capture's distortion statistic was **reading its
own instrument** (a Hann window floors it near −46 dB, exactly where the
default-resampler rows landed). Also corrected: §4's "Catmull-Rom" (the
resampler is two-point **linear**), §4's "the limiter is set nowhere in Java"
(`Library.java:131` enables it unconditionally; only the *curve* is in the DLL),
a `play.rs` doc claiming nothing drains `sound_events` when it has two call
sites and a device, `REWO_PACKET_COVERAGE.md` using id **129 twice in one
sentence** meaning two different things, and this file's claim that smithing's
`RecipePropertySet`s are not jar-derivable — measured: **30 smithing recipes,
all 30 carrying all three of `base`/`template`/`addition`**, the fifth "class-C
blocker" in this project to turn out not to be one. **Nobody has listened yet,
and no number above is that claim.***

*Update (2026-08-14 session, Rewo): **M151 — the tab list renders, and the
M86 shape a second time.** `crates/rewo-gpu/src/tab_list.rs` was 1209 lines, 41
passing tests and **zero consumers** — pressing Tab in `rewo live` showed
nothing, and had since M52f. This file was already carrying the tell without
drawing the conclusion: M136 corrected the spectator colour and noted that
*"nothing consumed the constant, which is exactly why it survived"*. **Four
inputs were crossing the wire into a discard**: `listed`, `show_hat`, the
`UPDATE_DISPLAY_NAME` component, and `onlineMode`, whose only mention anywhere
in the tree was a **test fixture writing it**. `listed` is a **SET and absence
means excluded** — the only thing that ever adds to `listedPlayers` is
`UPDATE_LISTED` with `true`, which is what lets a plugin keep a vanished player
in `playerInfoMap` (skin and team still resolve) and off the list; a stored
`bool` with a `true` default shows every one of them. `UPDATE_DISPLAY_NAME`
needs an `Option<Option<Nbt>>` because present-with-null **clears**, and it is
the one action in that packet with a variable-length payload, so the discard
was load-bearing where the neighbouring ones were not. `showHat` **defaults to
true**. And **`onlineMode` is a WIDTH input**, not a visibility one:
`extractRenderState:145` reads it as `showHead` and it changes `slotWidth`, so
an offline server's rows are nine pixels narrower and every name moves.
Four more invert in the renderer — **the sort key and the drawn name are
different strings** (the comparator's last key is the PROFILE name while every
width measures the display override, indistinguishable on any server that sets
no overrides); **only the fallback name is team-formatted**, so formatting
unconditionally doubles a prefix on every renamed player; **the spectator
treatment is an ALPHA** (`Font.getTextColor:336` keeps a styled span's RGB and
takes the default's alpha, so a coloured display name is faded rather than
recoloured); and **a row's background is per row** where the sidebar one class
over fills one rect for all its rows. `KeyboardHandler.keyPress` is also
**asymmetric about the key** — the press is gated on no screen being open and
the release is unconditional — so handling both in one place leaves the list
stuck behind an inventory. **Two gaps are stated rather than half-built**: the
8x8 faces (`showHead` is honoured so the geometry is vanilla's, but Rewo has no
GUI-side path that can sample a 64x64 skin at all — the pool is in the ENTITY
atlas and the HUD atlas has no runtime upload) and `RenderType::HEARTS` (the
90 px column is reserved because `widthForScore` moves every name; the hearts
are not drawn). The numeric LIST objective IS drawn, in `PLAYER_LIST_DEFAULT`
**yellow**. **The battery's four gaps were three-and-one**: "UPDATE_LISTED only
ever ADDS", "player_info_remove leaves the departed player listed" and "showHat
defaults to FALSE" all survived because the only tests of that arithmetic were
COPIES of it — M45's `install_shapes` shape twice in one milestone, over state
living in `PlaySession`, which has no test module anywhere in the repo (M71);
the fix is M97's, a free `TabListPlayers` both former copies now call. The
fourth was a gate fixture that could not express its claim. **And
`tools/render_check.py` did not build** — it checked only that the binary
existed, so the first attempt to prove r47 non-vacuous compiled nothing and
read the unmutated number. It builds now, and r47 is verified three ways: the
key ignored gives 7277 of 7277 frames, `listed` ignored gives 5 rows where 3 is
correct, and no resolver at all gives 0. 3200 tests, 35 gates, `--render-check`
47/47, demo PNG byte-identical.*

*Update (2026-08-23 session, launcher-side bug pass — read-only orientation first, then fixes): the
**`EwoModuleData` schema mismatch is closed, and it was hiding a second bug.** The 2026-05-26 post-ban
bump to `SCHEMA_VERSION = 3` was applied Java-side only (`EwoModuleData.java`) while the Rust writer
(`crates/ewo-jni/src/modules.rs`) kept writing **2** — so `ready()` sat permanently false for three
months. Nothing failed because no read gates on `ready()` (only `EwoModules`' startup announcement);
the drift guard was inert, which is exactly what a drift guard exists to prevent. The buffer geometry
was verified field-by-field and matches (header `i32 schema + i32 count`, records at offset 8,
stride 40, dynamic count at offset 4) — only the version integer lagged; Rust now writes 3 and a new
test pins the pairing so a one-sided bump fails loud. **The sharper find: that module's layout test
had been corrupting memory since `MAX_SETTINGS` grew** — `buffer_layout_matches_the_schema` wrote
12 records × 40 B + header = 488 bytes into a `[0u8; 256]` stack array through raw pointers and then
sliced reads up to offset 452; running `cargo test -p ewo-jni --lib` killed the process abnormally
(`0xe06d7363`, not even a clean panic). It went unnoticed because verification counts only ever ran
the eight `rewo-*` crates — **the `ewo-*` crates' tests are in nobody's loop**, which is worth
remembering whenever an `ewo-*` change lands. Fixed with a CAPACITY-sized fixture plus an explicit
`needed <= CAPACITY` assert. Both fixes deployed via `ingame-mod/build.ps1` (jar redeployed past the
file:// cache; fresh `target/debug/ewo_jni.dll`, which is what the mod loads first).

Also this session: four verified-stale comments corrected (`skin.rs` claimed wide-only though slim
ships; `window/mod.rs` + `win32.rs` headers still described the dead `DWMWCP_ROUND` approach;
`ewo-jni/src/lib.rs`'s JNI contract listed 7 of 13 exports; `rewo-audio/src/lib.rs` still said
"deliberately has no dependencies yet" and "M138b scope" — rewritten for the shipped crate).
An orientation sweep also confirmed several shipped-but-undocumented things, recorded here so the
next session does not re-derive them: the **PvP-Utils layer** (intentional, separate from legit/pvp:
launcher `SettingsTab::PvpUtils`, `ewo_core::pvp.rs` + per-profile `pvp.toml`, Java
`dev.lewlone.ewohud.pvp.*` ships in BOTH builds and `EwoHitRange` is called from the *legit*
PlayerAttackMixin — do not "clean up" that call); the ingame-mod **26.2 support layer**
(`EwoMixinPlugin` picks 26.1-vs-26.2 mixin variants via `-Dewo.mc.version`; build compiles against
the 26.2 jar — Phase E prose describing the 26.1 toolchain is historical); `bundled.rs` catalog is
now 17 toggleable (Iris Shaders and Force Crawl joined after the "16" was written); `ewo-render`
gained `widgets/liquid_glass.rs` (SkSL SDF refraction glass used by the HUD's `.iw-shell`);
`ewo-jni/src/audio.rs` is a WASAPI process-loopback spectrum visualiser feeding the media widget;
and `dist/EwoClient` had gone partial (exe only — no fonts/icon/rewo.exe, breaking its own
self-contained contract) until `package.ps1` was re-run this session.*

*Update (2026-08-23 session, M174 landed — the sign editor): the m174 worktree's
mid-flight WIP (it did not compile; the app-side pump/close, render arm, key
routing, gate and battery were all absent) was finished and merged. The editor
is complete: `open_sign_editor`/`sign_update` decode,
`rewo_world::sign_edit_screen` (`TextFieldHelper` over the current line with a
whole-candidate PIXEL-width validator that rejects rather than truncates), flat
GUI-blit boards (`gui/signs` + `gui/hanging_signs`, wall = top 12 rows), the
per-keystroke local echo through `set_sign_messages`, and the load-bearing
vanilla rule that **every exit commits** — `removed()` sends `sign_update` on
Done, Esc, the validity tick AND on being replaced by another screen
(`Minecraft.setScreen` calls it), which r64 drives live. Gate:
**`rewo signshot --check`** (the 42nd, 23 witnesses — five of which were wrong
before the code was, down to a p5 band derived from a zero-width origin);
battery **15/15 + control + one proven-equivalent survivor** (the paste arm's
own selection collapse is dead code — `insert_text` collapses both paths).
`--render-check` **64/64**; coverage **122 / 0 / 19**, class C **8**. Full
detail in REWO_PLAN §15 (M174).*
*Update (2026-08-24 session, M175 — the baby sheets): M165's pinned gap ("jar
has 147 *baby*.png, Rewo bakes 0") is half closed, exactly. A new generator
(`tools/gen_baby_textures.py`) extracts vanilla's `isBaby` whole-sheet swaps
from the decompile into generated `baby_texture_table.rs` (21 swaps); the 10
whose baby sheet tiles like its adult are baked and applied as a per-slot UV
offset gated on `MobCombat::is_baby`; the other 11 need vanilla's separate
BABY model layer and stay unbaked-by-arithmetic (`baby_swap_skips`, named in
the gate). Load-bearing: PiglinRenderer's NESTED ternary means piglin_brute
must not inherit piglin's baby sheet (both are named exclusions);
kind_for_entity_name needs NAMESPACED names (bare names → Capsule → inert
swaps — THE bug the battery's namespace mutation now guards); a gate witness
that outlives its stage's destroy() dies as 0xC0000005, not a panic.
mobtexshot 13 -> **17** witnesses (m8 rewritten per contract + n1-n4);
battery **6/6 + control**; MAX_AMBIGUOUS 5 -> 6 (dolphin/dolphin_baby share a
palette). Two net tests pin slot-16 fallthrough -> set_baby. 42 gates,
**3450 tests**, render-check 64/64, coverage unchanged at 122/0/19.*
*Update (2026-08-24 session, M176 — the leash's light): M170's recorded
divergence is closed — the ribbon now interpolates PACKED `(block, sky)`
components per vertex and evaluates the lightmap curve (the vanilla
`LeashFeatureRenderer.java:60-77` shape), instead of blending the two ends'
final RGB. `build_ribbon` takes a lightmap-eval closure so rewo-gpu stays
world-agnostic. Two test-side traps the old signature had been hiding: a flat
test lightmap lets the 0.7 dim alternation dominate the fade witness, and
index slices into the strip re-cover the start because the second pass runs
backward — position windows at each end are the robust read. leashshot 5/5,
gpu 320/320, render-check 64/64.*

*Update (2026-08-24 session, M177 + M178 — the advancements screen, decode to
pixels): the last dark GUI subsystem landed in two milestones. **M177**
decoded `select_advancements_tab` (85) + `update_advancements` (130) and built
`ClientAdvancements`' tree/progress state plus the pure screen model
(`rewo_world::advancements_screen`) — coverage **122/0/19 → 124/0/17**, class
C **8 → 6**. Load-bearing: tree insertion runs parent-before-child PASSES
(one packet may list a child before its root); `announce_to_chat` never
crosses the wire; **the battery's survivor** was a second copy of the
AND-over-groups done rule with no caller and no witness — flipping it to ANY
survived a full green suite, so one body now lives on
`ClientAdvancements::is_done`, which also carries vanilla's
empty-requirements-are-false rule. **M178** rendered it: the screen pass grew
`ScreenDraw.scissored` batches (contents clip under chrome, vanilla's
extractInside→extractWindow order) and the atlas grew 512→1024 tall under
M172's append-only rule; `window.png` turned out paletted with per-index tRNS
alphas (transparent interior — the first gate probes sat on transparent
texels and "passed" against whatever was behind them); L opens/closes
(vanilla's own default), opening sends `openedTab`, every close sends
`CLOSED_SCREEN`. Gates: **`advshot` is the 43rd** (14 witnesses through the
production builders). Process lessons: a mutation battery that doesn't rebuild
between mutants grades a stale binary and reads all-SURVIVED (the control
caught it); presence-only pixel counts can't see pass ORDER (zero
black-on-core is what does). Battery 7 killed + control. All 36 counted gates
+ 5 prose gates exit 0, render-check 64/64, **3489 tests**, demo PNG
byte-identical. Open from the arc: M179 tab clicks (+ live r-witnesses),
mid-screen tree updates, item-icon scissor clipping.*

*Update (2026-08-25 session, M179 — the advancement clicks, headless): the
premise check fixed the model half before any wiring landed — M178's
`tab_click` refused clicks at ≤1 tabs, but that is the DRAW rule misread as
the click rule: `AdvancementsScreen.mouseClicked` (java:113-127) iterates tabs
unconditionally; only `extractWindow` (:206) and the tab tooltips (:228) gate
on size > 1. And with one tab a click still RE-SENDS `opened_tab`, because
`setSelectedTab` sends BEFORE its change check (`ClientAdvancements.java:77-86`)
— re-clicking the open tab re-tells the server. Wired: tab click → select +
`openedTab` through a pure `tab_click_report` (the gate drives exactly what
production runs); wheel → both axes ×16 (`SCROLL_SPEED`), consumed iff a tab
is selected; drag → RAW GUI-scaled deltas with vanilla's dead-first-event
latch and the non-left cancel; any release clears. The drag machine is a pure
`AdvDrag` because `LiveApp` has no test seam, so the unit tests are the only
instrument that reaches it — the battery's checker runs both instruments.
The battery's first run produced two real survivors: m9 hand-rolled
`screen.select` beside production's handler so deleting production's select
survived everything (M93b's shape, in the newest form — the copy was written
by the same milestone as the gate); and my own `press()` cleared the latch,
which `mouseClicked` never does, making the cancel arm unreachable and its
test vacuous until the test was rewritten to press RIGHT mid-drag. advshot 14
→ **20** witnesses; battery **8 killed + control SURVIVED**; live r65
witnesses for the sends are claimed in the allocation table and DEFERRED to a
servered session. Measured: **3491 tests / 0 failures** across eight crates
(app 231 → 236; the old headline's splits summed 3486 against its own 3489 —
unaccountable +3, today they agree), all 43 gates green, render-check 64/64,
demo PNG byte-identical.*

*Update (2026-08-25 session, M180 — the written-book page-text clicks): the
M172 leftover, headless-only, premise verified first and it HELD this time:
`BookViewScreen.mouseClicked` (java:215-226) walks a ClickableStyleFinder
over the PAGE-TEXT lines only; `handleClickEvent` (:228-247) treats ChangePage
as ONE-based (`forcePage(page - 1)`); RunCommand closes first (a no-op in the
plain reader — only LecternScreen overrides) then sends unsigned; other events
are declined-but-consumed (M85's no-URL rule). The rect test is half-open
LEFT-INCLUSIVE. Structure: one layout walk (`layout_spans`) feeds both
`book_text_lines` and `click_event_at`, with m6 pinning the agreement.
Findings: my half-open witness was asserted backwards and failed against
correct code; change_page reads field "page" not "value", and 26.x component
events are snake_case on the wire ("click_event"); a styled component
inherits its event across its whole wrapped text so plain-span controls need
SIBLING components. The battery's one survivor was PROVEN EQUIVALENT (with
disjoint rects plus and_then, an event gate is dead weight — vanilla needs
its check only because its scanner overwrites last-wins), so the clause was
deleted and the equivalence kept as a named survivor. **Process find worth
its fix: after a battery, the exe IS THE LAST MUTANT** — restore fixes
sources, not binaries; the post-battery gate sweep graded the drift mutant
and only bookshot's m6 went red. All three battery harnesses now rebuild
after the final restore. bookshot 21 → **24** witnesses; battery 7 killed +
control + 1 named equivalent; **3495 tests** / 0 failures (world 1259); 43
gates green on a tree-matching binary; render-check 64/64; demo PNG
byte-identical.*
