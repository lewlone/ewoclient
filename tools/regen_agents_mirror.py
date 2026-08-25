"""Regenerate AGENTS.md from CLAUDE.md, preserving the header.

    python tools/regen_agents_mirror.py

The mirror is a blind whole-file rename of CLAUDE.md (CLAUDE.md -> AGENTS.md)
with its first-15-line comment header re-prepended. Run after any CLAUDE.md
edit; the diff must be exactly the paragraphs you changed in CLAUDE.md.
"""

import io
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CLAUDE = os.path.join(ROOT, "CLAUDE.md")
AGENTS = os.path.join(ROOT, "AGENTS.md")

END = "-->"


def main():
    agents_old = io.open(AGENTS, encoding="utf-8", newline="").read()
    head_end = agents_old.index(END) + len(END)
    header = agents_old[:head_end]

    body = io.open(CLAUDE, encoding="utf-8", newline="").read().replace("CLAUDE.md", "AGENTS.md")
    io.open(AGENTS, "w", encoding="utf-8", newline="").write(header + "\n\n" + body)

    agents_new = io.open(AGENTS, encoding="utf-8", newline="").read()
    print(f"header: {header.count(chr(10))} lines; new file: {agents_new.count(chr(10))} lines")


if __name__ == "__main__":
    sys.exit(main())
