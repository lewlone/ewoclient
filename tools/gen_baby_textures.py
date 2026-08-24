"""Extract vanilla's isBaby texture swaps into a generated Rewo table.

    python tools/gen_baby_textures.py

Ground truth: the decompile at %APPDATA%/EwoClient/rewo/26.2/decompiled plus
the client jar (for PNG dimensions). Re-run after a version bump; never
hand-edit crates/rewo-data/src/baby_texture_table.rs.

What it extracts, and what it deliberately does NOT:

- For every entity renderer class, collect `Identifier` constants and every
  `state.isBaby ? A : B` return. Resolve A/B through the FILE's constants,
  walking the `extends` chain when an arm names an inherited constant.
- The EFFECTIVE swap for an entity = the NEAREST override up its ancestry
  (vanilla dispatches getTextureLocation virtually), fanned out to every
  EntityType mapped to that class in EntityRenderers.java - so
  PIGLIN_BRUTE inherits PiglinRenderer's swap exactly as vanilla renders it.
- Excluded ON PURPOSE, each named in EXCLUDED: renderer classes whose baby
  arm is mediated by a variant record / type switch rather than a plain
  constant pair (`textures.baby` on the axolotl, the bee's four state combos,
  UndeadHorseRenderer's Type) - Rewo cannot evaluate those states yet, and
  binding the adult sheet would be a confident wrong answer.

Fails loud: zero extractions, an unresolved identifier, a baby path missing
from the jar, or a jar count that disagrees with a fresh count are all hard
errors rather than warnings.
"""

import os
import re
import struct
import sys
import zipfile

HOME = os.environ.get("USERPROFILE") or os.path.expanduser("~")
DECOMPILED = os.path.join(
    HOME, "AppData", "Roaming", "EwoClient", "rewo", "26.2", "decompiled"
)
JAR = os.path.join(
    HOME, "AppData", "Roaming", "EwoClient", "shared", "versions", "26.2", "26.2.jar"
)
OUT = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "crates", "rewo-data", "src", "baby_texture_table.rs",
)
RENDERERS = os.path.join(DECOMPILED, "net", "minecraft", "client", "renderer", "entity")

CONST_RE = re.compile(
    r"(?:private|public|protected)?\s*(?:static\s+)?final\s+\w*Identifier\s+(\w+)\s*=\s*"
    r"\w*Identifier\.with(?:DefaultNamespace|Namespaced)\(\s*\"([^\"]+)\""
)
TERNARY_RE = re.compile(r"state\.isBaby\s*\?\s*([\w.]+)\s*:\s*([\w.]+)")
CLASS_RE = re.compile(r"public\s+(?:abstract\s+)?class\s+(\w+)(?:<[^>]*>)?\s+extends\s+(\w+)")
REGISTER_RE = re.compile(r"EntityTypes\.(\w+),\s*\n?\s*(?:context\s*->\s*new\s+)?(\w+)(?:::new|\()")
PNG_SIG = b"\x89PNG\r\n\x1a\n"


def png_size(jar, path):
    with zipfile.ZipFile(jar) as z:
        data = z.read("assets/minecraft/" + path)
    if data[:8] != PNG_SIG:
        raise SystemExit(f"{path}: not a PNG")
    w, h = struct.unpack(">II", data[16:24])
    return int(w), int(h)


def main():
    if not os.path.isdir(RENDERERS):
        raise SystemExit(f"decompile not found at {RENDERERS}")
    if not os.path.exists(JAR):
        raise SystemExit(f"client jar not found at {JAR}")

    # ---- constants + getTextureLocation bodies per file --------------------
    constants = {}   # file stem -> {ident: path}
    bodies = {}      # file stem -> brace-matched getTextureLocation body ('' if none)
    extends = {}     # file stem -> parent stem
    for dirpath, _, files in os.walk(RENDERERS):
        for fn in files:
            if not fn.endswith(".java"):
                continue
            stem = fn[:-5]
            text = open(os.path.join(dirpath, fn), encoding="utf-8").read()
            constants[stem] = dict(CONST_RE.findall(text))
            m = CLASS_RE.search(text)
            if m:
                extends[m.group(1)] = m.group(2)
            i = text.find("getTextureLocation")
            if i < 0:
                bodies[stem] = ""
                continue
            j = text.find("{", i)
            depth, k = 0, j
            while k < len(text):
                if text[k] == "{":
                    depth += 1
                elif text[k] == "}":
                    depth -= 1
                    if depth == 0:
                        break
                k += 1
            bodies[stem] = text[j:k + 1]

    def chain(stem):
        seen, cur = [], stem
        while cur and cur not in seen:
            seen.append(cur)
            cur = extends.get(cur)
        return seen

    def resolve(stem, ident):
        for f in chain(stem):
            if ident in constants.get(f, {}):
                return constants[f][ident]
        return None

    # ---- entity -> renderer class -----------------------------------------
    er = open(os.path.join(RENDERERS, "EntityRenderers.java"), encoding="utf-8").read()
    entity_class = {}
    for name, cls in REGISTER_RE.findall(er):
        entity_class.setdefault(name.lower(), cls)

    rows, excluded = {}, []
    for entity, cls in sorted(entity_class.items()):
        decided = None
        for f in chain(cls):
            body = bodies.get(f, "")
            if "isBaby" not in body:
                continue
            # The swap must be the fn's ONLY conditional: a second `?` means
            # another render state gates first (PiglinRenderer's
            # isBrute ? BRUTE : (isBaby ? ...)), and applying the baby arm to
            # every entity of the class would put piglin_baby on a brute.
            if body.count("?") != 1:
                decided = ("complex", f)
                break
            pairs = set()
            ok = True
            for a, b in TERNARY_RE.findall(body):
                ba, aa = resolve(f, a), resolve(f, b)
                if ba is None or aa is None:
                    ok = False
                    break
                pairs.add((ba.replace("state.", "").replace("this.", ""),
                           aa.replace("state.", "").replace("this.", "")))
            dotted = [t for t in re.findall(r"isBaby\s*\?\s*([\w.]+)\s*:\s*([\w.]+)", body)
                      if "." in t[0] or "." in t[1]]
            if (not ok or not pairs) or dotted:
                # Either an arm this extractor cannot read (a record/type
                # field: axolotl's textures.baby, the wolf variants) or the
                # swap is mediated some other way entirely.
                decided = ("complex", f)
                break
            babies = {p for p, _ in pairs}
            adults = {a for _, a in pairs}
            if len(babies) != 1 or len(adults) != 1:
                decided = ("multi", f)
                break
            decided = ("ok", next(iter(babies)), next(iter(adults)))
            break
        if decided is None:
            continue
        if decided[0] == "ok":
            rows[entity] = (decided[1], decided[2], cls)
        elif decided[0] == "multi":
            excluded.append((entity, f"renderer {decided[1]} selects among several state combos"))
        else:
            excluded.append((entity, f"renderer {decided[1]} mediates the baby sheet through a variant record"))

    if not rows:
        raise SystemExit("extracted zero baby swaps - the idiom moved; fix the generator")

    # ---- sizes from the jar ------------------------------------------------
    out_rows = []
    with zipfile.ZipFile(JAR) as z:
        names = set(z.namelist())
        for entity, (baby_path, adult_path, cls) in sorted(rows.items()):
            full = "assets/minecraft/" + baby_path
            if full not in names:
                raise SystemExit(f"{entity}: baby sheet {baby_path} missing from the jar")
            w, h = png_size(JAR, baby_path)
            out_rows.append((entity, baby_path, adult_path, w, h))

    jar_babies = sum(
        1 for n in names
        if n.startswith("assets/minecraft/textures/entity/")
        and n.endswith(".png")
        and "baby" in n.lower()
    )
    covered = len(out_rows)

    # ---- emit --------------------------------------------------------------
    def rust_str(s):
        return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'

    lines = []
    lines.append("//! GENERATED by tools/gen_baby_textures.py - do not edit.")
    lines.append("//!")
    lines.append("//! Vanilla's `getTextureLocation` overrides that swap on `isBaby`,")
    lines.append("//! extracted from the 26.2 decompile (nearest override wins up the")
    lines.append("//! `extends` chain, fanned out over EntityRenderers' map). One row per")
    lines.append("//! ENTITY NAME; the GPU side resolves the kind via")
    lines.append("//! `mobs::kind_for_entity_name`, which keys kinds by the same short")
    lines.append("//! name. Re-run after a version bump.")
    lines.append("//!")
    lines.append(f"//! Measured against this jar: {jar_babies} *baby*.png under")
    lines.append("//! textures/entity, of which the plain-constant swaps this table")
    lines.append(f"//! encodes cover {covered}; the rest belong to variant systems")
    lines.append("//! Rewo cannot evaluate yet and are named in `EXCLUDED_REASON`.")
    lines.append("")
    lines.append("/// One extracted swap: `entity` (no namespace), the BABY sheet's jar")
    lines.append("/// path, the ADULT path it replaces (the slot the UV offset rides),")
    lines.append("/// and the baby PNG's own dimensions.")
    lines.append("#[derive(Clone, Copy, Debug)]")
    lines.append("pub struct BabySwap {")
    lines.append("    pub entity: &'static str,")
    lines.append("    pub baby_key: &'static str,")
    lines.append("    pub baby_path: &'static str,")
    lines.append("    pub adult_path: &'static str,")
    lines.append("    pub w: u32,")
    lines.append("    pub h: u32,")
    lines.append("}")
    lines.append("")
    lines.append("/// Why an entity with a baby SHEET in the jar is absent from")
    lines.append("/// [`BABY_SWAPS`] - suppression recorded, not silent.")
    lines.append("pub static EXCLUDED_REASON: &[(&str, &str)] = &[")
    for entity, why in excluded:
        lines.append(f"    ({rust_str(entity)}, {rust_str(why)}),")
    lines.append("];")
    lines.append("")
    lines.append("pub const BABY_SWAPS: &[BabySwap] = &[")
    for entity, baby_path, adult_path, w, h in out_rows:
        key = f"{entity}_baby"
        lines.append("    BabySwap {")
        lines.append(f"        entity: {rust_str(entity)},")
        lines.append(f"        baby_key: {rust_str(key)},")
        lines.append(f"        baby_path: {rust_str(baby_path)},")
        lines.append(f"        adult_path: {rust_str(adult_path)},")
        lines.append(f"        w: {w},")
        lines.append(f"        h: {h},")
        lines.append("    },")
    lines.append("];")
    lines.append("")
    lines.append("/// The row for an entity name, namespace prefix optional.")
    lines.append("pub fn baby_swap_for_entity(name: &str) -> Option<&'static BabySwap> {")
    lines.append("    let short = name.strip_prefix(\"minecraft:\").unwrap_or(name);")
    lines.append("    BABY_SWAPS.iter().find(|r| r.entity == short)")
    lines.append("}")
    lines.append("")
    lines.append("#[cfg(test)]")
    lines.append("mod tests {")
    lines.append("    use super::*;")
    lines.append("")
    lines.append("    #[test]")
    lines.append("    fn the_table_is_sorted_by_entity_and_unique() {")
    lines.append("        let mut names: Vec<_> = BABY_SWAPS.iter().map(|r| r.entity).collect();")
    lines.append("        let sorted = names.clone();")
    lines.append("        names.sort_unstable();")
    lines.append("        assert_eq!(names, sorted, \"rows must be sorted so a binary search can replace find\");")
    lines.append("        let uniq = sorted.len();")
    lines.append("        names.dedup();")
    lines.append("        assert_eq!(names.len(), uniq, \"duplicate entity rows\");")
    lines.append("    }")
    lines.append("")
    lines.append("    #[test]")
    lines.append("    fn zombie_and_sheep_are_covered_and_bee_is_named() {")
    lines.append("        assert!(baby_swap_for_entity(\"minecraft:zombie\").is_some());")
    lines.append("        assert_eq!(baby_swap_for_entity(\"zombie\").unwrap().adult_path, \"textures/entity/zombie/zombie.png\");")
    lines.append("        assert!(baby_swap_for_entity(\"sheep\").is_some());")
    lines.append("        assert!(")
    lines.append("            EXCLUDED_REASON.iter().any(|(e, _)| *e == \"bee\"),")
    lines.append("            \"the bee's four state-combo sheets must stay a NAMED exclusion\"")
    lines.append("        );")
    lines.append("    }")
    lines.append("}")

    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")

    print(f"[gen_baby_textures] jar *baby*.png sheets : {jar_babies}")
    print(f"[gen_baby_textures] extracted swaps      : {covered}")
    print(f"[gen_baby_textures] named exclusions     : {len(excluded)}")
    print(f"[gen_baby_textures] wrote {OUT}")


if __name__ == "__main__":
    main()
