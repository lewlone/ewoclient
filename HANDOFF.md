# Next-session prompt — Rewo

Copy everything below the line into a fresh session.

---

You are continuing **Rewo** — the from-scratch native Minecraft Java client in Rust that lives in `crates/rewo-*` of the EwoClientV3 workspace. The `ewo-*` crates are an unrelated Skia launcher + JNI HUD project; **do not touch them**.

**Read `REWO_PLAN.md` §0.0 HANDOFF first.** It carries the current measurements, the gate list, the live-check recipe, the load-bearing gotchas, the shared-resource allocation table and a categorised known-gaps list. `AGENT_LOOP_BRIEF.md` carries the process rules and deliberately *points at* §0.0 rather than restating it — that form is load-bearing, don't duplicate numbers back into it.

## State

Everything is on `main` and pushed. Nothing else exists.

```bash
git rev-list --left-right --count origin/main...main   # expect: 0   0
```

Also expect: clean `git status`, one worktree plus whatever the harness makes for you, and no branch holding a commit off `main`. No sha is quoted here on purpose — a file cannot name its own commit, and two earlier drafts of this prompt tried and were both wrong on commit.

**Verified on the merged tree, not on the branches that fed it** (2026-08-20):

* **3336 tests / 0 failures** across eight crates — world 1201, net 1195, gpu 293, data 231, app 228, mesh 52, proto 16, audio 120. Read them per crate off the runner: `rewo-app` needs `--bins`, the other seven `--lib`, and a crate whose tests fail to compile prints no `test result` line at all and reads as silence.
* **37 serverless gates** green, 0 validation errors. Enumerate them from `rewo --help`, never from a list.
* **`live --render-check` 54/54, exit 0** — `python tools/render_check.py`. Debug build; validation is `cfg!(debug_assertions)`-gated for `live`.
* `soundshot` **37** default / **57** under `--features audio`.
* Demo PNG `2cc56b4acbfb92cb` — byte-identical since M15.
* `REWO_PACKET_COVERAGE.md` **119 / 0 / 22**, classes A and B empty. Its §2 table is machine-checked by a unit test in `ids.rs`.

The last run shipped **M158–M165**. Read their §15 entries before starting: they contain the findings, not just the changes.

## What to do next

§0.0's "what to do next" box was rewritten on 2026-08-20 against the merged tree — trust it over anything older in the file. In priority order:

1. **`resource-pack=` hangs the client.** If the destination server sets it, `rewo live` never opens a window and never errors — the config task never self-finishes and the 30 s socket timeout cannot fire because a keep-alive arrives every 15 s. `crates/rewo-net/src/lib.rs:619`'s ignore arm is where it dies. Fix is a two-field decode and a 17-byte ack, ~30 lines. **Check whether Frogsy sets it before scheduling anything else** — if it does, this is first and everything else waits.
2. **`is_usable_for_crafting` tests the wrong field** — `SlotText::name.is_some()` (custom OR item name) where vanilla tests `has(CUSTOM_NAME)` alone (`crates/rewo-world/src/inventory.rs`), so a stack carrying only a patched `item_name` is wrongly refused by the recipe-book solver. Small, exact oracle, live consequence.
3. **The `*shot` witness-NAME namespace is unguarded.** `Checker::record` dedups nothing and every gate is fail-closed on a *count*, so two witnesses sharing a name are counted twice and read as a **pass**. M160 guarded the `rNN` namespace one level up and left this one — same milestone shape, and cheap.
4. **The HUD's real gaps** — armour bar, air bubbles, mob-effect icons, vehicle health, jump bar; zero mentions each in `hud.rs`. Four traps already paid for: there is no `ArmorLevelBar` or `Gui.renderArmor` in 26.2 (the site is `Hud.java:815 extractArmor`); no `AirLevelBar` either, and air is ten independently-sprited 9×9 bubbles rather than a bar; the jump bar replaces the **XP** bar, not the food column; and `hudshot_cmd.rs` is the *Velvet* gate with zero `Gpu`/`Offscreen`, so it cannot host a HUD pixel witness.

**Deliberately not next, with the measurement:** the dialog framework (a real 26.2 server sends `show_dialog` **zero** times, both tags empty), advancements (**1562 of 1688** have no `display`, so the screen shows 126), and `render-misc [AO]`/`[FLOW]` — both need a decision the user has not made. For `[AO]`, note that **doing the cheap heuristic first pins the heuristic in witnesses and blocks the exact version**, so do not start either half.

**Known-deletable code is named in §0.0** rather than hidden — `MobDef::textures`, the `upload_skin`/`upload_cape`/`upload_face` slot recycling, `explode`'s call site, and three serverless-only `live_cmd` bypasses. Closing any of them is a legitimate small milestone.

## Process (non-negotiable — `AGENT_LOOP_BRIEF.md` has the rest)

* Ground truth is the decompile at `%APPDATA%/EwoClient/rewo/26.2/decompiled` plus the datagen reports. Cite `file:line`. **Never cite a wiki.**
* Every milestone ships a headless gate or unit tests, **then a mutation battery with a no-op control that must SURVIVE**. `tools/m15*_mutate.py`, `m158`, `m162`–`m165` are worked examples.
* **Read exit codes, never substrings.**
* **Route a battery through whatever you are claiming coverage from.** M155 shipped a 10/10 battery through `cargo test` for a feature its gate could not see at all.
* **A witness that asks its subject where to look grades everything except the thing they share.** Predict from the decompile's literals, the jar's own bytes, or a re-declared constant.
* **Claim your `rNN` and any atlas coordinates in §0.0's shared-resource allocation table, in the commit that starts the work.** Fifteen parallel specs once picked `r48` independently.
* Branch off `main`, commit per logical step with a message explaining the **finding**, merge `--no-ff`. End messages with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
* Update §15 and §0.0, then **regenerate `AGENTS.md`** — it is a generated mirror of `CLAUDE.md` and the command is in its header. Never hand-edit it.
* `python tools/render_check.py` is the only gate that drives the **windowed** client. Run it after any milestone that adds a render path — **and after any integration**, because a branch being green is not evidence about the merged tree.

## Traps that cost this project real time

1. **Backticks inside a double-quoted bash string run as command substitution**, and a `git commit -m "…"` containing double quotes shatters into pathspec errors. Use `git commit -F <file>` with a heredoc.
2. **A verification script that names its own subject in a constant grades the wrong thing.** A sweep script hard-coded its root to a *worktree* and silently graded whatever branch that worktree was on — reporting 3288 for a tree with 3336 tests.
3. **`git branch --merged` says nothing about uncommitted work.** Check a worktree's dirty state before calling it litter, and archive the diff before removing it.
4. **A resolved merge conflict can be syntactically valid and semantically wrong**, and one that cuts through a function call leaves unbalanced delimiters that only the compiler finds. After every conflict resolution: build, then re-check the thing the conflict was about.
5. **Line endings**: exactly five files under `crates/` are not pure LF (`rewo-gpu/src/cem.rs` is pure CRLF; `mobshot_cmd.rs`, `vanilla_hier.rs`, `chunk.rs`, `light.rs` are mixed). Measure as **bytes in Python**; never put a raw control character in a shell command. New files from editor tools may arrive CRLF.
6. **Never run `cargo fmt`** — the code is hand-formatted.

## The one thing no machine can do

**The listening pass, and it is the user's.**

```bash
cargo run -p rewo-audio --example listen            # 13 staged stages, ~3 min
cargo build -p rewo-app --features audio
rewo live --audio                                   # or REWO_AUDIO=1
```

**No gate in this project opens an audio device** — absent, muted, exclusive-mode and unplugged all look identical from inside the process, so everything checkable passes and *that is not the same claim*. Before stage 3, know that the 8-block horn being quieter than the 0-block one is **expected**: M139 measured that OpenAL does not attenuate a multi-channel buffer while Rewo does, and M153 made keeping it a pinned decision. A listener not told this files the correct behaviour as a bug.

Audio is **off by default**, so a default build links no audio stack and the other gates are unchanged.

## The standing lesson

A green suite means the transcription agrees with itself. In the last run: a feature with no witness at all shipped with a 10/10 battery; four parallel branches were each green alone while holding production code that could be deleted whole; the client played no music for three milestones with 36 gates green; and my own measuring script graded the wrong tree.

**Open the source beside the claim, and prefer a measurement to a sentence.**
