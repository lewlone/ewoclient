# ewo-hud — EwoClient in-game HUD (Fabric mod)

The Java half of EwoClient's in-game HUD. It pairs with the `ewo-jni` Rust
cdylib (`../crates/ewo-jni`), which paints `ewo-render`'s Skia UI over a
running Minecraft through a borrowed GL context. One jar serves Minecraft
**26.1 and 26.2**: `EwoMixinPlugin` picks the per-version mixins at load.

Design rules, schema versions and gotchas live in
`../.claude/rules/ewo-ingame-hud.md` and `../.claude/rules/ewo-modules.md`;
this file is the build-and-run pointer.

## Build

No Loom, no Gradle: the mods run in Minecraft's Mojmap namespace with no
remap step, so the mod is `javac` + `jar`, compiled by `build.ps1` against
`%APPDATA%\EwoClient\shared\versions\26.2\26.2.jar` with the JDK 25 at
`%APPDATA%\EwoClient\jdks\temurin-25` (output stays `--release 21`).

```powershell
./build.ps1              # legit build: cargo build -p ewo-jni, jar, deploy
./build.ps1 -Pvp         # adds the assist package and --features pvp
./build.ps1 -NoDeploy    # compile + jar only; no cargo build, nothing copied
```

Every build writes `build/ewo-hud-0.1.0.jar` and checks it: the legit jar fails
the build if any entry sits under `dev/lewlone/ewohud/assist/` or any entry's
bytes name that package; the pvp jar must contain `EwoAssistEntry`.

A deploying build (no `-NoDeploy`) copies the jar to
`%APPDATA%\EwoClient\shared\libraries\dev\lewlone\ewo-hud\0.1.0\` with
`target\debug\ewo_jni.dll` beside it, and to the dev drop
`%APPDATA%\EwoClient\dev\ewo-hud-0.1.0.jar`, which the loader manifests' `file://`
entry names. It fails if Minecraft still holds the dll open. Use `-NoDeploy`
in CI, audits and worktrees.

`python ../tools/check_mixin_targets.py` resolves every mixin target against
the 26.1 and 26.2 jars (CI runs it on the self-hosted runner).

Keep `build.ps1` ASCII-only: PowerShell 5.1 reads `.ps1` as ANSI.

## How the dll is found

`EwoHudMod` loads `ewo_jni.dll` at client init, in order: `-Dewo.jni.path`
(or the legacy `-Dewo.hud.nativePath`), beside the mod jar, the game
(instance) directory, then the newest cargo build under
`<user.home>\Desktop\EwoClientV3\target\` (release, unless debug is newer).

## Hooks

| Mixin | Target | Versions |
|---|---|---|
| `EwoHudMixin` | `RenderSystem.flipFrame`: once per frame, `nativeRender` | 26.1 |
| `EwoHudMixin262` | `Minecraft.renderFrame`, at `GpuSurface.present()` | 26.2 |
| `MinecraftShutdownMixin` | `Minecraft.destroy`: arms the exit watchdog | 26.1 |
| `MinecraftShutdownMixin262` | `Minecraft.exitWorldAndClose` (26.2 removed `destroy`) | 26.2 |
| `GuiCrosshairMixin` / `...262` | `Gui` / `Hud` `extractCrosshair` | 26.1 / 26.2 |
| `FireOverlayMixin` / `...262` | `ScreenEffectRenderer` `renderFire` / `submitFire` | 26.1 / 26.2 |
| the rest of `ewohud.mixins.json` | camera, input, lightmap, hurt/bob, entity status | both |

`EwoFrameHook` (called from the frame mixins) catches any `Throwable` from the
native call or the module tick and disables the native bridge after 5
consecutive failing frames, so a stale or broken dll cannot crash the render
loop. The pvp build adds `ewohud-pvp.mixins.json` and the `EwoAssistEntry`
client entrypoint (declared only in `fabric-pvp.mod.json`).

## Logs

- Rust side: `%TEMP%\ewo-jni.log` and the per-launch log under
  `%APPDATA%\EwoClient\instances\<id>\logs\`.
- Mod side: `[ewo-hud]` lines in the same per-launch log.
