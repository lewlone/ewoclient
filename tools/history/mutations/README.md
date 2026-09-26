# Archived mutation batteries (M-series)

One script per milestone, each with its own copy of the harness. They are kept
as a record of what each milestone's tests were shown to catch **at that
commit**. They are not regression tests: every mutation is an exact-text anchor
into the source as it was then, so most anchors no longer match.

New batteries should use `tools/mutate_lib.py` (shared harness: no-op control,
byte-verified restore, exit-code verdicts, final rebuild), or `cargo mutants`
for systematic mutation of a pure crate.
