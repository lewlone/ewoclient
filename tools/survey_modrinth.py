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


# --------------------------------------------------------------- classification

# Problem classes. A/B/C exist only because the game is a JVM client with a mod
# loader -- Rewo dissolves them by construction. D is Rewo's actual roadmap.
#
# **Suffix-safe patterns only.** An earlier revision wrapped the whole
# alternation in \b(...)\b, which made `optimi[sz]` require a word boundary
# right after "optimiz" -- so it never matched "optimization", and Lithium was
# filed as a QoL feature. The same trap hit `librar` vs "library" (so YACL and
# half the library ecosystem escaped class C) and `skybox` vs "skyboxes". It
# understated the JVM-tax share by 15 points. Put a `\w*` tail on any stem;
# never leave a trailing \b after one.
#
# Applied DISJOINTLY, infrastructure first, so a library that also says
# "performance" lands in C once rather than being counted twice.
CLASSES = {
    "C. Modding infrastructure": (
        r"\b(librar\w*|\bapi\b|config\w* (librar\w*|system|api)|core ?mod|mod ?menu|mod loader"
        r"|modding|kotlin|placeholder|mixin\w*|crash report|stack ?trace|framework|scripting"
        r"|javascript|development aid|\bLib\b|utilit\w* (mod|set)|for (mod|modpack) (dev|author|pack)"
        r"|pre-?generat\w*)"
    ),
    "A. JVM/render performance": (
        r"\b(fps|frame ?rate|framerate|performance|optimi\w*|memory usage|ram usage|lag\w*"
        r"|stutter|cull\w*|faster|speed ?up|load ?time|thread (schedul|tweak|pool)"
        r"|model gap|z-?fight\w*|packet\w* (fix|problem)|reversed dns|rendering engine)"
    ),
    "B. OptiFine-pack parity":   r"\b(optifine|mcpatcher|connected (block )?texture\w*|custom entity (model|texture)\w*|emissive|\bcem\b|\bcit\b|animated texture\w*|custom gui|skybox\w*|colormap\w*|shader ?pack\w*)",
}

# An "addon for X" is ecosystem, not a feature. This matters more than it looks:
# 51 of 54 item/recipe-viewer matches are JEI/EMI/REI *plugins* whose whole
# purpose is surfacing OTHER MODS' recipes. Rewo speaks the vanilla protocol --
# there are no modded recipes to integrate -- so the viewer ports and its entire
# addon tail does not. Counting them trebles that cluster's apparent size.
_HOSTS = (r"JEI|EMI|REI|Sodium|Iris|Xaero\w*|Create|Cobblemon|Litematica|ReplayMod"
          r"|Flashback|JourneyMap|MouseTweaks|Tweakeroo|MiniHUD|Masa|Figura|Emotecraft"
          r"|Distant Horizons|WorldEdit|Carpet|KubeJS|GeckoLib|YACL|Controlify")

# An "addon for X" is ecosystem, not a feature. This matters more than it looks:
# 51 of 54 item/recipe-viewer matches are JEI/EMI/REI *plugins* whose whole
# purpose is surfacing OTHER MODS' recipes. Rewo speaks the vanilla protocol --
# there are no modded recipes to integrate -- so the viewer ports and its entire
# addon tail does not. Counting them trebles that cluster's apparent size.
#
# The three trailing alternatives were added after sampling the COVERED set
# found leaks the first version missed: a title prefix ("ReplayMod-Pose-Fix"),
# a verb phrase ("Let Litematica be able to ..."), and a bare fix-for.
ADDON = re.compile(
    r"(addon|add-on|plugin|extension|compat\w*) (for|to|of)\b"
    r"|\b(" + _HOSTS + r") (addon|add-on|plugin|extension|compat)"
    r"|(menu|options|settings|config\w*) (for|to) (" + _HOSTS + r")"
    r"|^(EMI|JEI|REI) "
    r"|^(" + _HOSTS + r")[- ]"
    r"|\b(let|lets|allows?|enables?|makes?|for) (" + _HOSTS + r")\b"
    r"|\b(fix|patch|tweak)\w*\s+(for|to)\s+(" + _HOSTS + r")\b",
    re.I,
)

# Mods written for one server network or one other mod's content are real work
# but cannot port to a vanilla-protocol client. Sampling the covered set found
# these leaking into feature clusters (Cobblemon held-items into "Held-item
# info", WynnCompare into "Tooltip overhaul", a cape fix "for the Modern Beta
# SMP" into "Capes").
SERVER_SPECIFIC = re.compile(
    r"\b(hypixel|skyblock|skyhanni|wynncraft|wynn\w*|cobblemon|pixelmon|mcc ?island"
    r"|mineplex|hive|origins|tiertagger|pvptiers|oce ?net)\b"
    r"|\bfor (the )?[\w' ]{0,20}(SMP|network|realm)\b",
    re.I,
)

# ----------------------------------------------------------------- feature demand

# One entry per DISTINCT feature -- the unit is "a problem someone solved", not
# "a mod". The count of independent reimplementations, not download total, is
# the primary signal: a feature 100 authors rebuilt is an itch nobody has
# definitively scratched, while a feature with huge downloads and few mods
# already has a winner and is a "ship it, don't innovate" item.
#
# Measured only within class D, after the addon filter. Both guards are load
# bearing: ModernFix ("improves performance ... fixes many bugs") was inflating
# `Vanilla bug fixes` by 69M until classes were applied first.
FEATURES = {
    # -- interface / information
    "Item & recipe viewer":       r"(item and recipe|recipe viewer|view items and recipes|item viewer|roughly enough items|just enough items)",
    # AppleSkin (78M) is the largest single feature the first 55-entry revision
    # missed entirely -- bigger than the item/recipe viewer. "food/hunger HUD"
    # reads like content until you notice it is a *readout*, not a food item.
    "Food / hunger HUD":          r"(food|hunger|saturation|nutrition).{0,30}(hud|bar|display|overlay|info|indicat)|appleskin|hunger.?related",
    "Tooltip overhaul":           r"tooltip",
    "World list / save QoL":      r"world (list|selection|screen|preview)|favou?rit\w*.{0,20}world|pin\w*.{0,15}world|backup (screen|prompt)|cherished",
    "IME / input method":         r"input method|\bIME\b|ingameime|imblocker|(chinese|korean|japanese) (input|chat)",
    # -- found by random-sampling the unread tail (see REWO_FEATURE_SURVEY.md
    #    §1); the hand-authored list missed all of these.
    "Accessibility":              r"accessibility (screen|option|setting|feature)|colou?r ?blind|\bdeaf\b|subtitle|narrat\w*|high contrast|sticky ?key|screen ?reader|text shadow|gui scal\w*",
    "Account switching":          r"account switch\w*|switch\w*.{0,20}account|in-?game account|re-?validate your session|auth ?me|offline (and|or) (microsoft|online) account",
    "Ghost block resync":         r"ghost (block|item)|anti ?ghost|re-?updates your inventory|desync\w*",
    "Confirmation prompts":       r"confirm\w*.{0,30}(before|dialog|screen|prompt)|are you sure|think twice|accident\w*.{0,25}(disconnect|drop|delete|reset)",
    "Window appearance":          r"(window|title ?bar) (title|icon|appearance|colou?r|customi[sz])|dark ?title ?bar|\bmica\b|acrylic|fluent design|taskbar",
    "Container info overlays":    r"(furnace|villager|trade|beehive|bee ?nest|fuel|brewing|horse).{0,25}(info|hud|overlay|display|stat)|offers ?hud",
    "Font / text rendering":      r"truetype|opentype|font (support|render|custom)|custom font|text render\w*",
    "Elytra / flight HUD":        r"(elytra|flight).{0,25}(hud|pitch|assist|stat|indicator)|flight style hud",
    "Motion blur":                r"motion ?blur|frame blend\w*",
    "Item durability display":    r"durabilit",
    "Held-item / enchant info":   r"held item|item info|enchantment description|enchantment lore",
    "Shulker box preview":        r"shulker (box )?(tooltip|preview|content)",
    "Inventory sorting":          r"inventor\w* (sort|manage|profile)|sort\w* (your |the )?inventor|quick ?stack|inventory tweak",
    "Status-effect timers":       r"status effect|potion (counter|display|effect)|effect (bar|timer|insight)",
    "Player / mob health bars":   r"health ?(bar|indicator|display)|hp ?bar|mob plaque|entity (health|status)|expand health|overflowing bar",
    "Armor HUD":                  r"armou?r ?(hud|bar)|armou?r durabilit",
    "Ping display":               r"\bping\b.{0,30}(display|view|numeric|tab)|display.{0,20}\bping\b",
    "Keystrokes display":         r"keystroke|key ?display",
    "Coords / FPS readout":       r"coordinates? display|fps ?(display|hud|counter)|show ?fps|custom ?hud|info overlay",
    "Mount / boat HUD":           r"mount hud|riding|boat (hud|view)|horse stat",
    "Advancements screen":        r"advancement",
    "Scoreboard / tab list":      r"scoreboard|tab ?list|player ?list",
    "Debug / F3 overlay":         r"\bf3\b|debug (hud|screen|overlay)|pie ?chart",
    # -- world-space
    "Minimap / waypoints":        r"minimap|mini[- ]map|waypoint|world ?map",
    "Extended render distance":   r"render distance (greater|beyond|past)|view[- ]distance.{0,25}(server|beyond)|chunk (cach\w*|unload delay)|hold that chunk|\bbobby\b",
    "Schematics":                 r"litematica|schematic|worldedit cui",
    "Light-level overlay":        r"light ?(level )?overlay|spawn\w* overlay|light overlay",
    "Block info under crosshair": r"what am i looking at|\\bwaila\\b|\\bjade\\b|\\bhwyla\\b|block under (your |the )?crosshair|block info|looking at.{0,20}(block|entity).{0,20}(info|hud)|harvest\\w* (tool|level) (info|display)",
    "Block highlight / outline":  r"block (highlight|overlay|outline)|highlight\w*.{0,30}(block|item|visual)|selection box|visuali[sz]ation of specific block",
    "Dynamic lights":             r"dynamic ?light|lambdyn",
    # -- camera / view
    "Zoom":                       r"\bzoom",
    "Freecam / freelook":         r"freecam|free ?look|detach\w* camera",
    "Shoulder / 3rd-person cam":  r"third[- ]person|shoulder surf|over[- ]the[- ]shoulder",
    "Fullbright / gamma":         r"fullbright|full bright|\bgamma\b|night ?vision|brightness (beyond|control|plus)",
    "Custom crosshair":           r"(custom|dynamic|centered|configurable|vanilla) crosshair|crosshair (mod|tweak|addon|customi[sz]|render|overlay|style)|change\\w*.{0,15}crosshair|crosshair on reach",
    "View-bob / hurt-cam":        r"view ?bob|hand ?bob|screen ?bob|hurt ?cam|damage tilt",
    "Fog control":                r"\bno fog\b|fog (control|override|remove)|clear water|void fog",
    "Overlay removal (fire/etc)": r"(fire|pumpkin|water|spyglass) overlay|low fire|no fire",
    # -- input
    "Keybind search & manage":    r"key ?bind\w* (menu|screen|manage|conflict|fix|search|setup|purge)|search bar to the key|multiple keybinding|modifier key|rebind\w*|remap\w* (key|control)",
    "Controller support":         r"controller support|gamepad|touch control|\bcontrolify\b",
    "Toggle sprint / sneak":      r"toggle ?(sprint|sneak)|auto ?sprint",
    "Walk while in inventory":    r"walk around while|move while.{0,20}(inventor|screen|gui)|invmove",
    "FOV control":                r"\bfov\b|field of view|zoom-?free fov|dynamic fov",
    "Death recap / waypoint":     r"death (log|recap|point|marker|waypoint|history)|where you died|track\w* .{0,20}death|deaths? (coordinate|location)",
    "Inventory slot locking":     r"(lock|reserve)\w* .{0,15}(slot|inventory)|slot lock|reserved slot|locked slot",
    "Recipe book QoL":            r"recipe book",
    "World downloader":           r"world downloader|download\w* .{0,20}world|save .{0,15}server world",
    "Window focus behaviour":     r"minimi[sz]e.{0,25}(focus|fullscreen)|focus loss|alt.?tab",
    "Block placement helper":     r"block placement|bridging|reach[- ]?around|smart\\w* block placement|pro placer|accurate block break",
    "Reach / hit indicators":     r"hit ?(range|indicator|colou?r|reg)|reach|attack indicator|hitbox",
    # -- social / session
    "Chat QoL":                   r"chat ?(patch|tweak|tool|plus|log|histor|tab|timestamp|filter|search)|timestamp.{0,20}chat|compact chat",
    "Chat heads / bubbles":       r"chat ?head|talk ?(bubble|balloon)|chat ?bubble",
    "Capes / cosmetics":          r"\bcape(s)?\b|cosmetic",
    "Custom skin loading":        r"skin ?(loader|server|override|switch)|custom skin|hd skin",
    "Discord rich presence":      r"discord|rich presence",
    "Server list QoL":            r"server list|multiplayer (menu|screen)|add server|server country|server ?browser",
    "Auto-reconnect":             r"auto[- ]?reconnect",
    # -- session / shell
    "Loading / menu flow":        r"loading screen|splash screen|title screen|main menu|reloading screen|fast ?quit|tips to loading|loading (menu|tip)",
    "Options preservation":       r"options shall be respected|default (config|option)|configured default|preserve.{0,15}option",
    "Resource-pack management":   r"resource ?pack\w*.{0,25}(manag|overrid|organi[sz]|browser|updater|folder|profile|selection|load)|in-game resource pack|pack (browser|updater|organizer)|datapack load",
    "Borderless fullscreen":      r"borderless|windowed fullscreen",
    "Screenshot tooling":         r"screenshot|panorama|isometric render|photo mode|picture mode",
    "Replay / recording":         r"replay ?mod|record\w*.{0,20}(gameplay|your|session)|flashback",
    "Vanilla bug fixes":          r"fix\w*.{0,25}(minecraft )?bug|bug ?tracker|\bMC-\d+|vanillafix",
    "UI polish (scroll/anim)":    r"smooth scroll|scroll\w* (smooth|speed)|blur effect|menu blur|skip transition|chat animation|smooth chat|message\w*.{0,20}animation|hotbar animation|immersive hotbar",
    # -- audio (BLOCKED: Rewo has no audio subsystem at all)
    "Sound physics / muffling":   r"sound (physic|muffl|tweak)|audio (improve|engine|tweak)|reverb",
    "Music control":              r"music (control|delay|player|notification)|now playing|constant music|endless music",
    "Ambient sounds":             r"ambient ?sound|ambience|listentonature|soundscape",
    # -- atmosphere (cosmetic; ranked separately from QoL)
    "Ambient particles":          r"(particle|firefl|falling leaves|snow ?fall).{0,40}(effect|ambien|visual)|add(s|ing)? .{0,30}particle|prettier particle|new particles|particle (effect|improvement)",
    "Pickup notifications":       r"pick ?up notif|notif\w*.{0,20}collect|item pickup",
}
def _validate_patterns() -> None:
    """Fail loud on a corrupted or uncompilable pattern.

    Scripted edits to this file have twice collapsed a `\\b` escape into a
    literal 0x08 backspace, which silently changes what a pattern matches
    instead of raising. Control characters are never intentional here.
    """
    for table in (CLASSES, FEATURES):
        for name, pat in table.items():
            if any(ord(c) < 32 for c in pat):
                raise SystemExit(f"pattern for {name!r} contains a control character")
            re.compile(pat, re.I)


_validate_patterns()


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

    # Features are measured INSIDE class D only, minus addons. Both guards are
    # load bearing -- see the FEATURES comment. `rest` is already class D.
    pool = [m for m in rest
            if not ADDON.search(text(m)) and not SERVER_SPECIFIC.search(text(m))]
    addons = len(rest) - len(pool)
    print(f"\nfeature demand (independent reimplementations), measured within "
          f"class D minus {addons} addons -> {len(pool):,} mods")
    print(f"  {'mods':>5} {'dl(M)':>8}  feature")
    rows = []
    covered: set[str] = set()
    for name, pat in FEATURES.items():
        rx = re.compile(pat, re.I)
        hit = [m for m in pool if rx.search(text(m))]
        covered.update(m["slug"] for m in hit)
        rows.append((len(hit), sum(m["downloads"] for m in hit) / 1e6, name))
    for n, dl, name in sorted(rows, reverse=True):
        print(f"  {n:>5} {dl:>8.1f}  {name}")

    # How much of class D these distinct features actually explain. The
    # uncovered remainder is the genuine long tail of one-off gameplay and
    # cosmetic tweaks -- thousands of mods, individually tiny, none portable.
    unc = [m for m in pool if m["slug"] not in covered]
    print(f"\n  {len(FEATURES)} distinct features cover {len(covered):,}/{len(pool):,} "
          f"class-D mods ({len(covered)/len(pool)*100:.1f}%), "
          f"{sum(m['downloads'] for m in pool if m['slug'] in covered)/total_dl*100:.1f}% of all downloads")
    print(f"  uncovered long tail: {len(unc):,} mods, "
          f"{sum(m['downloads'] for m in unc)/1e6:,.0f}M "
          f"(median {sorted(m['downloads'] for m in unc)[len(unc)//2]:,} downloads)")

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
# One row per distinct feature, with `mods`/`dl_M` taken from the measured
# output of `--survey` (do NOT hand-edit those two columns; re-run and copy).
#
# kind:
#   qol    -- build it. The Rewo roadmap.
#   port   -- exists in the Fabric client (ewo-jni modules / HUD widgets) but
#             NOT in the rewo-* crates. Scored individually these flood the
#             table; in practice they are ONE milestone.
#   atmos  -- cosmetic/ambience. Real demand, but it competes on taste rather
#             than utility, so it is ranked separately instead of against
#             tooltips.
#   audio  -- BLOCKED. Rewo has no audio subsystem at all (rewo-world/src/
#             entities.rs:940 says so). 117M of demand sitting behind one
#             prerequisite that is a subsystem, not a feature.
#   parity -- Rewo already implements VANILLA's version; the mod cluster is
#             about exceeding it. Real work, but not greenfield, so the effort
#             number means "beyond parity" and the demand is partly satisfied
#             already. Audit these against the crates before scheduling -- the
#             first revision of this table had five of them marked `qol` and
#             two of those shipped (M40 tooltips, M41 durability bars) before
#             the survey was even written.
#   have   -- already in the rewo-* crates, nothing meaningful left.
#   na     -- does not port. See the per-row note.
#
# effort: 1 (a weekend) .. 5 (a milestone)
# Disposition per feature: (effort, kind). `mods` and `dl` are NOT stored here
# -- they are measured live from the survey by `measure()` and keyed on the
# FEATURES name, so this table cannot go stale when a pattern is widened. Every
# name here must exist in FEATURES; rank() asserts it.
#
# kind:
#   qol    -- build it. The Rewo roadmap.
#   port   -- exists in the Fabric client (ewo-jni modules / HUD widgets) but
#             NOT in the rewo-* crates. Scored individually these flood the
#             table; in practice they are ONE milestone.
#   atmos  -- cosmetic/ambience. Real demand, but competes on taste rather than
#             utility, so ranked separately instead of against tooltips.
#   audio  -- BLOCKED. Rewo has no audio subsystem at all (see
#             rewo-world/src/entities.rs:940). ~117M of demand behind one
#             prerequisite that is a subsystem, not a feature.
#   parity -- Rewo already implements VANILLA's version; the mod cluster is
#             about exceeding it. Real work, but not greenfield, so the effort
#             number means "beyond parity" and the demand is partly satisfied
#             already. Audit these against the crates before scheduling -- the
#             first revision of this table had five of them marked `qol` and
#             two of those shipped (M40 tooltips, M41 durability bars) before
#             the survey was even written.
#   have   -- already in the rewo-* crates, nothing meaningful left.
#   na     -- does not port; see the note.
#
# effort: 1 (a weekend) .. 5 (a milestone)
DISPOSITION = {
    "Tooltip overhaul":           (2, "parity"),  # M40/41/42 tooltip sprites + layout, container.rs:230
    "Item durability display":    (1, "parity"),  # M41 bar_width/bar_color, rewo-gpu/src/container.rs:409
    "Screenshot tooling":         (2, "parity"),  # M51a/b capture + F2, rewo-app/src/capture.rs
    "Reach / hit indicators":     (2, "port"),
    "Debug / F3 overlay":         (1, "have"),   # rewo-gpu/src/overlay.rs
    "Minimap / waypoints":        (5, "qol"),
    "Extended render distance":   (3, "qol"),    # Bobby (LGPL) -- rewo-world owns the column store
    "Loading / menu flow":        (4, "qol"),
    "Scoreboard / tab list":      (2, "qol"),
    "Zoom":                       (1, "port"),
    "Custom crosshair":           (2, "parity"),  # vanilla crosshair drawn, rewo-gpu/src/hud.rs:36
    "Fullbright / gamma":         (1, "port"),
    "Player / mob health bars":   (2, "qol"),
    "Chat QoL":                   (3, "qol"),
    "Capes / cosmetics":          (2, "qol"),
    "Armor HUD":                  (1, "port"),
    "Vanilla bug fixes":          (1, "na"),     # reimplemented from spec: most simply don't exist
    "Ambient particles":          (2, "atmos"),  # M37 shipped the particle system
    "Shoulder / 3rd-person cam":  (2, "qol"),
    "Schematics":                 (5, "qol"),
    "Toggle sprint / sneak":      (1, "port"),
    "Inventory sorting":          (4, "qol"),
    "Block highlight / outline":  (1, "parity"),  # selection outline, rewo-gpu/src/world.rs:449
    "Block info under crosshair": (2, "qol"),   # Jade/WAILA class    # M-targeting draws the outline; config is the gap
    "Status-effect timers":       (2, "qol"),
    "Discord rich presence":      (1, "qol"),    # tension with OFFLINE FIRST -- opt-in only
    "Freecam / freelook":         (3, "port"),
    "Advancements screen":        (3, "qol"),
    "Held-item / enchant info":   (2, "qol"),
    "Music control":              (5, "audio"),
    "Resource-pack management":   (3, "qol"),
    "Keybind search & manage":    (2, "qol"),
    "Replay / recording":         (5, "qol"),
    "Mount / boat HUD":           (2, "qol"),
    "Ping display":               (1, "port"),
    "Server list QoL":            (3, "na"),     # the server list lives in the launcher, not Rewo
    "Keystrokes display":         (1, "port"),
    "Overlay removal (fire/etc)": (1, "port"),
    "Ambient sounds":             (5, "audio"),
    "World list / save QoL":      (2, "qol"),
    "Block placement helper":     (3, "qol"),
    "Coords / FPS readout":       (1, "port"),
    "Sound physics / muffling":   (5, "audio"),
    "Food / hunger HUD":          (1, "qol"),    # AppleSkin, 82M -- the largest single miss of the first revision
    "Borderless fullscreen":      (1, "qol"),
    "Fog control":                (1, "qol"),
    "View-bob / hurt-cam":        (1, "port"),
    "Chat heads / bubbles":       (2, "qol"),
    "Pickup notifications":       (1, "qol"),
    "UI polish (scroll/anim)":    (2, "qol"),
    "Shulker box preview":        (2, "qol"),
    "Custom skin loading":        (2, "qol"),
    "IME / input method":         (3, "qol"),    # also the accessibility/i18n seam
    "Controller support":         (4, "qol"),
    "Light-level overlay":        (2, "qol"),
    "Item & recipe viewer":       (4, "qol"),
    "Options preservation":       (1, "na"),     # Rewo owns its config; no vanilla options to preserve
    "Dynamic lights":             (3, "qol"),
    "Auto-reconnect":             (1, "qol"),
    "Walk while in inventory":    (1, "qol"),
    "FOV control":                (1, "qol"),
    "Death recap / waypoint":     (2, "qol"),
    "Inventory slot locking":     (2, "qol"),
    "Recipe book QoL":            (3, "qol"),
    "World downloader":           (4, "qol"),
    "Window focus behaviour":     (1, "qol"),
    # -- added after random-sampling the unread tail; see §1 of the doc.
    "Accessibility":              (2, "qol"),   # narrator/subtitles/colourblind/sticky keys/GUI scale
    "Account switching":          (2, "qol"),   # Rewo already owns the auth chain via the launcher
    "Confirmation prompts":       (1, "qol"),
    "Window appearance":          (1, "qol"),   # title bar, icon, Win11 Mica -- window layer, not renderer
    "Container info overlays":    (2, "qol"),   # furnace/villager/beehive readouts
    "Elytra / flight HUD":        (2, "qol"),
    "Ghost block resync":         (2, "qol"),   # rewo-net owns the container/state sync already
    "Font / text rendering":      (3, "qol"),
    "Motion blur":                (2, "atmos"),
}


def measure(mods: list[dict]) -> dict[str, tuple[int, float]]:
    """Feature -> (independent mods, downloads in M), measured the same way
    report() does: disjoint classes first, then the addon filter, then
    features within class D only."""
    live = alive(mods)
    seen: set[str] = set()
    for pat in CLASSES.values():
        rx = re.compile(pat, re.I)
        seen.update(m["slug"] for m in live if rx.search(text(m)))
    pool = [m for m in live
            if m["slug"] not in seen and not ADDON.search(text(m))
            and not SERVER_SPECIFIC.search(text(m))]
    out = {}
    for name, pat in FEATURES.items():
        rx = re.compile(pat, re.I)
        hit = [m for m in pool if rx.search(text(m))]
        out[name] = (len(hit), sum(m["downloads"] for m in hit) / 1e6)
    return out


def rank() -> None:
    """Breadth-first ranking: mods-weighted demand per unit of effort."""
    stats = measure(fetch())

    missing = set(DISPOSITION) - set(stats)
    if missing:                       # fail loud rather than silently drop a row
        raise SystemExit(f"DISPOSITION names not in FEATURES: {sorted(missing)}")
    untriaged = set(stats) - set(DISPOSITION)
    if untriaged:
        raise SystemExit(f"FEATURES with no disposition: {sorted(untriaged)}")

    rows = [(n, stats[n][0], stats[n][1], e, k) for n, (e, k) in DISPOSITION.items()]
    buildable = [r for r in rows if r[4] in ("qol", "port", "atmos", "audio", "parity")]
    max_mods = max(r[1] for r in buildable)
    max_dl = max(r[2] for r in buildable)

    def score(mods_n: int, dl_m: float, effort: int) -> float:
        # Normalise first -- the two spans differ by an order of magnitude, so a
        # raw sum would silently hand the ranking to downloads.
        demand = MODS_WEIGHT * (mods_n / max_mods) + (1 - MODS_WEIGHT) * (dl_m / max_dl)
        return demand / effort * 100                      # /effort = breadth-first

    def table(kinds, title: str) -> None:
        sel = sorted(((score(m, d, e), n, m, d, e, k)
                      for n, m, d, e, k in buildable if k in kinds), reverse=True)
        if not sel:
            return
        print(f"\n{title}")
        print(f"  {'#':>2} {'score':>6} {'mods':>5} {'dl(M)':>6} {'eff':>3}  feature")
        print("  " + "-" * 58)
        for i, (s, n, m, d, e, k) in enumerate(sel, 1):
            print(f"  {i:>2} {s:>6.1f} {m:>5} {d:>6.1f} {e:>3}  {n}"
                  f"{'  [port]' if k == 'port' else ''}")

    print(f"breadth-first ranking  (MODS_WEIGHT={MODS_WEIGHT}, score = demand / effort)")
    table({"qol", "port"}, "QoL + features  ([port] rows are ONE milestone, see below)")
    table({"parity"}, "Vanilla parity already shipped -- these rank the work BEYOND it")
    table({"atmos"}, "Atmosphere (cosmetic -- competes on taste, ranked apart)")
    table({"audio"}, "Audio (BLOCKED: Rewo has no audio subsystem)")

    ports = [r for r in rows if r[4] == "port"]
    print(f"\n[port] = the single 'port the module + HUD set into Rewo' milestone:")
    print(f"        {len(ports)} features, {sum(r[1] for r in ports)} mods, "
          f"{sum(r[2] for r in ports):.0f}M downloads, effort ~3 as one piece of work.")
    for kind, label in (("have", "already in the rewo-* crates"), ("na", "does not port")):
        sel = [r for r in rows if r[4] == kind]
        if sel:
            print(f"\n{label}: " + ", ".join(f"{r[0]} ({r[1]} mods, {r[2]:.0f}M)" for r in sel))
    qol = [r for r in rows if r[4] == "qol"]
    print(f"\ntotals: {len(rows)} distinct features = {len(qol)} qol + {len(ports)} port "
          f"+ {sum(1 for r in rows if r[4]=='parity')} parity "
          f"+ {sum(1 for r in rows if r[4]=='audio')} audio-blocked "
          f"+ {sum(1 for r in rows if r[4]=='atmos')} atmosphere "
          f"+ {sum(1 for r in rows if r[4]=='have')} have "
          f"+ {sum(1 for r in rows if r[4]=='na')} n/a")


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
