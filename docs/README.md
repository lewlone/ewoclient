# docs/ — history and reasoning (read on demand)

Until 2026-09-26, `CLAUDE.md` was a 7,192-line, ~519 KB log (about 131k tokens
loaded into every session). It was cut to an always-loaded index plus
path-scoped rules in `.claude/rules/`. **Every line of the old file was moved
here verbatim**: the five files below, concatenated in line order, reproduce
it byte for byte. Nothing was paraphrased away.

Each file opens with a short provenance header and, where relevant, a list of
**superseded statements**: things the history says that are no longer true,
with a pointer to the current rule. The body below the header is unedited.

| File | Original `CLAUDE.md` lines | Contents |
|---|---|---|
| [`history/ewoclient-v1.md`](history/ewoclient-v1.md) | 1–630 | The original project brief, reference materials, non-negotiables, locked architecture, v1 scope, build-sequence table, Velvet tokens, render graph, glossary, Step 1–16 implementation notes |
| [`history/ewoclient-v2-phases.md`](history/ewoclient-v2-phases.md) | 631–1500 | v2 Phases A–E, the bundle phase, Phases F–H, the unfocused-swap leak, the legit / pvp split, the floating-card window, the 2026-05-31 memory + performance pass |
| [`history/claude-md-session-log.md`](history/claude-md-session-log.md) | 1501–2147, 6937–7192 | Every italic "Update (… session)" footer, 2026-05-26 → 2026-08-25. **This is where M143–M180 are summarised.** |
| [`rewo/milestones-M0-M86.md`](rewo/milestones-M0-M86.md) | 2148–4918 | Rewo milestone record M0–M86, plus the Velvet type stack and the headless wire subsystems |
| [`rewo/milestones-M87-M142.md`](rewo/milestones-M87-M142.md) | 4919–6936 | Containers, recipe book, the docs audit, the chat arc, tickable sounds (M141) and ambient handlers (M142) |

For Rewo, `REWO_PLAN.md` §15 is the maintained per-milestone log; these files
are the `CLAUDE.md`-side record of the same work, often with different emphasis
(process lessons, cross-milestone findings).

## Other retired documents in `history/`

Moved out of the repo root and `.claude/` when they stopped being current.
They are records: their numbers, paths and "next" lists are as of when they
were written, and relative links inside them may point at their old location.

| File | What it was |
|---|---|
| [`history/HANDOFF.md`](history/HANDOFF.md) | The last session-to-session continuation prompt for Rewo (post-M179/M180) |
| [`history/NEXT_SESSION_PROMPT.md`](history/NEXT_SESSION_PROMPT.md) | An earlier (M157-era) handoff prompt |
| [`history/AGENT_LOOP_BRIEF.md`](history/AGENT_LOOP_BRIEF.md) | Process rules for the headless two-agent loop (still sound; the durable ones are in `.claude/rules/rewo.md`) |
| [`history/REWO_MOB_REDO_HANDOFF.md`](history/REWO_MOB_REDO_HANDOFF.md) | The mob-model redo brief, completed 2026-07-22 |
| [`history/MEMORY.md`](history/MEMORY.md) | An orientation snapshot from 2026-08-23/25 |
| [`history/PHASE_E_PLAN.md`](history/PHASE_E_PLAN.md) | Phase E (in-game HUD) build plan |
| [`history/PHASE_F_PLAN.md`](history/PHASE_F_PLAN.md) | Phase F (profiles and dashboard) plan |
| [`history/PHASE_G_PLAN.md`](history/PHASE_G_PLAN.md) | Phase G (modules) plan |
| [`history/PHASE_H_PLAN.md`](history/PHASE_H_PLAN.md) | Phase H (social) plan |

## Reviews and audits (current)

- [`REVIEW-2026-09.md`](REVIEW-2026-09.md): the September 2026 independent
  review and its fix ledger.
- [`AUDIT-2026-10.md`](AUDIT-2026-10.md): the October 2026 full audit, which
  re-grades that review, with a status column for its own findings.
- [`OVERNIGHT-2026-10-15.md`](OVERNIGHT-2026-10-15.md): the overnight push
  of 2026-10-15 (M186–M189, release packaging), its decisions and what it
  left for the owner to playtest.
- [`RELEASE.md`](RELEASE.md): how to cut a release bundle and what a person
  still has to check.

Retired mutation batteries are in `tools/history/mutations/`.

## Where new material goes

- A milestone's narrative goes to `REWO_PLAN.md` §15 (Rewo) or a new file here.
- A durable, path-specific rule goes to the matching `.claude/rules/*.md`.
- A cross-cutting rule goes to `CLAUDE.md`, in one line, which stays under ~250
  lines.
