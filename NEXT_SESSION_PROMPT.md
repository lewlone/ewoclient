# Handoff prompt for the next Rewo session

Copy everything below the line into a fresh session.

---

You are continuing **Rewo** — the from-scratch native Minecraft Java client in
Rust that lives in `crates/rewo-*` of the EwoClientV3 workspace. The `ewo-*`
crates are an unrelated Skia launcher + JNI HUD project; do not touch them.

**Read `REWO_PLAN.md` §0.0 HANDOFF first.** It carries the current
measurements, the gate list, the live-check recipe, the load-bearing gotchas
and the known-gaps list. `AGENT_LOOP_BRIEF.md` carries the process rules and
deliberately *points* at §0.0 rather than restating it — that form is load
bearing, don't "helpfully" duplicate numbers back into it.

## State

**Everything is on `main` and pushed. Nothing else exists.**

```
main == origin/main == 4013111
```

No other local branch, no other remote branch, no worktrees. Verified on the
merged tree, not on the branches that fed it:

- **3273 tests / 0 failures** across all **eight** rewo crates
  (world 1198, net 1162, gpu 290, data 228, app 222, mesh 45, proto 16,
  audio 112). Read them per crate off the runner: `rewo-app` needs `--bins`,
  the other seven `--lib`, and a crate whose tests fail to *compile* prints no
  `test result` line at all and reads as silence.
- **36 serverless gates green**, enumerated from `rewo --help` rather than
  from a list.
- Demo PNG `2cc56b4acbfb92cb` — byte-identical since M15.
- `REWO_PACKET_COVERAGE.md` at **119 / 0 / 22**, classes A and B empty.
  Its §2 table is machine-checked by a unit test in `ids.rs`.

The last session shipped **M152–M157** and closed every item §0.0 was
carrying. Read `REWO_PLAN.md` §15's "M155–M157" and "M154" entries for the
findings; the short version is in the merge commits.

## What is actually left

### 1. The listening pass — still the user's, still the only thing no machine can do

This has been the top item for ten milestones and it has not moved, because a
machine cannot do it. **No gate in this project opens an audio device** — an
absent, muted, exclusive-mode or unplugged one all look identical from inside
the process — so everything checkable passes and *that is not the same claim*.

```bash
cargo run -p rewo-audio --example listen            # 13 staged stages, ~3 min
cargo run -p rewo-audio --example listen -- --list  # what each stage grades
cargo build -p rewo-app --features audio
rewo live --audio                                   # or REWO_AUDIO=1
```

Two things have changed *underneath* it since anyone last considered listening,
and neither has been heard: **M156** moved the static decode onto a worker (so
when a sound becomes audible after a first play is now different), and
**M157** made `musicFrequency` real (so how often music starts now depends on
`options.txt`).

Before running stage 3, know that **the 8-block horn being quieter than the
0-block one is expected** — it is M139's measured divergence (OpenAL does not
attenuate a multi-channel buffer; Rewo does), and M153 made keeping it a
deliberate, pinned decision. A listener not told this will file the correct
behaviour as a bug.

### 2. The streaming decode — the half M156 deliberately left

M156 moved the **static** decode and said why it stopped there:
`getCompleteBuffer` wraps a whole static decode, `getStream` wraps only the
*opening*, and every streaming packet after that decodes on a **single daemon
thread** reached from `ChannelHandle.execute` / `ChannelAccess.scheduleTick`.
A unified worker pool would get static right and spread streaming across N
threads where vanilla serialises it on one.

So this milestone is *modelling `ChannelAccess`'s one-thread discipline*, not
adding more workers. Its hazards are different from M156's two (which were
`played_at` and the pending-key filter): here they are **ordering against
`stop_sound`** and the buffer-queue invariant M144 records (a BUFFER count, not
a duration).

### 3. The rest of the options

M157 made two real. Vanilla has roughly eighty. The natural next group is
`Options.soundSourceVolumes`, which Rewo **already models** as
`CategoryVolumes` (`sound_instance.rs`, and a public field on `SoundEngine`)
and does not surface. It needs the slider widget M157 deliberately did not
build, since neither of its two options is one.

Three traps M157 already paid for, so you don't have to:
`getSerializedName()` is the **uppercase** first constructor argument, not the
lowercase translation key beside it; an option's `onValueUpdate` does **not**
fire at load, so start-up seeding is a *pull*; and Rewo **merges** into
`options.txt` rather than rewriting it, because vanilla owns all ~80 entries
and a wholesale rewrite of a shared game directory discards keybinds and
volumes.

### 4. A pixel witness for the tab list's hearts

M155's eight heart witnesses are model-level — they drive the production
emitters but nothing reads back a rendered heart. `tablistshot` already builds
a real `Gpu` with validation on and an `Offscreen`, so the harness exists. The
sprite values to probe against are in the PNGs (`container` vs
`container_blinking` etc.), and reading them out of the art before writing the
witness is what made M155's other witnesses sound.

### 5. Anything from `REWO_FEATURE_SURVEY.md`

The roadmap for *features* rather than milestones. **Audit any item against
the crates before scheduling it** — that table has already been wrong about
five, which were already at vanilla parity.

## Process (non-negotiable — `AGENT_LOOP_BRIEF.md` has the rest)

- **Ground truth is the decompile** at
  `%APPDATA%/EwoClient/rewo/26.2/decompiled` plus the datagen reports. Cite
  `file:line`. **Never cite a wiki.**
- Every milestone ships a **headless gate or unit tests**, then a **mutation
  battery with a no-op control that must SURVIVE**. `tools/m15*_mutate.py` are
  five worked examples with the discipline baked in (per-mutation timeout,
  restore verified by *bytes*, exit codes not substrings).
- **Read exit codes, never substrings.**
- Branch off `main`, commit per logical step with a message explaining the
  **finding**, merge `--no-ff`. End messages with
  `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- Update `REWO_PLAN.md` §15 and §0.0, then regenerate `AGENTS.md` — it is a
  **generated** mirror of `CLAUDE.md`; the command is in its own header.
- `python tools/render_check.py` stands up a fresh server and runs the
  windowed check. **It is the only gate that drives the windowed client**, so
  run it after any milestone that adds a render path. Use a debug build —
  validation is `cfg!(debug_assertions)`-gated for `live`.

## Traps that cost the last session real time

1. **Backticks in a double-quoted bash string run as command substitution.**
   This bit three times in `python -c "…"` with backticks in doc text, each
   time silently deleting words from a comment. Use a quoted heredoc
   (`python - <<'PY'`) or `-F` for git messages. **Always.**
2. **Don't compute a number by hand that a machine can produce.** The last
   session reported 3272 tests; the real total is 3273, summed from eight
   per-crate counts by eye. Earlier the same session, a totalling script
   printed `TOTAL=0` because `bc` is absent from this Git Bash — the per-crate
   numbers were right and only the sum was broken, silently.
3. **A scripted struct-field addition needs to know if the literal is
   single-line.** An auto-fixer looping over `missing field` errors inserted
   12 duplicate lines after a one-line struct literal. Verify a repair by
   **bytes**, not `git diff`, which cannot distinguish a leftover from
   legitimate uncommitted work.
4. **A `_ => true` catch-all silently absorbs new enum variants.** M152 added
   three restrictive `SlotKind`s that fell straight through `may_place`'s
   catch-all and were granted permission; the compiler cannot see it.
5. **A witness-id collision is invisible to a count.** `containershot`
   reported `N / N` while running fourteen witnesses under seven ids. It now
   rejects duplicates — and the *first* version of that check passed, because
   it keyed on the full name where the collision is in the prefix.
6. **Check a worktree's DIRTY state before calling it litter.**
   `git branch --merged` says nothing about uncommitted work.

## The standing lesson, which recurred four more times last session

**A green suite means the transcription agrees with itself.** Four of the last
session's milestones each found that a claim in the document proposing them
was wrong — the faces were not "structural", `getMusicVolume` was not an
option, "vanilla's `supplyAsync`" was half the story, and one comment was not
decoration beside the code but the code's own justification (and false). In
every case the *code* was checkable and the *prose about it* was not.

So: open the source beside the claim, and prefer a measurement to a sentence.
