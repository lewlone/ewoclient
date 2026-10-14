"""Check a staged EwoClient bundle (what `package.ps1` writes to dist/EwoClient).

    python tools/check_bundle.py dist/EwoClient

Exits non-zero, naming every problem, if the bundle is not what a release
must be:

- the three binaries, the fonts, the icon, LICENSE.txt and BUILD.txt exist;
- every loader manifest parses, names no `file://` url and no absolute path,
  and every relative library it names exists in the bundle with the sha1 and
  size the manifest says (what the launcher will verify at launch);
- `rewo.exe` carries none of the gate code: no render-check witness strings
  (`live_cmd/render_check.rs` is `cfg(feature = "gates")`) and no gate
  subcommand help (a default-features build has neither);
- BUILD.txt names a commit.

`package.ps1` runs it after staging and fails the packaging if it fails.
Standard library only.
"""

from __future__ import annotations

import hashlib
import json
import re
import sys
from pathlib import Path

BINARIES = ("EwoClient.exe", "rewo.exe", "ewo_jni.dll")
FONTS_MIN = 3  # Fraunces, Newsreader, JetBrains Mono

# Byte strings that exist only in a `--features gates` build of rewo.exe.
# Witness ids are `"rN <description>"` literals in render_check.rs; the
# subcommand names are gate-only clap subcommands.
GATE_MARKERS = (
    b"r1 the run rendered frames",
    b"r6 the hand pass was built",
    b"mobshot",
    b"containershot",
)


def sha1(path: Path) -> str:
    h = hashlib.sha1()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def check_manifests(bundle: Path, problems: list[str]) -> int:
    loader = bundle / "loader" / "0.1.0"
    manifests = sorted(loader.glob("*.json"))
    if not manifests:
        problems.append(f"no loader manifests in {loader}")
        return 0
    checked = 0
    for m in manifests:
        text = m.read_text(encoding="utf-8")
        if "file://" in text:
            problems.append(f"{m.name}: still names a file:// url")
        if re.search(r'"url"\s*:\s*"[A-Za-z]:[\\/]', text):
            problems.append(f"{m.name}: names an absolute Windows path")
        try:
            doc = json.loads(text)
        except json.JSONDecodeError as e:
            problems.append(f"{m.name}: not JSON ({e})")
            continue
        for lib in doc.get("libraries", []):
            art = (lib.get("downloads") or {}).get("artifact")
            if not art:
                continue
            url = art.get("url", "")
            if url.startswith("https://"):
                continue
            f = loader / url
            if not f.is_file():
                problems.append(f"{m.name}: {lib.get('name')} -> {url} is not in the bundle")
                continue
            if art.get("sha1", "").lower() != sha1(f):
                problems.append(f"{m.name}: {lib.get('name')} sha1 does not match {url}")
            if art.get("size") is not None and int(art["size"]) != f.stat().st_size:
                problems.append(f"{m.name}: {lib.get('name')} size does not match {url}")
            checked += 1
    return checked


def check(bundle: Path) -> list[str]:
    problems: list[str] = []
    if not bundle.is_dir():
        return [f"{bundle} is not a directory"]
    for name in BINARIES:
        p = bundle / name
        if not p.is_file() or p.stat().st_size == 0:
            problems.append(f"missing or empty: {name}")
    fonts = list((bundle / "assets" / "fonts").glob("*.ttf"))
    if len(fonts) < FONTS_MIN:
        problems.append(f"assets/fonts holds {len(fonts)} .ttf, expected at least {FONTS_MIN}")
    for name in ("assets/icon.ico", "LICENSE.txt", "BUILD.txt"):
        if not (bundle / name).is_file():
            problems.append(f"missing: {name}")
    build = bundle / "BUILD.txt"
    if build.is_file() and not re.search(r"^commit\s+[0-9a-f]{7,}", build.read_text("utf-8"), re.M):
        problems.append("BUILD.txt names no commit")
    rewo = bundle / "rewo.exe"
    if rewo.is_file():
        data = rewo.read_bytes()
        for marker in GATE_MARKERS:
            if marker in data:
                problems.append(f"rewo.exe contains gate code ({marker.decode()!r}); build it without --features gates")
    checked = check_manifests(bundle, problems)
    if not problems:
        print(f"check_bundle: OK — {bundle} ({checked} bundled loader jar reference(s) verified)")
    return problems


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: python tools/check_bundle.py <bundle dir>", file=sys.stderr)
        return 2
    problems = check(Path(argv[1]))
    for p in problems:
        print(f"check_bundle: {p}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
