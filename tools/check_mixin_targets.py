#!/usr/bin/env python3
"""Verify every mixin in ingame-mod/ still has its targets in the Minecraft jars.

A mixin whose target method is renamed or removed does not fail the build (the
mod compiles against the 26.2 jar and Mixin resolves targets by string at
runtime): with `require = 0` it silently does nothing, with the default
`require = 1` it crashes the game on launch. The 26.2 `Minecraft.destroy()`
miss (the exit-watchdog arm point) went unnoticed for exactly that reason. This
script resolves each target against the real jars with `javap`, so such a miss
fails CI instead.

What is checked, per mixin class and per Minecraft version it applies to:

  * the `@Mixin(X.class)` target class exists in the jar;
  * every `method = "name"` / `"name(desc)ret"` / `"Lowner;name(desc)ret"` of
    @Inject / @Redirect / @Modify* / @Wrap* exists *declared in that class*
    (Mixin does not target inherited methods) and, if a descriptor is given,
    matches it;
  * every `@At(value = "INVOKE" | "INVOKE_ASSIGN" | "INVOKE_STRING", target =
    "Lowner;name(desc)ret")` call really occurs inside the targeted method's
    bytecode (the same test Mixin applies); `FIELD` targets likewise;
  * every `@Shadow` field / method exists in the target class;
  * every mixin source file is listed in a mixin config and vice versa.

Per-version selection mirrors `EwoMixinPlugin`: a class named `*262` applies on
26.2+ only; a class in its `LEGACY_ONLY` set applies below 26.2 only; the rest
apply everywhere. The set is read from EwoMixinPlugin.java, not copied here.

Usage:
    python tools/check_mixin_targets.py [--versions 26.1 26.2]
        [--appdata DIR] [--javap PATH]

Jars come from <appdata>/EwoClient/shared/versions/<v>/<v>.jar (the same
Mojmap jars build.ps1 compiles against). Exit status: 0 all targets resolve,
1 at least one miss or config mismatch, 2 setup problem (jar/javap not found).
"""
from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
MOD_SRC = REPO / "ingame-mod" / "src" / "main"
JAVA_ROOT = MOD_SRC / "java"
RES_ROOT = MOD_SRC / "resources"
PLUGIN = JAVA_ROOT / "dev" / "lewlone" / "ewohud" / "mixin" / "EwoMixinPlugin.java"

INJECTORS = (
    "Inject|Redirect|ModifyArg|ModifyArgs|ModifyVariable|ModifyConstant|"
    "ModifyExpressionValue|ModifyReceiver|ModifyReturnValue|WrapOperation|"
    "WrapWithCondition|WrapMethod|WrapWithCondition"
)
# Annotations that name a target without a `method =` string. None are used
# today; if one appears this script must learn about it rather than skip it.
UNSUPPORTED = ("Overwrite", "Accessor", "Invoker")


# --------------------------------------------------------------------------
# Source parsing
# --------------------------------------------------------------------------

def strip_comments(src: str) -> str:
    """Drop // and /* */ comments while leaving string literals intact."""
    out = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            out.append(src[i:j + 1])
            i = j + 1
        elif c == "'":
            j = i + 1
            while j < n and src[j] != "'":
                j += 2 if src[j] == "\\" else 1
            out.append(src[i:j + 1])
            i = j + 1
        elif src.startswith("//", i):
            j = src.find("\n", i)
            i = n if j < 0 else j
        elif src.startswith("/*", i):
            j = src.find("*/", i + 2)
            end = n if j < 0 else j + 2
            out.append("\n" * src.count("\n", i, end))  # keep line numbers true
            i = end
        else:
            out.append(c)
            i += 1
    return "".join(out)


def balanced(src: str, open_idx: int) -> tuple[str, int]:
    """Contents of the (...) group opening at open_idx, and the index after it."""
    depth, i, n = 0, open_idx, len(src)
    while i < n:
        c = src[i]
        if c == '"':
            i += 1
            while i < n and src[i] != '"':
                i += 2 if src[i] == "\\" else 1
        elif c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return src[open_idx + 1:i], i + 1
        i += 1
    raise ValueError("unbalanced parentheses")


STR = re.compile(r'"((?:[^"\\]|\\.)*)"')


class Injection:
    def __init__(self, kind, methods, ats, line):
        self.kind = kind          # annotation name
        self.methods = methods    # list of method strings
        self.ats = ats            # list of (value, target|None)
        self.line = line


class Mixin:
    def __init__(self, path: Path):
        self.path = path
        self.simple = path.stem
        self.target = None            # fully-qualified class name
        self.injections: list[Injection] = []
        self.shadows: list[tuple[str, bool, int]] = []   # name, is_method, line
        self.problems: list[str] = []
        self._parse()

    def _parse(self):
        raw = self.path.read_text(encoding="utf-8")
        src = strip_comments(raw)
        pkg = re.search(r"^\s*package\s+([\w.]+);", src, re.M)
        imports = dict(
            (m.group(1).rsplit(".", 1)[-1], m.group(1))
            for m in re.finditer(r"^\s*import\s+([\w.]+);", src, re.M)
        )

        def line_of(idx):
            return src.count("\n", 0, idx) + 1

        m = re.search(r"@Mixin\s*\(", src)
        if not m:
            self.problems.append("no @Mixin annotation")
            return
        body, _ = balanced(src, m.end() - 1)
        cls = re.search(r"([\w.]+)\.class", body)
        tgt = re.search(r'targets\s*=\s*"([^"]+)"', body)
        if cls:
            name = cls.group(1)
            if "." in name:
                self.target = name
            elif name in imports:
                self.target = imports[name]
            else:
                self.target = (pkg.group(1) + "." if pkg else "") + name
        elif tgt:
            self.target = tgt.group(1).replace("/", ".")
        else:
            self.problems.append("cannot read the @Mixin target")

        for un in UNSUPPORTED:
            if re.search(r"@" + un + r"\b", src):
                self.problems.append(
                    f"uses @{un}, which check_mixin_targets.py does not understand yet")

        for m in re.finditer(r"@(" + INJECTORS + r")\s*\(", src):
            body, _ = balanced(src, m.end() - 1)
            methods = []
            mm = re.search(r"\bmethod\s*=\s*(\{[^}]*\}|\"(?:[^\"\\]|\\.)*\")", body)
            if mm:
                methods = [s.group(1) for s in STR.finditer(mm.group(1))]
            else:
                self.problems.append(
                    f"line {line_of(m.start())}: @{m.group(1)} without a method = string")
            ats = []
            for am in re.finditer(r"@At\s*\(", body):
                abody, _ = balanced(body, am.end() - 1)
                val = re.search(r'\bvalue\s*=\s*"([^"]*)"', abody)
                if val is None:
                    val = re.match(r'\s*"([^"]*)"', abody)
                tg = re.search(r'\btarget\s*=\s*"([^"]*)"', abody)
                ats.append((val.group(1) if val else None, tg.group(1) if tg else None))
            self.injections.append(Injection(m.group(1), methods, ats, line_of(m.start())))

        for m in re.finditer(r"@Shadow\b(\s*\([^)]*\))?", src):
            # skip further annotations/modifiers up to the declarator
            rest = src[m.end():]
            decl = re.match(r"((?:\s*@\w+(?:\([^)]*\))?)*[^;{=(]*?)(\(|;|=)", rest, re.S)
            if not decl:
                self.problems.append(f"line {line_of(m.start())}: cannot read @Shadow member")
                continue
            names = re.findall(r"[\w$]+", decl.group(1))
            if not names:
                continue
            name = names[-1]
            if name.startswith("shadow$"):
                name = name[len("shadow$"):]
            self.shadows.append((name, decl.group(2) == "(", line_of(m.start())))


# --------------------------------------------------------------------------
# Mixin config / version selection (mirrors EwoMixinPlugin)
# --------------------------------------------------------------------------

def legacy_only() -> set[str]:
    src = strip_comments(PLUGIN.read_text(encoding="utf-8"))
    m = re.search(r"LEGACY_ONLY\s*=\s*Set\.of\((.*?)\)\s*;", src, re.S)
    if not m:
        raise SystemExit("cannot find LEGACY_ONLY in EwoMixinPlugin.java")
    return {s.group(1) for s in STR.finditer(m.group(1))}


def line_at_least(version: str, major: int, minor: int) -> bool:
    parts = [p for p in re.split(r"[^0-9]+", version) if p != ""]
    maj = int(parts[0]) if len(parts) > 0 else 0
    mi = int(parts[1]) if len(parts) > 1 else 0
    return maj > major or (maj == major and mi >= minor)


def applies(simple: str, version: str, legacy: set[str]) -> bool:
    new_line = line_at_least(version, 26, 2)
    if simple.endswith("262"):
        return new_line
    if simple in legacy:
        return not new_line
    return True


def config_listed() -> set[str]:
    listed = set()
    for cfg in sorted(RES_ROOT.glob("*.mixins.json")):
        data = json.loads(cfg.read_text(encoding="utf-8"))
        pkg = data["package"]
        for key in ("mixins", "client", "server"):
            for name in data.get(key, []):
                listed.add(f"{pkg}.{name}")
    return listed


# --------------------------------------------------------------------------
# javap
# --------------------------------------------------------------------------

class ClassInfo:
    def __init__(self, name):
        self.name = name
        self.methods: list[tuple[str, str, str]] = []   # name, desc, code text
        self.fields: dict[str, str] = {}


MEMBER = re.compile(r"^  (\S.*?);\s*$")


def parse_javap(text: str, fqn: str) -> ClassInfo:
    info = ClassInfo(fqn)
    simple = fqn.rsplit(".", 1)[-1].rsplit("$", 1)[-1]
    cur = None  # (kind, name, desc, [code lines])
    lines = text.splitlines()
    i = 0
    pending = None

    def flush():
        nonlocal pending
        if pending is None:
            return
        kind, name, desc, code = pending
        if kind == "method":
            info.methods.append((name, desc, "\n".join(code)))
        else:
            info.fields[name] = desc
        pending = None

    while i < len(lines):
        ln = lines[i]
        m = MEMBER.match(ln)
        if m and not ln.startswith("   "):
            flush()
            decl = m.group(1)
            if decl.startswith("static {"):
                pending = ["method", "<clinit>", "", []]
            elif "(" in decl:
                head = decl.split("(", 1)[0].split()
                name = head[-1] if head else ""
                if name == fqn or name.rsplit(".", 1)[-1] == simple:
                    name = "<init>"
                pending = ["method", name, "", []]
            else:
                name = decl.split()[-1]
                pending = ["field", name, "", []]
        elif pending is not None:
            d = re.match(r"^    descriptor:\s*(\S+)", ln)
            if d and not pending[2]:
                pending[2] = d.group(1)
            else:
                pending[3].append(ln)
        i += 1
    flush()
    return info


def run_javap(javap: str, jar: Path, fqn: str):
    """ClassInfo for fqn in jar, or None if the class is not there."""
    proc = subprocess.run(
        [javap, "-p", "-c", "-s", "-cp", str(jar), fqn],
        capture_output=True, text=True, encoding="utf-8", errors="replace")
    if proc.returncode != 0 or "Compiled from" not in proc.stdout:
        return None
    return parse_javap(proc.stdout, fqn)


def find_javap(explicit: str | None, appdata: Path) -> str:
    if explicit:
        return explicit
    cand = appdata / "EwoClient" / "jdks" / "temurin-25" / "bin" / "javap.exe"
    if cand.is_file():
        return str(cand)
    found = shutil.which("javap")
    if found:
        return found
    raise SystemExit("javap not found (looked in %s and PATH); pass --javap" % cand.parent)


# --------------------------------------------------------------------------
# Checks
# --------------------------------------------------------------------------

def split_method_ref(s: str):
    """'[Lowner;]name[(desc)ret]' -> (name, desc|None)."""
    s = re.sub(r"^L[\w/$]+;", "", s)
    m = re.match(r"^([\w$<>*]+)(\(.*\).+)?$", s)
    if not m:
        return None, None
    return m.group(1), m.group(2)


def at_target_regex(value: str, target: str):
    if value in ("INVOKE", "INVOKE_ASSIGN", "INVOKE_STRING"):
        m = re.match(r"^L([\w/$]+);([\w$<>]+)(\(.*\).+)$", target)
        if not m:
            return None
        return re.compile(r"//\s*(?:Interface)?Method\s+" + re.escape(
            f"{m.group(1)}.{m.group(2)}:{m.group(3)}") + r"\s*$", re.M)
    if value == "FIELD":
        m = re.match(r"^L([\w/$]+);([\w$]+):(.+)$", target)
        if not m:
            return None
        return re.compile(r"//\s*Field\s+" + re.escape(
            f"{m.group(1)}.{m.group(2)}:{m.group(3)}") + r"\s*$", re.M)
    return None


def check_mixin(mx: Mixin, info: ClassInfo | None, version: str) -> list[str]:
    errs = []
    if info is None:
        return [f"target class {mx.target} not found in {version} jar"]
    for inj in mx.injections:
        wanted = []
        for ms in inj.methods:
            name, desc = split_method_ref(ms)
            if name is None:
                errs.append(f"line {inj.line}: unreadable method string {ms!r}")
                continue
            if "*" in name:
                rx = re.compile("^" + re.escape(name).replace(r"\*", ".*") + "$")
                hits = [m for m in info.methods if rx.match(m[0])]
            else:
                hits = [m for m in info.methods
                        if m[0] == name and (desc is None or m[1] == desc)]
            if not hits:
                near = sorted({m[0] for m in info.methods
                               if name.lower() in m[0].lower() or m[0].lower() in name.lower()})
                errs.append(
                    f"line {inj.line}: @{inj.kind} method {ms!r} not declared in "
                    f"{mx.target}" + (f" (similar: {', '.join(near[:6])})" if near else ""))
            wanted.extend(hits)
        for value, target in inj.ats:
            if value is None or target is None:
                continue
            rx = at_target_regex(value, target)
            if rx is None:
                if value in ("INVOKE", "INVOKE_ASSIGN", "INVOKE_STRING", "FIELD"):
                    errs.append(f"line {inj.line}: unreadable @At target {target!r}")
                continue
            if wanted and not any(rx.search(code) for _, _, code in wanted):
                errs.append(
                    f"line {inj.line}: @At({value}) target {target!r} is not called/read "
                    f"inside {', '.join(sorted({w[0] for w in wanted}))}")
    for name, is_method, line in mx.shadows:
        if is_method:
            ok = any(m[0] == name for m in info.methods)
        else:
            ok = name in info.fields
        if not ok:
            errs.append(f"line {line}: @Shadow {'method' if is_method else 'field'} "
                        f"{name!r} not declared in {mx.target}")
    return errs


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--versions", nargs="+", default=["26.1", "26.2"])
    ap.add_argument("--appdata", default=os.environ.get("APPDATA")
                    or str(Path.home() / "AppData" / "Roaming"))
    ap.add_argument("--javap")
    args = ap.parse_args()

    appdata = Path(args.appdata)
    jars = {}
    for v in args.versions:
        jar = appdata / "EwoClient" / "shared" / "versions" / v / f"{v}.jar"
        if not jar.is_file():
            print(f"setup: Minecraft {v} jar not found at {jar}", file=sys.stderr)
            return 2
        jars[v] = jar
    javap = find_javap(args.javap, appdata)

    mixins = [Mixin(p) for p in sorted(JAVA_ROOT.rglob("*Mixin*.java"))
              if "mixin" in p.parent.name and p.stem != "EwoMixinPlugin"]
    legacy = legacy_only()
    failures = 0

    # config <-> source agreement
    listed = config_listed()
    on_disk = set()
    for mx in mixins:
        fqn = ".".join(mx.path.relative_to(JAVA_ROOT).with_suffix("").parts)
        on_disk.add(fqn)
    for fqn in sorted(on_disk - listed):
        print(f"FAIL  {fqn}: source exists but is in no *.mixins.json")
        failures += 1
    for fqn in sorted(listed - on_disk):
        print(f"FAIL  {fqn}: listed in a mixins json but has no source file")
        failures += 1

    # one javap run per (version, class), in parallel
    wanted = {(v, mx.target) for mx in mixins if mx.target
              for v in jars if applies(mx.simple, v, legacy)}
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        futs = {k: pool.submit(run_javap, javap, jars[k[0]], k[1]) for k in wanted}
        infos = {k: f.result() for k, f in futs.items()}

    checked = 0
    for mx in mixins:
        rel = mx.path.relative_to(REPO).as_posix()
        for p in mx.problems:
            print(f"FAIL  {rel}: {p}")
            failures += 1
        if not mx.target:
            continue
        vs = [v for v in jars if applies(mx.simple, v, legacy)]
        if not vs:
            print(f"skip  {mx.simple}: applies to none of {', '.join(jars)}")
            continue
        for v in vs:
            checked += 1
            errs = check_mixin(mx, infos[(v, mx.target)], v)
            if errs:
                for e in errs:
                    print(f"FAIL  {mx.simple} [{v}] {e}")
                failures += len(errs)
            else:
                print(f"ok    {mx.simple} [{v}] -> {mx.target}")

    print(f"\n{checked} mixin/version pairs checked against {', '.join(jars)}; "
          f"{failures} problem(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
