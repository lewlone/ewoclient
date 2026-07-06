package dev.lewlone.ewohud;

import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;

import net.fabricmc.api.ClientModInitializer;

/**
 * Client entrypoint for the EwoClient in-game HUD (Phase E spike).
 *
 * <p>Loads the {@code ewo-jni} native library and pings it once. The actual
 * per-frame painting is driven from {@link dev.lewlone.ewohud.mixin.EwoHudMixin}
 * once {@link #nativeReady} is set.
 */
public final class EwoHudMod implements ClientModInitializer {

    /** True once the native library is loaded and {@code nativeHello()} returned. */
    public static volatile boolean nativeReady = false;

    @Override
    public void onInitializeClient() {
        Path dll = resolveNativeLibrary();
        if (dll == null) {
            System.err.println("[ewo-hud] could not locate ewo_jni.dll — HUD spike disabled. "
                    + "Set -Dewo.hud.nativePath=<path> or build crates/ewo-jni.");
            return;
        }
        try {
            System.load(dll.toAbsolutePath().toString());
            EwoHudNative.nativeHello();
            // Allocate the JVM->Rust HUD block + the Rust->JVM module block.
            EwoHudNative.nativeInit(EwoHudData.allocate());
            EwoHudNative.nativeInitModules(EwoModuleData.allocate());
            nativeReady = true;
            System.err.println("[ewo-hud] native bridge loaded: " + dll);
            armExitWatchdog();
        } catch (Throwable t) {
            System.err.println("[ewo-hud] failed to load native bridge: " + t);
            t.printStackTrace();
        }
    }

    /**
     * The JVM's shutdown can deadlock in native teardown on 26.2 —
     * DLL_PROCESS_DETACH under the Windows loader lock (the HUD's second GL
     * context + the WinRT SMTC media thread are the suspects) — leaving a
     * windowless zombie java.exe that holds the dll + instance files and
     * blocks the next launch. Vanilla's own shutdown watchdog exists for
     * exactly this but crashes on an internal bug before it can act.
     *
     * <p>So: a shutdown hook (runs when orderly JVM exit begins — world
     * saves are already done) arms a daemon timer. If the process is still
     * alive 4 seconds later, halt() is clearly stuck in native teardown and
     * {@link EwoHudNative#nativeForceExit} terminates the process outright.
     * On a clean exit the process dies first and the timer dies with it.
     * The hook itself returns immediately, so it never delays a good exit.
     */
    private static void armExitWatchdog() {
        Runtime.getRuntime().addShutdownHook(new Thread(() -> {
            Thread killer = new Thread(() -> {
                try {
                    Thread.sleep(4000);
                } catch (InterruptedException ignored) {
                    return;
                }
                System.err.println("[ewo-hud] exit watchdog: JVM halt is stuck in native teardown — terminating hard");
                EwoHudNative.nativeForceExit();
            }, "ewo-exit-watchdog");
            killer.setDaemon(true);
            killer.start();
        }, "ewo-exit-watchdog-arm"));
    }

    /**
     * Locate {@code ewo_jni.dll}: an explicit {@code -Dewo.hud.nativePath}
     * override first, then the EwoClientV3 cargo build outputs.
     */
    private static Path resolveNativeLibrary() {
        String override = System.getProperty("ewo.hud.nativePath");
        if (override != null && !override.isEmpty()) {
            Path p = Paths.get(override);
            if (Files.isRegularFile(p)) {
                return p;
            }
            System.err.println("[ewo-hud] ewo.hud.nativePath set but no file there: " + p);
        }

        Path target = Paths.get(System.getProperty("user.home", "."))
                .resolve("Desktop").resolve("EwoClientV3").resolve("target");
        for (String profile : new String[] { "debug", "release" }) {
            Path p = target.resolve(profile).resolve("ewo_jni.dll");
            if (Files.isRegularFile(p)) {
                return p;
            }
        }
        return null;
    }
}
