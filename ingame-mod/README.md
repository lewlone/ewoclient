# ewo-hud — EwoClient in-game HUD

The Fabric-side half of EwoClient's Phase E in-game HUD. Pairs with the
`ewo-jni` Rust crate (`../crates/ewo-jni`) to paint `ewo-render`'s Skia UI over
a running Minecraft through a borrowed GL context.

## What it does

`EwoHudMixin` injects at the head of `RenderSystem.flipFrame` (Minecraft's
buffer-swap call) and, once per frame, reads `Minecraft.getFps()` and calls
`EwoHudNative.nativeRender(fps)`. That JNI call drops into Rust, which paints
the HUD onto an offscreen Skia surface and composites it over the window
framebuffer — visible on the title screen, in menus and in-game alike.

`EwoHudMod` loads `ewo_jni.dll` at client init and pings it once
(`nativeHello`) as a liveness check.

## Pieces

| Piece | Where |
|---|---|
| Rust bridge (cdylib) | `../crates/ewo-jni` → `../target/debug/ewo_jni.dll` |
| Mod entrypoint | `EwoHudMod` — loads the dll |
| JNI surface | `EwoHudNative` — `nativeHello` / `nativeRender(int fps)` |
| Frame hook | `EwoHudMixin` — `@Mixin(RenderSystem.class)`, `flipFrame` HEAD |

## Build

No Loom, no Gradle. This toolchain runs mods in Minecraft's mapped namespace
with no remapping step, so the mod is plain `javac` + `jar`:

```powershell
./build.ps1        # → build/ewo-hud-0.1.0.jar
```

From E2 the mod reads Minecraft classes directly (`Minecraft.getFps()`,
`RenderSystem`, …), so the build classpath includes `26.1.jar` — which Mojang
ships Mojmap-named, directly compilable. That jar is Java-25 bytecode, so the
build needs a **JDK 25** at `%APPDATA%/EwoClient/jdks/temurin-25/` (see
`PHASE_E_PLAN.md` E2). Output stays `--release 21` so a Java-25 JVM runs it and
`ewohud.mixins.json` keeps `compatibilityLevel: JAVA_21`.

Keep `build.ps1` ASCII-only — PowerShell 5.1 reads `.ps1` as ANSI, so a stray
em-dash breaks the parser.

## Run

1. `cargo build -p ewo-jni` in the repo root → `target/debug/ewo_jni.dll`.
2. `./build.ps1` here → `build/ewo-hud-0.1.0.jar`.
3. Delete the staged copy at
   `%APPDATA%/EwoClient/shared/libraries/dev/lewlone/ewo-hud/0.1.0/ewo-hud-0.1.0.jar`
   so the launcher re-stages the fresh jar from the manifest's `file://` entry.
4. Launch Minecraft 26.1 through the EwoClient launcher.

`EwoHudMod` finds the dll automatically from
`<user.home>/Desktop/EwoClientV3/target/{debug,release}/ewo_jni.dll`, or from
an explicit `-Dewo.hud.nativePath=<path>`.

## Logs

- Rust side: `%TEMP%\ewo-jni.log` and the per-launch log under
  `%APPDATA%\EwoClient\instances/<name>/logs/`.
- Mod side: `[ewo-hud]` lines in the same per-launch log.
