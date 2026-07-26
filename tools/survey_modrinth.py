#!/usr/bin/env python3
"""Survey every open-source client-side Fabric mod on Modrinth.

This is the machine behind REWO_FEATURE_SURVEY.md. Re-run it when the survey
gets stale (a new Minecraft version line, or a year of ecosystem drift) and
regenerate the tables in that document from this output. Like the other
tools/gen_*.py extractors it is *derived data* -- the cache is written outside
the repo and is not committed.

    python tools/survey_modrinth.py            # fetch (cached 7d) + print tables
    python tools/survey_modrinth.py --refresh  # force a re-fetch
    python tools/survey_modrinth.py --rank     # rank the Rewo candidate features

Cache: %APPDATA%/EwoClient/rewo/survey/modrinth-<facet-hash>.json

Population definition (this is the thing to keep stable across re-runs -- it
reproduced the 9,291 the survey was written against):

    project_type:mod
    categories:fabric
    open_source:true
    server_side:(unsupported OR optional)

`open_source:true` is load-bearing and NOT cosmetic. It is what excludes
Sodium (Polyform Shield 1.0.0) and EntityCulling (a bespoke protective
license) -- both source-available, neither OSI-open. Rewo reimplements rather
than bundles, so a mod's license governs whether its source may be read for
reference at all. Do not drop that facet to "get more data".
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import os
import re
import sys
import time
import urllib.parse
import urllib.request

BASE = "https://api.modrinth.com/v2/search"
UA = "lewlone/ewoclient-rewo-survey (+https://github.com/lewlone/ewoclient)"

FACETS = [
    ["project_type:mod"],
    ["categories:fabric"],
    ["open_source:true"],
    ["server_side:unsupported", "server_side:optional"],
]

# Version lines that count as "alive". Bump when the client pin moves; a mod
# that stopped at 1.20 is ecosystem archaeology, not a demand signal.
MODERN = {
    "1.21", "1.21.1", "1.21.2", "1.21.3", "1.21.4", "1.21.5", "1.21.6",
    "1.21.7", "1.21.8", "1.21.9", "1.21.10", "1.21.11", "26.1", "26.2",
}

CACHE_TTL = 7 * 24 * 3600

# Licenses that forbid or restrict derivative/competing use. A mod tagged this
# way must not be read as a reference implementation for Rewo.
NON_OSI = re.compile(r"licenseref|all-rights|^arr$", re.I)


# --------------------------------------------------------------------------- fetch


def cache_path() -> str:
    h = hashlib.sha1(json.dumps(FACETS).encode()).hexdigest()[:10]
    root = os.environ.get("APPDATA") or os.path.expanduser("~/.config")
    d = os.path.join(root, "EwoClient", "rewo", "survey")
    os.makedirs(d, exist_ok=True)
    return os.path.join(d, f"modrinth-{h}.json")


def _page(offset: int, limit: int = 100) -> dict:
    qs = urllib.parse.urlencode({
        "facets": json.dumps(FACETS),
        "limit": limit,
        "offset": offset,
        "index": "downloads",
    })
    req = urllib.request.Request(f"{BASE}?{qs}", headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)


KEEP = ("slug", "title", "description", "categories", "downloads", "follows",
        "license", "versions", "date_modified", "author", "client_side", "server_side")


def fetch(refresh: bool = False) -> list[dict]:
    path = cache_path()
    if not refresh and os.path.exists(path) and time.time() - os.path.getmtime(path) < CACHE_TTL:
        return json.load(open(path, encoding="utf-8"))

    first = _page(0)
    total = first["total_hits"]
    hits = list(first["hits"])
    print(f"fetching {total:,} projects ...", file=sys.stderr)

    off = 100
    while off < total:
        for attempt in range(5):
            try:
                hits.extend(_page(off)["hits"])
                break
            except Exception as exc:                       # noqa: BLE001 - retry any transport error
                print(f"  retry @{off}: {exc}", file=sys.stderr)
                time.sleep(2 + attempt * 3)
        else:
            raise SystemExit(f"failed to fetch offset {off}")
        off += 100
        time.sleep(0.12)

    slim = [{k: h.get(k) for k in KEEP} for h in hits]
    json.dump(slim, open(path, "w", encoding="utf-8"))
    print(f"cached {len(slim):,} -> {path}", file=sys.stderr)
    return slim


def license_id(m: dict) -> str:
    lic = m.get("license")
    return lic if isinstance(lic, str) else (lic or {}).get("id", "?")


def alive(mods: list[dict]) -> list[dict]:
    return [m for m in mods if MODERN & set(m.get("versions") or [])]


# ----------------------------------------------------------------- feature demand

# Each entry counts INDEPENDENT reimplementations of one feature. That count --
# not download total -- is the primary signal: a feature 100 authors rebuilt is
# an itch nobody has definitively scratched, while a feature with huge downloads
# and few mods already has a winner and is a "ship it, don't innovate" item.
FEATURES = {
    "Tooltip overhaul":          r"tooltip",
    "Item durability display":   r"durabilit",
    "Reach / hit indicators":    r"hit ?(range|indicator|color|reg)|reach|attack indicator|hitbox",
    "Screenshot tooling":        r"screenshot|panorama|isometric render|photo mode|picture mode",
    "Loading / menu flow":       r"loading screen|splash screen|title screen|main menu",
    "Minimap / waypoints":       r"minimap|mini[- ]map|waypoint|world ?map",
    "Zoom":                      r"\bzoom",
    "Fullbright / gamma":        r"fullbright|full bright|\bgamma\b|night ?vision|brightness (beyond|control|plus)",
    "Custom crosshair":          r"crosshair",
    "Capes / cosmetics":         r"\bcape(s)?\b|cosmetic",
    "Freecam / freelook":        r"freecam|free ?look|detach(ed)? camera",
    "Armor HUD":                 r"armou?r ?(hud|bar)|armou?r durabilit",
    "Player / mob health bars":  r"health ?(bar|indicator|display)|hp ?bar|mob plaque",
    "Schematics":                r"litematica|schematic|worldedit cui",
    "Toggle sprint / sneak":     r"toggle ?(sprint|sneak)|auto ?sprint",
    "Shoulder / 3rd-person cam": r"third[- ]person|shoulder surf|over[- ]the[- ]shoulder",
    "Status-effect timers":      r"status effect|potion (counter|display|effect)|effect (bar|timer)",
    "Chat QoL":                  r"chat ?(patch|tweak|tool|plus|log|history|tab|timestamp)|timestamp.*chat",
    "Inventory sorting":         r"inventory (sort|sorting|management|profiles)|sort(ing)? (your |the )?inventor",
    "Discord rich presence":     r"discord|rich presence",
    "Held-item / enchant info":  r"held item|item info|enchantment description",
    "Ping display":              r"\bping\b.*(display|view|numeric|tab)|display.*\bping\b",
    "Keystrokes display":        r"keystroke|key ?display",
    "Server list QoL":           r"server list|multiplayer (menu|screen)|add server|server country",
    "Block placement helper":    r"block placement|bridging|reach[- ]?around|smart(block)? placement",
    "Auto-reconnect":            r"auto[- ]?reconnect",
    "Borderless fullscreen":     r"borderless|windowed fullscreen",
    "Sound physics / muffling":  r"sound (physic|muffl|tweak)|audio (improve|engine|tweak)",
    "Chat heads / bubbles":      r"chat ?head|talk ?(bubble|balloon)|chat ?bubble",
    "Shulker box preview":       r"shulker (box )?(tooltip|preview|contents)",
    "Dynamic lights":            r"dynamic ?light|lambdyn",
    "Light-level overlay":       r"light ?(level )?overlay|spawn(able)? overlay",
}

# Problem classes. A/B/C exist only because the game is a JVM client with a mod
# loader -- Rewo dissolves them by construction. D is Rewo's actual roadmap.
CLASSES = {
    "A. JVM/render performance": r"\b(fps|frame ?rate|performance|optimi[sz]|memory usage|ram usage|lag|stutter|culling|faster|speed ?up|load ?time)\b",
    "B. OptiFine-pack parity":   r"\b(optifine|mcpatcher|connected texture|custom entity (model|texture)|emissive|cem\b|cit\b|animated texture|custom gui|skybox|colormap)\b",
    "C. Modding infrastructure": r"\b(librar|api\b|config(uration)? (librar|system|api)|core ?mod|mod ?menu|loader)\b",
}


def text(m: dict) -> str:
    return f"{m.get('title') or ''} {m.get('description') or ''}"


def report(mods: list[dict]) -> None:
    live = alive(mods)
    total_dl = sum(m["downloads"] for m in live)

    print(f"\npopulation: {len(mods):,} matched  |  {len(live):,} still on {min(MODERN)}+")
    ranked = sorted(live, key=lambda m: -m["downloads"])
    for n in (50, 100, 250, 500, 1000):
        share = sum(m["downloads"] for m in ranked[:n]) / total_dl * 100
        print(f"  top {n:>4} carry {share:5.1f}% of {total_dl/1e6:,.0f}M downloads")

    print("\ndownload mass by problem class")
    seen: set[str] = set()
    for name, pat in CLASSES.items():
        rx = re.compile(pat, re.I)
        hit = [m for m in live if rx.search(text(m)) and m["slug"] not in seen]
        seen.update(m["slug"] for m in hit)
        dl = sum(m["downloads"] for m in hit)
        print(f"  {name:<28} {len(hit):>5} mods {dl/1e6:>8,.0f}M {dl/total_dl*100:>5.1f}%")
    rest = [m for m in live if m["slug"] not in seen]
    dl = sum(m["downloads"] for m in rest)
    print(f"  {'D. QoL / features (Rewo roadmap)':<28} {len(rest):>5} mods {dl/1e6:>8,.0f}M {dl/total_dl*100:>5.1f}%")

    print("\nfeature demand (independent reimplementations)")
    print(f"  {'mods':>5} {'dl(M)':>8}  feature")
    rows = []
    for name, pat in FEATURES.items():
        rx = re.compile(pat, re.I)
        hit = [m for m in live if rx.search(text(m))]
        rows.append((len(hit), sum(m["downloads"] for m in hit) / 1e6, name))
    for n, dl, name in sorted(rows, reverse=True):
        print(f"  {n:>5} {dl:>8.1f}  {name}")

    # The reason open_source:true matters -- surface anything reference-unsafe
    # that still ranks high enough to be tempting.
    blocked = [m for m in ranked[:400] if NON_OSI.search(license_id(m))]
    if blocked:
        print("\nreference-UNSAFE despite ranking in the top 400 (do not read the source):")
        for m in blocked[:15]:
            print(f"  {m['downloads']//1000:>7}k  {m['title'][:34]:<34} {license_id(m)}")


# ------------------------------------------------------------------ candidate rank

# Ranking policy (chosen by the user, 2026-07-26): **breadth-first, weight
# independent mods, divide by effort.**
#
# The two demand signals disagree on purpose and the disagreement is the
# information -- see REWO_FEATURE_SURVEY.md §5. `mods` carries the weight
# because a feature 100 authors rebuilt is an itch nobody scratched;
# downloads are kept only as an escape hatch.
#
# MODS_WEIGHT = 1.0 is pure mods/effort. A 0.75 blend (reintroducing downloads
# as a minority term, to stop a *solved* high-demand feature like Chat Heads --
# 8 mods, 50M downloads -- ranking last for having won) was measured and made
# **no material difference**: the top 8 were near-identical and nothing moved
# more than 4 places. So the simpler rule stands. Lower this only if a future
# candidate appears with very few mods and enormous downloads, where the
# mods-only signal would genuinely mislead.
MODS_WEIGHT = 1.0

# (name, mods, downloads_M, effort 1..5, in_fabric_client, in_rewo, port_bundle)
#
# `in_fabric_client` = ewo-jni modules / HUD widgets / bundled jar.
# `in_rewo`          = actually exists in the rewo-* crates today.
# `port_bundle`      = covered by the single "port the module + HUD set into
#                      Rewo" milestone, so it is grouped rather than scored
#                      individually -- scoring nine ports separately would
#                      flood the top of the table with one piece of work.
CANDIDATES = [
    # name                        mods  dl_M  eff  fabric  rewo   port
    ("Tooltip overhaul",           115,  66.0,  2,  False, False, False),
    ("Item durability display",    103,  10.6,  1,  False, False, False),
    ("Screenshot tooling",          83,  30.3,  2,  False, False, False),
    ("Loading / menu flow",         77,  95.3,  4,  False, False, False),
    ("Minimap / waypoints",         74,  27.9,  5,  False, False, False),
    ("Zoom",                        67,  83.3,  1,  True,  False, True ),
    ("Fullbright / gamma",          61,  16.7,  1,  True,  False, True ),
    ("Custom crosshair",            58,  68.4,  2,  True,  False, True ),
    ("Reach / hit indicators",      85,  19.9,  2,  True,  False, True ),
    ("Armor HUD",                   48,  19.1,  1,  True,  False, True ),
    ("Toggle sprint / sneak",       39,   2.8,  1,  True,  False, True ),
    ("Freecam / freelook",          25,  20.5,  3,  True,  False, True ),
    ("Ping display",                25,  14.1,  1,  True,  False, True ),
    ("Keystrokes display",          21,   0.5,  1,  True,  False, True ),
    ("Capes / cosmetics",           49,  29.7,  2,  False, False, False),
    ("Player / mob health bars",    48,   6.3,  2,  False, False, False),
    ("Schematics",                  42,  25.8,  5,  False, False, False),
    ("Shoulder / 3rd-person cam",   37,  27.2,  2,  False, False, False),
    ("Status-effect timers",        34,  28.0,  2,  False, False, False),
    ("Chat QoL",                    34,  11.6,  3,  False, False, False),
    ("Inventory sorting",           32, 124.9,  4,  False, False, False),
    ("Discord rich presence",       29,  14.4,  1,  False, False, False),
    ("Held-item / enchant info",    23,  53.7,  2,  False, False, False),
    ("Block placement helper",      18,  17.0,  3,  False, False, False),
    ("Server list QoL",             18,   5.2,  3,  False, False, False),
    ("Borderless fullscreen",       12,  27.1,  1,  False, False, False),
    ("Sound physics / muffling",    10,  51.8,  5,  False, False, False),
    ("Chat heads / bubbles",         8,  50.4,  2,  False, False, False),
    ("Shulker box preview",          7,  33.7,  2,  False, False, False),
    ("Dynamic lights",               5,  18.1,  3,  False, False, False),
    ("Light-level overlay",          5,   5.7,  2,  False, False, False),
    ("Auto-reconnect",               4,   0.3,  1,  False, False, False),
]


def rank() -> None:
    """Breadth-first ranking: mods-weighted demand per unit of effort."""
    pool = [c for c in CANDIDATES if not c[5]]                 # not in Rewo yet
    max_mods = max(c[1] for c in pool)
    max_dl = max(c[2] for c in pool)

    def score(mods: int, dl_m: float, effort: int) -> float:
        # Normalise first -- mods spans 4..115 and downloads 0.3..125, so a raw
        # sum would silently hand the ranking to downloads.
        demand = MODS_WEIGHT * (mods / max_mods) + (1 - MODS_WEIGHT) * (dl_m / max_dl)
        return demand / effort * 100                            # /effort = breadth-first

    scored = sorted(
        ((score(m, d, e), n, m, d, e, port) for n, m, d, e, _fab, _rewo, port in pool),
        reverse=True,
    )

    print(f"breadth-first ranking  (MODS_WEIGHT={MODS_WEIGHT}, score = demand / effort)\n")
    print(f"  {'#':>2} {'score':>6} {'mods':>5} {'dl(M)':>7} {'eff':>3}  feature")
    print("  " + "-" * 62)
    for i, (s, n, m, d, e, port) in enumerate(scored, 1):
        tag = "  [port]" if port else ""
        print(f"  {i:>2} {s:>6.1f} {m:>5} {d:>7.1f} {e:>3}  {n}{tag}")

    grouped = [x for x in scored if x[5]]
    if grouped:
        tot_e = max(eff for *_, eff, _p in grouped)  # one milestone, not a sum
        print(f"\n  [port] = the single 'port the module + HUD set into Rewo' milestone")
        print(f"          {len(grouped)} rows, {sum(m for _s, _n, m, *_ in grouped)} mods, "
              f"{sum(d for _s, _n, _m, d, *_ in grouped):.0f}M downloads, effort ~{tot_e}.")
        print(f"          Ranked individually above; treat as ONE piece of work.")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--refresh", action="store_true", help="ignore the cache and re-fetch")
    ap.add_argument("--rank", action="store_true", help="rank candidate features, skip the survey tables")
    args = ap.parse_args()
    if args.rank:
        rank()
        return
    report(fetch(refresh=args.refresh))


if __name__ == "__main__":
    main()
