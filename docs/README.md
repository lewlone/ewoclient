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

## Where new material goes

- A milestone's narrative goes to `REWO_PLAN.md` §15 (Rewo) or a new file here.
- A durable, path-specific rule goes to the matching `.claude/rules/*.md`.
- A cross-cutting rule goes to `CLAUDE.md`, in one line, which stays under ~250
  lines.
