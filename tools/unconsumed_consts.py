"""Find `pub const`s that nothing outside their own file reads.

    python tools/unconsumed_consts.py [--all]

**Why this exists.** M135 and M136 were the same bug in one day: a value
transcribed from vanilla that no production code consumed, so no render could
contradict it and no gate could see it. M136's had been wrong for eighty
milestones and **its own doc comment restated the wrong value**, so the code and
the prose agreed with each other and neither agreed with the decompile. An
unread constant is not necessarily wrong — it is simply the population where
being wrong is invisible, which is where to look first.

This is a CANDIDATE FINDER, not a verdict. This repo deliberately records facts
as constants nothing reads (a `.mcmeta` sheet size, a builder default used by
zero registrations, a ceiling an assertion references). Those are legitimate;
they still need their values checked against the decompile, which is a human
step this script cannot do.

**Result of the first run (2026-08-10, at M137): 265 files, 856 `pub const`s,
9 with zero readers anywhere — and every one of them checked out.** So M136 was
not the tip of a systemic problem, which is a useful negative result: it is the
reason not to run this again for a while.

Eight carry a value and all eight match the decompile — `BAND_COLOR` and
`DEFAULT_ROW_BACKGROUND` (`PlayerTabOverlay.java:180/191/226` and `:192`),
`BOOK_OVERLAY_PANEL_SHEET`/`_BORDER`, `BUILDER_DEFAULT_SIZE`
(`EntityType.java:477`), `ABSOLUTE_MAX_STACK` (`Item.java:114`),
`BEACON_PAYMENT_ICON_Y` (`BeaconScreen.java:130-134`) and `BUTTON_SHEET_H`. The
ninth, `SEEN_RECIPE_IS_ONE_VARINT`, is `()` — a marker naming a packet, with no
value that could be wrong.

Worth knowing while reading the beacon one: vanilla writes its x offsets as
`41 + 22`, `42 + 44`, `42 + 66`, and **the base changes from 41 to 42 partway
through**, so the row is 63/86/108 where a uniform `41 + 22n` gives 63/85/107.
Rewo has it right. That is the flavour of thing this list is for.

Re-run it after a milestone that adds transcribed constants.
"""
import argparse
import io
import os
import re
import sys

ROOT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "crates")
CRATES = ["rewo-gpu", "rewo-world", "rewo-net", "rewo-data", "rewo-mesh", "rewo-proto", "rewo-app"]

DECL = re.compile(r"^\s*pub const ([A-Z][A-Z0-9_]*)\s*:\s*([^=]+?)\s*=", re.M)
# One tokenizing pass per file beats one regex per constant: 856 constants over
# 265 files is 227k scans the naive way, and it took over two minutes.
#
# **`*` and not `{2,}`.** The first cut required three characters, which made
# every short constant unmatchable and therefore unconditionally "unread" —
# `EditBox`'s `A`/`C`/`V`/`X` key codes are read on the line below their
# declaration and this reported all four as dead. A detector whose own pattern
# cannot express the thing it is looking for reports the bug it was built to
# find, which is the failure mode this repo keeps meeting; it showed up here as
# a candidate list that grew when the tool got faster.
IDENT = re.compile(r"\b[A-Z][A-Z0-9_]*\b")
NUMERIC = re.compile(r"\b(u8|u16|u32|u64|i8|i16|i32|i64|f32|f64|usize|isize)\b")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--all", action="store_true", help="also list own-file-only constants")
    args = ap.parse_args()

    files = []
    for c in CRATES:
        for dirpath, _, names in os.walk(os.path.join(ROOT, c, "src")):
            files += [os.path.join(dirpath, n) for n in names if n.endswith(".rs")]
    if not files:
        sys.exit("walked no .rs files under %s -- wrong ROOT?" % ROOT)

    text = {f: io.open(f, encoding="utf-8", errors="replace").read() for f in files}

    decls = {}
    for f, s in text.items():
        for m in DECL.finditer(s):
            decls.setdefault(m.group(1), []).append((f, m.group(2).strip()))

    counts = {f: {} for f in files}
    for f, s in text.items():
        d = counts[f]
        for tok in IDENT.findall(s):
            d[tok] = d.get(tok, 0) + 1

    dead, onlyself = [], []
    for name, sites in decls.items():
        if len(sites) != 1:
            continue  # declared twice: which one a reference means is ambiguous
        own, ty = sites[0]
        if not NUMERIC.search(ty) and not ty.startswith("("):
            continue
        other = sum(counts[f].get(name, 0) for f in files if f is not own)
        inside = counts[own].get(name, 0) - 1  # minus the declaration itself
        rel = os.path.relpath(own, ROOT).replace("\\", "/")
        (dead if (other == 0 and inside <= 0) else onlyself if other == 0 else []).append(
            (name, ty, rel, inside)
        )

    print("scanned %d files, %d pub consts" % (len(files), len(decls)))
    print()
    print("=== ZERO readers anywhere -- a wrong value here is invisible ===")
    for name, ty, rel, _ in sorted(dead, key=lambda r: (r[2], r[0])):
        print("  %-34s %-12s %s" % (name, ty[:12], rel))
    print("  total: %d" % len(dead))
    if args.all:
        print()
        print("=== read only inside their own file ===")
        for name, ty, rel, inside in sorted(onlyself, key=lambda r: (r[2], r[0])):
            print("  %-34s %-12s reads=%-3d %s" % (name, ty[:12], inside, rel))
        print("  total: %d" % len(onlyself))


if __name__ == "__main__":
    main()
