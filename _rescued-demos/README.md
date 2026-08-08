# `_rescued-demos`

Four PNGs kept from the M15/M16 gate runs, when `rewo demo`'s byte-identity was
being established as a merge gate.

**All four are byte-identical to each other and to the current output** —
sha256 prefix `2cc56b4acbfb92cb`, which is the hash every milestone entry in
`REWO_PLAN.md` §15 asserts. Verify with:

```
rewo demo --out /tmp/demo.png
```

They are therefore redundant with that one-line assertion, and nothing in the
tree references them. They are kept rather than deleted because they are the
only *artifact* form of the reference frame: a hash tells you the render
changed, and these let you diff and see **where**. If you ever need that, diff
against `m15-final/demo.png` — the other three are the same bytes under
different names.

If the demo output is ever intentionally changed, replace all four (or delete
the directory) in the same commit that changes the hash, so the two cannot
disagree.
