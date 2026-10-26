"""Extract every block's `SoundType` and every `SoundType`'s seven fields from the
decompiled jar into `crates/rewo-data/src/block_sounds.rs`.

The sounds the player's own actions make (a step, a place, a break, the hit
while mining, a landing) are played by the CLIENT from the block's
`SoundType`, which no datagen report carries: it is a `BlockBehaviour.Properties`
builder call in `Blocks.java`.

* `SoundType.java` declares the constants:
      public static final SoundType WOOD = new SoundType(
          1.0F, 1.0F, SoundEvents.WOOD_BREAK, SoundEvents.WOOD_STEP,
          SoundEvents.WOOD_PLACE, SoundEvents.WOOD_HIT, SoundEvents.WOOD_FALL);
  as `(volume, pitch, break, step, place, hit, fall)`.
* `SoundEvents.java` names each event: `WOOD_BREAK = register("block.wood.break")`.
* A block's properties default to `SoundType.STONE`; `.sound(SoundType.X)` sets
  it; `ofLegacyCopy` / `ofFullCopy` copy it from the named block.

Precedence per block, as `gen_block_hardness.py`: the registration's own
builder chain (its last `.sound`), then the `Properties` helpers it calls, then
the copied block, then STONE. One class overrides `getSoundType` by state
(`DecoratedPotBlock`: cracked or not); it keeps its properties' sound here.

Reuses `gen_block_light.py`'s registration walker, so the tables agree on
which registry names exist. Re-run after a version bump:
    python tools/gen_block_sounds.py
"""
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen_block_light as gbl  # noqa: E402
import gen_block_props as gbp  # noqa: E402

FLOAT = r"(-?\d+(?:\.\d+)?)[Ff]?"
SOUND_TYPE = re.compile(
    r"public static final SoundType ([A-Z0-9_]+) = new SoundType\(\s*"
    + FLOAT + r"\s*,\s*" + FLOAT + r"\s*,"
    + r"\s*SoundEvents\.([A-Z0-9_]+)\s*,\s*SoundEvents\.([A-Z0-9_]+)\s*,"
    + r"\s*SoundEvents\.([A-Z0-9_]+)\s*,\s*SoundEvents\.([A-Z0-9_]+)\s*,"
    + r"\s*SoundEvents\.([A-Z0-9_]+)\s*\)",
    re.S,
)
SOUND_EVENT = re.compile(r"public static final (?:SoundEvent|Holder<SoundEvent>|Holder\.Reference<SoundEvent>) "
                         r"([A-Z0-9_]+) = register(?:ForHolder)?\(\s*\"([a-z0-9_./]+)\"")
SOUND_CALL = re.compile(r"\.sound\(\s*SoundType\.([A-Z0-9_]+)\s*\)")


# Classes whose constructor overrides the properties' sound from a type argument
# (`super(properties.sound(type.soundType()))`): the constructor runs after the
# builder, so this wins. Subclasses inherit it (`WeatheringCopperDoorBlock`).
CTOR_SET_TYPE = {
    "BasePressurePlateBlock": "soundType", "ButtonBlock": "soundType", "DoorBlock": "soundType",
    "FenceGateBlock": "soundType", "StandingSignBlock": "soundType", "TrapDoorBlock": "soundType",
    "WallSignBlock": "soundType", "CeilingHangingSignBlock": "hangingSignSoundType",
    "WallHangingSignBlock": "hangingSignSoundType",
}
TYPE_ARG = re.compile(r"new [A-Za-z]+\([^)]*?(BlockSetType|WoodType)\.([A-Z0-9_]+)")
SE = r"\s*SoundEvents\.([A-Z0-9_]+)\s*"
BLOCK_SET_LONG = re.compile(
    r"public static final BlockSetType ([A-Z0-9_]+) = register\(\s*new BlockSetType\(\s*\"[a-z_]+\"\s*,"
    r"\s*(?:true|false)\s*,\s*(?:true|false)\s*,\s*(?:true|false)\s*,\s*BlockSetType\.PressurePlateSensitivity\.[A-Z]+\s*,"
    r"\s*SoundType\.([A-Z0-9_]+)\s*," + ",".join([SE] * 8) + r"\)", re.S)
BLOCK_SET_SHORT = re.compile(r"public static final BlockSetType ([A-Z0-9_]+) = register\(new BlockSetType\(\"[a-z_]+\"\)\)")
WOOD_LONG = re.compile(
    r"public static final WoodType ([A-Z0-9_]+) = register\(\s*new WoodType\(\s*\"[a-z_]+\"\s*,\s*BlockSetType\.([A-Z0-9_]+)\s*,"
    r"\s*SoundType\.([A-Z0-9_]+)\s*,\s*SoundType\.([A-Z0-9_]+)\s*," + SE + "," + SE + r"\)", re.S)
WOOD_SHORT = re.compile(r"public static final WoodType ([A-Z0-9_]+) = register\(new WoodType\(\"[a-z_]+\",\s*BlockSetType\.([A-Z0-9_]+)\)\)")


def ancestors(cls, block_dir):
    """`cls` and its superclasses, from the decompiled class files."""
    out = []
    while cls and cls not in out:
        out.append(cls)
        f = os.path.join(block_dir, cls + ".java")
        if not os.path.exists(f):
            break
        m = re.search(r"class " + cls + r"(?:<[^>]*>)?\s+extends\s+([A-Za-z0-9_]+)",
                      open(f, encoding="utf8", errors="replace").read())
        cls = m.group(1) if m else None
    return out


def last_sound(text):
    found = None
    for m in SOUND_CALL.finditer(text):
        found = m.group(1)
    return found


def main():
    block_dir = os.path.dirname(gbl.BLOCKS)
    st_src = open(os.path.join(block_dir, "SoundType.java"), encoding="utf8", errors="replace").read()
    ev_src = open(os.path.join(gbl.ROOT, "net/minecraft/sounds/SoundEvents.java"), encoding="utf8",
                  errors="replace").read()
    events = {m.group(1): m.group(2) for m in SOUND_EVENT.finditer(ev_src)}
    types = {}
    for m in SOUND_TYPE.finditer(st_src):
        name, vol, pitch = m.group(1), float(m.group(2)), float(m.group(3))
        evs = [m.group(i) for i in range(4, 9)]
        unknown = [e for e in evs if e not in events]
        if unknown:
            sys.exit(f"[gen_block_sounds] SoundType.{name} names unknown SoundEvents {unknown}")
        types[name] = (vol, pitch, [f"minecraft:{events[e]}" for e in evs])
    declared = len(re.findall(r"public static final SoundType ", st_src))
    if declared != len(types):
        parsed = set(types)
        missed = [n for n in re.findall(r"public static final SoundType ([A-Z0-9_]+)", st_src) if n not in parsed]
        sys.exit(f"[gen_block_sounds] parsed {len(types)} of {declared} SoundType constants; unparsed {missed}")

    props_dir = os.path.join(block_dir, "state", "properties")
    bs_src = open(os.path.join(props_dir, "BlockSetType.java"), encoding="utf8", errors="replace").read()
    wt_src = open(os.path.join(props_dir, "WoodType.java"), encoding="utf8", errors="replace").read()

    def ev(e):
        return f"minecraft:{events[e]}"

    block_sets = {}
    for m in BLOCK_SET_LONG.finditer(bs_src):
        block_sets[m.group(1)] = (m.group(2), [ev(m.group(i)) for i in range(3, 11)])
    short_default = ("WOOD", [ev(e) for e in (
        "WOODEN_DOOR_CLOSE", "WOODEN_DOOR_OPEN", "WOODEN_TRAPDOOR_CLOSE", "WOODEN_TRAPDOOR_OPEN",
        "WOODEN_PRESSURE_PLATE_CLICK_OFF", "WOODEN_PRESSURE_PLATE_CLICK_ON",
        "WOODEN_BUTTON_CLICK_OFF", "WOODEN_BUTTON_CLICK_ON")])
    for m in BLOCK_SET_SHORT.finditer(bs_src):
        block_sets[m.group(1)] = short_default
    n_bs = len(re.findall(r"public static final BlockSetType ", bs_src))
    if n_bs != len(block_sets):
        sys.exit(f"[gen_block_sounds] parsed {len(block_sets)} of {n_bs} BlockSetType constants")
    wood_types = {}
    for m in WOOD_LONG.finditer(wt_src):
        wood_types[m.group(1)] = (m.group(2), m.group(3), m.group(4), ev(m.group(5)), ev(m.group(6)))
    for m in WOOD_SHORT.finditer(wt_src):
        wood_types[m.group(1)] = (m.group(2), "WOOD", "HANGING_SIGN", ev("FENCE_GATE_CLOSE"), ev("FENCE_GATE_OPEN"))
    n_wt = len(re.findall(r"public static final WoodType ", wt_src))
    if n_wt != len(wood_types):
        sys.exit(f"[gen_block_sounds] parsed {len(wood_types)} of {n_wt} WoodType constants")

    src = open(gbl.BLOCKS, encoding="utf8", errors="replace").read()
    known = set(json.load(open(gbl.REGISTRY, encoding="utf8")).keys())
    helpers = gbl.property_helpers(src)
    regs = list(gbl.iter_registrations(src, helpers))
    by_field = {field: parts for field, _n, parts, _c, idx in regs if idx is None}

    def resolve(parts, depth=0):
        own = parts[0]
        helper_text = "\n".join(parts[1:])
        s = last_sound(own)
        if s is None and ".sound(soundType)" in helper_text:
            # A parametric helper: `logProperties(MapColor.X, MapColor.Y, SoundType.WOOD)`.
            args = re.findall(r"SoundType\.([A-Z0-9_]+)", own)
            s = args[-1] if args else None
        s = s or last_sound(helper_text)
        if s is None:
            m = re.search(r"of(?:Legacy|Full)Copy\(\s*([A-Z0-9_]+)\s*\)", own)
            if m and depth < 8 and m.group(1) in by_field:
                s = resolve(by_field[m.group(1)], depth + 1)
        return s

    rows = {}
    set_of = {}
    wood_of = {}
    for _field, name, parts, cls, _idx in regs:
        if f"minecraft:{name}" not in known:
            continue
        s = resolve(parts) or "STONE"
        chain = ancestors(cls, block_dir) if cls else []
        ctor = next((CTOR_SET_TYPE[c] for c in chain if c in CTOR_SET_TYPE), None)
        targ = TYPE_ARG.search(parts[0])
        if targ:
            kind, tname = targ.groups()
            if kind == "BlockSetType":
                set_of[name] = tname
            else:
                wood_of[name] = tname
                set_of[name] = wood_types[tname][0]
        if ctor:
            if not targ:
                sys.exit(f"[gen_block_sounds] {name} ({cls}) sets its sound from a type but names none")
            kind, tname = targ.groups()
            if kind == "BlockSetType":
                s = block_sets[tname][0]
            else:
                wt = wood_types[tname]
                s = wt[1] if ctor == "soundType" else wt[2]
        if s not in types:
            sys.exit(f"[gen_block_sounds] {name} uses unknown SoundType.{s}")
        rows[name] = s

    missing = sorted(n.split(":", 1)[1] for n in known if n.split(":", 1)[1] not in rows)
    dest = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                        "crates", "rewo-data", "src", "block_sounds.rs")
    out = open(dest, "w", encoding="utf8", newline="\n")
    out.write("//! GENERATED by `tools/gen_block_sounds.py` — do not edit.\n")
    out.write(f"//!\n//! Source: decompiled {gbl.VERSION} `SoundType.java`, `SoundEvents.java` and\n")
    out.write("//! `Blocks.java`: every `SoundType` constant and every block's sound type.\n")
    out.write(f"//! {len(types)} sound types; {len(rows)} blocks; {len(missing)} registry names not\n")
    out.write("//! found by the walker (they take `SoundType.STONE`, the `Properties` default).\n\n")
    out.write("/// One `SoundType`: `(volume, pitch, break, step, place, hit, fall)`.\n")
    out.write("#[derive(Clone, Copy, Debug, PartialEq)]\npub struct SoundTypeDef {\n")
    out.write("    pub volume: f32,\n    pub pitch: f32,\n    pub break_sound: &'static str,\n")
    out.write("    pub step_sound: &'static str,\n    pub place_sound: &'static str,\n")
    out.write("    pub hit_sound: &'static str,\n    pub fall_sound: &'static str,\n}\n\n")
    out.write("/// Every `SoundType` constant, by field name.\n#[rustfmt::skip]\n")
    out.write("pub const SOUND_TYPES: &[(&str, SoundTypeDef)] = &[\n")
    for k in sorted(types):
        v, p, e = types[k]
        out.write(f'    ("{k}", SoundTypeDef {{ volume: {v!r}, pitch: {p!r}, break_sound: "{e[0]}", '
                  f'step_sound: "{e[1]}", place_sound: "{e[2]}", hit_sound: "{e[3]}", fall_sound: "{e[4]}" }}),\n')
    out.write("];\n\n/// Every block whose sound type is not `STONE`, as `(block, SoundType field)`.\n")
    out.write("#[rustfmt::skip]\npub const BLOCK_SOUNDS: &[(&str, &str)] = &[\n")
    for k in sorted(rows):
        if rows[k] != "STONE":
            out.write(f'    ("minecraft:{k}", "{rows[k]}"),\n')
    out.write("];\n\n")
    out.write("/// `BlockSetType`: `(name, sound type, [door close, door open, trapdoor close, trapdoor\n")
    out.write("/// open, pressure plate off, pressure plate on, button off, button on])`.\n#[rustfmt::skip]\n")
    out.write("pub const BLOCK_SET_TYPES: &[(&str, &str, [&str; 8])] = &[\n")
    for k in sorted(block_sets):
        st, evs = block_sets[k]
        out.write(f'    ("{k}", "{st}", [' + ", ".join(f'"{e}"' for e in evs) + "]),\n")
    out.write("];\n\n/// `WoodType`: `(name, block set, fence gate close, fence gate open)`.\n#[rustfmt::skip]\n")
    out.write("pub const WOOD_TYPES: &[(&str, &str, &str, &str)] = &[\n")
    for k in sorted(wood_types):
        w = wood_types[k]
        out.write(f'    ("{k}", "{w[0]}", "{w[3]}", "{w[4]}"),\n')
    out.write("];\n\n/// Blocks built on a `BlockSetType` (doors, trapdoors, buttons, plates; the wood\n")
    out.write("/// blocks through their `WoodType`), as `(block, BlockSetType)`.\n#[rustfmt::skip]\n")
    out.write("pub const BLOCK_SETS: &[(&str, &str)] = &[\n")
    for k in sorted(set_of):
        out.write(f'    ("minecraft:{k}", "{set_of[k]}"),\n')
    out.write("];\n\n/// Blocks built on a `WoodType` (fence gates, signs), as `(block, WoodType)`.\n#[rustfmt::skip]\n")
    out.write("pub const BLOCK_WOOD_TYPES: &[(&str, &str)] = &[\n")
    for k in sorted(wood_of):
        out.write(f'    ("minecraft:{k}", "{wood_of[k]}"),\n')
    out.write("];\n")
    # The tags `Entity.playStepSound` / `Player.playStepSound` read.
    for const, tag, doc in (
        ("INSIDE_STEP_SOUND_BLOCKS", "inside_step_sound_blocks",
         "Blocks whose step sound plays from the block the player is inside"),
        ("COMBINATION_STEP_SOUND_BLOCKS", "combination_step_sound_blocks",
         "Blocks whose step sound plays together with the one below"),
        ("CRYSTAL_SOUND_BLOCKS", "crystal_sound_blocks",
         "Blocks that chime (`block.amethyst_block.chime`) when stepped on"),
    ):
        names = sorted(gbp.expand_tag(tag))
        out.write(f"\n/// {doc} (`#minecraft:{tag}`).\n#[rustfmt::skip]\n")
        out.write(f"pub const {const}: &[&str] = &[\n")
        for n in names:
            out.write(f'    "minecraft:{n}",\n')
        out.write("];\n")
    out.close()
    print(f"[gen_block_sounds] {len(types)} sound types, {len(rows)} blocks "
          f"({sum(1 for v in rows.values() if v != 'STONE')} not STONE), {len(missing)} missing: {missing[:8]}")


if __name__ == "__main__":
    main()
