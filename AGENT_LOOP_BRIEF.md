# Agent-loop brief — Rewo

The process/working-agreement layer for the headless two-agent loop.
**`REWO_PLAN.md` §0.0 HANDOFF remains the technical entry point**; this file
covers how we work, not what the code does.

---

## Roles

- **Sol (GPT-5.6) — senior dev.** Directs, reviews, decides scope and
  priority, challenges claims. Has full repo access.
- **Claude — implementer.** Writes code, runs the gates, reports measured
  results, keeps the docs and memory current.

Both operate headlessly. The user is away; nothing may depend on them
manually launching, clicking, or looking at anything.

---

## What Rewo is

A from-scratch native Minecraft client in Rust: vanilla protocol (pin **26.2 /
protocol 776**), raw Vulkan via `ash`, single-threaded tick + rayon mesh pool.
It lives in the `crates/rewo-*` half of the EwoClientV3 workspace and produces
one binary, `rewo.exe`. The `ewo-*` crates are a **different, unrelated
project** (a Skia launcher + a JNI in-game HUD) — Rewo work must not touch
them.

Performance is defined as **1%/0.1% frame-time lows and input latency**, not
average fps.

---

## The two non-negotiables

**1. Ground truth is the decompiled jar, never a wiki.**
`%APPDATA%/EwoClient/rewo/26.2/decompiled/` (Vineflower output) plus the
datagen reports under `.../26.2/datagen/generated/reports/`. Community
documentation is frequently wrong or version-stale. 26.x moved a great deal
(`LightTexture` → `Lightmap`, `getSkyDarken` → a keyframed timeline,
`ClientboundSetTimePacket` → a clock map), and each time the wiki would have
lied. If a fact is not in the decompile or a report, say so explicitly rather
than filling the gap.

**2. Verification is headless-first, and must check the property, not a
proxy.** The user has stated they will not manually test what a machine can
check. Every milestone ships a self-check path. "It looks right" is not a
result — a previous mob pass shipped textures that were silhouette-correct and
UV-scrambled, and was reported as verified. That is the failure mode to design
against.

---

## The gates

**This section used to list every gate with its counts, and that is exactly why
it rotted**: it duplicated `REWO_PLAN.md` §0.0, and by 2026-08-07 it was
claiming 435 tests against a real 2161 and `mobshot` 243/243 against 246/246.
A number kept in two places goes stale in one of them. So the list lives in one
place now.

**`REWO_PLAN.md` §0.0 carries the current gate list, the current counts, and
how to run the live checks.** What belongs here is the rule, which does not
change:

* Run the gates relevant to what you touched before claiming anything, and all
  of them before declaring a milestone done.
* `cargo test --workspace` also pulls in the unrelated `ewo-*` crates — run the
  `rewo-*` ones individually. **`cargo test -p A -p B` drops a result line in
  the combined stream**, so run each crate and sum, or you will write a wrong
  total in a doc. **And the per-crate flag is not uniform**: `rewo-app` is a
  binary crate, so it takes `--bins` where the other six take `--lib`; `--lib`
  there is `error: no library targets`, exit 101, and no `test result` line —
  which reads exactly like a crate whose tests failed to compile.
* Every gate is serverless and fail-closed, and most require Vulkan validation
  (they fail rather than silently skip when it is unavailable). A gate that
  cannot reach a call site does not test it; a gate that supplies an input
  production derives is testing itself.
* **Read a gate's EXIT CODE, never a substring of its output.** Different gates
  print different summary lines, and several are fail-closed on a *declared*
  witness count — so adding a witness without bumping the count turns the gate
  red while every individual line still says `ok`. 2026-08-07's M109 lost two
  whole mutation batteries to exactly that: a battery run against an
  already-failing command reads KILLED for every entry. **Put a no-op control
  in every battery** — a mutation that must SURVIVE — or you cannot tell a kill
  from a broken instrument. **And put a baseline run at the top of the battery
  itself**: M125's harness was wrong twice (see below) and both times that
  guard reported it, where a wrong verdict would have "confirmed" three
  mutations against a command that never executed.
* **A mutation harness shells through cmd.exe on this machine**, if it uses
  Python's `shell=True`. `VAR=value cmd` is not cmd syntax — it looks for a
  program literally named `REWO_PRECMD="give` — and cmd cannot run
  `./target/...` at all ("'.' is not recognized"). Set the variable through
  `env=` and build the path with `os.path.join`; writing a Windows path as a
  string literal puts a carriage return in it, because backslash-r is an escape
  in most languages. `tools/m125_mutate.py` is a working example.
* **The same rule one level up:** `cargo build` passing says nothing about
  whether the *tests* compile. Read each crate's exit code from `cargo test`;
  a crate whose test module fails to build prints no `test result` line at all,
  contributes 0, and reads as silence. M110 hit that and the only signal was
  the total falling.
* **`live --render-check` is the only check that drives the *windowed* client**
  and therefore the only one that can see a render path the windowed client
  never reaches. Run it after any milestone that adds one. It needs a debug
  build and has caller requirements — §0.0 states them.


## Test server

Do **not** reuse a shared directory when the world's shape or the player's state
matters — a concurrent session once left the shared one with a size-12 world
border, and unlocked recipes persist into the save (which made a fresh-player
witness look green in 2026-08-07's M105b before it was caught). Make your own
directory on a free port, and remove it when done.

```
copy testserver/{server.jar, eula.txt, server.properties}   # eula.txt BYTE-FOR-BYTE
                                                            # (a PowerShell-written
                                                            # one gets a UTF-8 BOM
                                                            # the server rejects)
rewrite server-port
write ops.json with the offline UUID (name-based MD5 of "OfflinePlayer:<name>",
      version/variant nibbles set)
```

**PROBE the port you are about to use — do not invent one.** 2026-08-07's M111
lost a run to this: a port picked by incrementing an earlier probe was already
held by an unrelated app, the server died on `FAILED TO BIND`, and the client
ran anyway. **27 of 28 render-check witnesses still passed**, because most of
them are injected — the container, the recipe book, the chat all drive
themselves — and only r25, which needs a real server to grant recipes, could
tell. A gate whose witnesses are mostly self-driven can look healthy against
nothing. Probe with a `TcpListener` bind, and grep the server log for
`FAILED TO BIND` before trusting a run.

**Start it detached, not backgrounded.** The server stops on stdin EOF, so a
`nohup java … &` from a shell dies immediately; use
`Start-Process -WindowStyle Hidden -PassThru` and keep the PID. Stop by that
PID — `Stop-Process -like '*testserver*'` will not match, because the java
command line is just `java -jar server.jar` with no cwd in it.

**The op name in `ops.json` must be the name you connect with**, and its UUID
must be that name's offline one. The *shared* `testserver` ops only `RewoOp`, so
connecting to it as `RewoBot` or `RewoLive` silently has every setup command
rejected — which looks exactly like a code bug. In your own directory, op
whatever name you pass to `--username`; 2026-08-07's M108–M113 runs all used
`RewoBot` and worked because `ops.json` named it.

`REWO_PRECMD` runs `/`-commands as that player on join and `REWO_SETTLE=<n>`
holds the session before the shot; together they make a scene reproducible in
one frame, which matters because **two live runs are not the same scene**.


## Traps that have already faked results

Each of these produced a confident, wrong conclusion at least once. They are
listed because they are invisible in the output.

- **Setup commands must be paced.** `--setup` accepts `;`-separated commands
  and sends one per 250 ms. Firing them in one tick trips the server's chat
  rate limit and the tail is dropped — the structure never appears, and the
  symptom reads as a lighting or meshing bug.
- **A structure built right after a `tp` is already present when the chunks
  stream in.** That masked `section_blocks_update` being entirely unhandled
  for a long time. To prove chunks are loaded first, pad with paced no-ops
  (`say w1;say w2;…`) before the real commands.
- **`--light-check` can grade the engine against itself.** It diffs a
  recomputation against the *stored* light, and incremental relighting writes
  that store. Pass `--no-relight`, or build in one run and grade from a fresh
  join in a second. A vanilla server sends **no light packets for ordinary
  block edits at all** — that is why the client engine exists.
- **Stale binaries.** A run that contradicts the previous one for no reason is
  usually a build that had not finished replacing the exe. Rebuild and repeat
  before theorising.
- **Farmland reverts to dirt** when unhydrated, and the server leaves stale
  block-light inside the now-opaque cell. Use `dirt_path` as the stable
  stand-in in lighting tests.
- **Absolute coordinates beat `~` after a `tp`.** The `~` resolves against
  wherever the entity is when the command executes, which may not be where you
  think.
- **A fixture graded against its own transcription proves nothing.** M16's
  `dimensioncheck` test was named
  `the_bundled_transcription_matches_the_decompiled_json` while only comparing
  the bundled built-ins to a handwritten table — the decompiled JSON was never
  opened. Senior review caught it. If a check's *name* claims a file is the
  oracle, the check must read that file, and the reading must not go through
  the code under test.
- **A block placed beside the player can land inside the player.** 26.2
  `BlockItem.canPlace` rejects a placement whose cell overlaps any entity
  (`isUnobstructed(state, clickedPos, placementContext(player))`). The bot's
  0.6-wide body reaches to `fx+1.3`, so placing at the adjacent cell `(fx+1, fy)`
  failed intermittently — about one run in four, whenever the resting sub-block x
  was ≥ 0.7 — and looked like a protocol bug when the packets were byte-perfect.
  Place two cells out (`fx+2`), which the footprint can never reach. The server
  still sends a `block_update` for the target on *rejection* too (air), so the
  client's observation is authoritative either way — grade the exact block, not
  "non-air", and let the gate exit nonzero.
- **Fixed time and the day timeline are different fields.** `has_fixed_time`
  and the `timelines` holder set are independent members of `DimensionType`;
  deriving one from the other happens to give the right answer for all four
  vanilla dimensions and is still wrong. The tag files decide the timeline.
- **Metadata for an untracked entity is dropped whole.** 26.2
  `ClientPacketListener.handleSetEntityData` does `Entity e = getEntity(id); if
  (e != null) e.getEntityData().assignValues(…)` — an untracked id mutates **no**
  state (not a baby fallback, not anything). M18's first cut applied a baby
  fallback for a missing id; senior review caught it against the decompile.
  `apply_set_entity_data` must return before applying when `entities.get(id)` is
  `None`.
- **A duplicated constant is a latent bug waiting for the constant to change.**
  The entity upload path hard-coded `total * 36` beside a `VERTEX_STRIDE` of 36.
  M21 grew the stride to 52 and only 36 of every 52 bytes reached the GPU, so
  `mobshot` fell to 223/243 with garbled far-side faces. The build was clean and
  the failure looked geometric. `grep` for the numeric value of a constant before
  changing it.
- **`rewo play`'s build gate no longer assumes flat ground** (M20.1): it scans
  east from `fx+2` for the first air-over-solid column and fails closed if there
  is none within eight blocks. The old assumption broke whenever an earlier run
  of the same gate had dug a hole the bot then walked into.
- **`ItemTransform.Deserializer` scales translation by 0.0625 before `apply`
  ever runs**, then clamps translation to ±5 and scale to ±4. Storing the raw
  JSON numbers puts every held item 16× too far from the hand, which reads
  exactly like a transform-order bug. The same shape of trap as the CEM
  translate conventions: the file is not the value the renderer wants.
- **A pose baked as a static fold cannot animate, and is probably also wrong.**
  Rewo froze the undead arms-forward pose at `Fold::rot(-π/2)` — vanilla rests
  at `−π/2.25` (−80°) and deepens to `−π/1.5` when aggressive. A baked pose that
  looks right at rest hides both a constant error and the absence of the whole
  rig. When a model part is "posed" by geometry rather than an `Anim`, check
  whether vanilla animates it.
- **An enum having a case is not proof the code path produces it.**
  `HumanoidModel.ArmPose` declares `ITEM`, `BLOCK`, `BOW_AND_ARROW` and more, but
  `HumanoidMobRenderer.getArmPose` returns only `SPEAR`/`EMPTY` — it is
  `AvatarRenderer` that falls through to `ITEM` for any ordinary held item. A
  review that read the enum instead of the renderer that produces it nearly
  dismissed M19's biggest gap; a review that read only the *mob* renderer would
  have concluded "everything is EMPTY" and been wrong for players.
- **Editing a CRLF or mixed-EOL file normalizes line endings across the touched
  region.** `crates/rewo-gpu/src/entities.rs` is mixed in HEAD and
  `crates/rewo-data/src/lib.rs` is uniformly CRLF; ordinary edits inflated the
  former's diff from 341/28 to 529/216, all invisible churn. Fix:
  `git diff -- <file> | tr -d '
' > p; git checkout HEAD -- <file>;
  git apply --ignore-whitespace p`, then confirm `git diff --numstat` agrees with
  `git diff --ignore-all-space --numstat`. Note `git diff --check` **cannot** be
  green for a uniformly-CRLF file whose added lines are CRLF — git flags the
  `
` unless `core.whitespace` includes `cr-at-eol`, which is unset. The
  LF-insertion is what makes it green, and it is the project convention.
- **A metadata slot can be polymorphic by entity kind.** `DATA_DANCING` (Allay)
  and `DATA_BABY_ID` (`AgeableMob`/`Zombie`) both sit at SynchedEntityData index
  16 with the BOOLEAN serializer (id 8) — the byte parser cannot separate them.
  Only the entity *type* (resolved from `registries.json`) can, at the routing
  layer. The parser must surface the raw bit and let the kind-aware caller route
  it, not prejudge it.

When a render looks wrong, the fastest diagnostic is to **force one term to a
constant** (e.g. `lm = vec3(1.0)`) and see which term owns the pixel. That
found the ground-plane lighting bug in minutes after speculation had failed.

---

## Current state

**Deliberately not restated here.** `REWO_PLAN.md` §0.0 is the single place
that carries what has shipped, the measured counts, and what to do next — and
§0.0 itself warns that its *prose* goes stale faster than its numbers, so read
`git log --oneline` before trusting any forward-looking paragraph anywhere,
including this file.

This section previously carried an M22-era snapshot that survived roughly
eighty milestones, still describing the M10–M18 arc as "reviewed local work,
not yet pushed". Everything is merged and pushed; there is no work branch.


## Conventions

- **Commit messages explain the why**, not just the what: what was wrong, how
  it was found, what was measured. Several of this project's commits are the
  only record of a subtle vanilla behaviour. End with
  `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- **After a milestone**, update `REWO_PLAN.md` §15 (the status log — it is the
  durable record), the Rewo section of `CLAUDE.md`, and the `rewo_client`
  memory file. Load-bearing conventions belong there so they are never
  re-derived.
- **Generated code is generated**: `tools/gen_block_light.py`,
  `tools/gen_vanilla_hierarchy.py`, `tools/gen_anim_defs.ps1`. Re-run after a
  version bump; never hand-edit their output. They are written to fail loud
  on an unrecognised form rather than silently defaulting.
- **Never run `cargo fmt`.** The Rewo code is hand-formatted; `cargo fmt`
  rewrites the whole workspace (~21k lines, incl. the generated `anim_defs.rs`).
  Keep the semantic diff narrow with targeted edits.
- **Line endings.** The tree is overwhelmingly LF and **exactly five files
  under `crates/` are not** — one pure CRLF (`rewo-gpu/src/cem.rs`) and four
  mixed (`rewo-app/src/mobshot_cmd.rs`, `rewo-gpu/src/vanilla_hier.rs`,
  `rewo-world/src/chunk.rs`, `rewo-world/src/light.rs`). Measured as **bytes**,
  most recently 2026-08-08 after M125; `REWO_PLAN.md` §0.0 gotcha 9 carries the
  full version, including why the obvious `grep -c $'\r$'` detector answers
  "every line" for *any* file and must not be used. Two hazards, in opposite
  directions: an editor may normalise one of those five to CRLF, which trips
  plain `git diff --check` on your added lines — the fix that keeps it green
  *and* preserves every unchanged byte is to insert LF-only lines
  (`git diff | tr -d '\r' > p; git checkout HEAD <file>;
  git apply --ignore-whitespace p`) — and the Write/Edit tools may emit CRLF
  into a *new* file in an all-LF crate, so **check a new file's bytes before
  committing it**. M114's `suggestions.rs` arrived CRLF and had to be
  converted; M125's `chat_translate.rs` arrived LF, which is why it was
  checked rather than assumed.
- **Leave the tree clean and the server stopped** at the end of a work block.
- **Report failures as failures.** If a gate regresses, say so with the
  numbers. An honest red result is worth more than a green one that was
  obtained by weakening the test.
