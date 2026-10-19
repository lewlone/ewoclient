"""Generate crates/rewo-data/src/block_entity_blocks.rs from the 26.2 decompile.

`LevelChunk.setBlockState` creates a block entity ON THE CLIENT when the new
block is one a `BlockEntityType` lists as valid (`BlockEntityTypes.register`'s
`validBlocks`), and the server sends no packet for it: a chest placed with
`setblock` or by a player arrives as a bare `block_update`. Rewo needs the
same block -> block-entity-type map to do what vanilla does (M247).

The table is `BlockEntityTypes.java`'s register calls. Each names its id
(`BlockEntityTypeIds.X`, `create("x")` in `BlockEntityTypeIds.java`) and its
blocks, as `Blocks.FIELD` (the registry name is the field in lower case,
checked against `blocks.json`) or one of four group forms whose members are
not spelled out in the call:

* `Blocks.COPPER_CHEST.asList()` / `Blocks.COPPER_GOLEM_STATUE.asList()` —
  a weathering group: the eight `[waxed_][exposed_|weathered_|oxidized_]`
  variants of the base name.
* `Blocks.BANNER.asList()` / `Blocks.WALL_BANNER.asList()` /
  `Blocks.DYED_SHULKER_BOX.asList()` — a colour set: the sixteen
  `<colour>_<name>` blocks.

Each group is expanded against `blocks.json` and must have exactly the count
its form implies.

Run:  python tools/gen_block_entity_blocks.py
Exit: 0 on success; 1 on any block, id or group that does not resolve. It
      fails loud rather than emitting a short table: a missing entry is a
      block entity that silently never exists on the client.
"""

from __future__ import annotations

import json
import os
import re
import sys
from typing import NoReturn

BASE = os.path.expandvars(r"%APPDATA%/EwoClient/rewo/26.2")
DEC = os.path.join(BASE, "decompiled/net/minecraft/world/level/block/entity")
TYPES = os.path.join(DEC, "BlockEntityTypes.java")
IDS = os.path.join(DEC, "BlockEntityTypeIds.java")
BLOCKS = os.path.join(BASE, "datagen/generated/reports/blocks.json")
REGISTRIES = os.path.join(BASE, "datagen/generated/reports/registries.json")
OUT = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "crates", "rewo-data", "src", "block_entity_blocks.rs",
)

COLOURS = [
    "white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray",
    "light_gray", "cyan", "purple", "blue", "brown", "green", "red", "black",
]
WEATHER = ["", "exposed_", "weathered_", "oxidized_"]


def die(msg: str) -> NoReturn:
    print(f"[gen_block_entity_blocks] FAIL — {msg}")
    sys.exit(1)


def main() -> None:
    for p in (TYPES, IDS, BLOCKS, REGISTRIES):
        if not os.path.isfile(p):
            die(f"missing {p}")
    blocks = set(json.load(open(BLOCKS, encoding="utf-8")).keys())
    be_registry = set(
        json.load(open(REGISTRIES, encoding="utf-8"))["minecraft:block_entity_type"]["entries"]
    )
    ids = dict(
        re.findall(r"public static final ResourceKey<BlockEntityType<\?>> (\w+) = create\(\"([a-z_]+)\"\)",
                   open(IDS, encoding="utf-8").read())
    )
    src = open(TYPES, encoding="utf-8").read()

    def block(field: str) -> str:
        name = f"minecraft:{field.lower()}"
        if name not in blocks:
            die(f"Blocks.{field} -> {name} is not in blocks.json")
        return name

    def group(field: str) -> list[str]:
        base = field.lower()
        if base in ("copper_chest", "copper_golem_statue"):
            out = [f"minecraft:{wax}{w}{base}" for wax in ("", "waxed_") for w in WEATHER]
            want = 8
        elif base in ("banner", "wall_banner", "dyed_shulker_box"):
            stem = "shulker_box" if base == "dyed_shulker_box" else base
            out = [f"minecraft:{c}_{stem}" for c in COLOURS]
            want = 16
        else:
            die(f"unknown group form Blocks.{field}.asList()")
        missing = [b for b in out if b not in blocks]
        if missing or len(out) != want:
            die(f"group Blocks.{field}: {len(out)} members, missing {missing}")
        return out

    rows: list[tuple[str, list[str]]] = []
    calls = re.findall(
        r"public static final BlockEntityType<\w+> \w+ = register\(\s*BlockEntityTypeIds\.(\w+),\s*\w+::new,(.*?)\);",
        src,
        re.S,
    )
    if not calls:
        die("no register calls parsed")
    for id_field, rest in calls:
        if id_field not in ids:
            die(f"BlockEntityTypeIds.{id_field} has no create(..)")
        name = f"minecraft:{ids[id_field]}"
        if name not in be_registry:
            die(f"{name} is not in the block_entity_type registry")
        members: list[str] = []
        for m in re.finditer(r"Blocks\.(\w+)(\.asList\(\))?", rest):
            members += group(m.group(1)) if m.group(2) else [block(m.group(1))]
        if not members:
            die(f"{name}: no valid blocks parsed from {rest.strip()[:80]!r}")
        rows.append((name, sorted(set(members))))

    missing_types = be_registry - {n for n, _ in rows}
    if missing_types:
        die(f"registered types with no register(..) parsed: {sorted(missing_types)}")
    seen: dict[str, str] = {}
    for name, members in rows:
        for b in members:
            if b in seen:
                die(f"{b} is valid for both {seen[b]} and {name}")
            seen[b] = name

    lines = [
        "// Generated by tools/gen_block_entity_blocks.py from the 26.2 decompile's",
        "// `BlockEntityTypes.java` (each type's `validBlocks`). Do not edit; re-run the",
        "// generator.",
        "",
        "/// `(block entity type, the blocks it is valid for)` — the blocks for which",
        "/// `LevelChunk.setBlockState` creates a block entity on the client (M247).",
        "#[rustfmt::skip]",
        "pub const BLOCK_ENTITY_BLOCKS: &[(&str, &[&str])] = &[",
    ]
    for name, members in rows:
        body = ", ".join(f'"{b}"' for b in members)
        lines.append(f'    ("{name}", &[{body}]),')
    lines.append("];")
    open(OUT, "w", encoding="utf-8", newline="\n").write("\n".join(lines) + "\n")
    print(f"[gen_block_entity_blocks] OK — {len(rows)} types, {len(seen)} blocks -> {OUT}")


if __name__ == "__main__":
    main()
