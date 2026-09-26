package dev.lewlone.ewohud;

import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.concurrent.atomic.AtomicBoolean;

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

    /** Ensures the exit watchdog's kill-timer is started at most once, no
     *  matter how many arm points fire (the teardown mixin + shutdown hook). */
    private static final AtomicBoolean watchdogArmed = new AtomicBoolean(false);

    /** How long an in-progress JVM teardown may run before it's declared
     *  stuck and hard-terminated. A clean exit finishes (and the process
     *  dies, killing this daemon timer) well inside this window; normal
     *  post-{@code destroy()} shutdown is sub-second. */
    private static final long WATCHDOG_GRACE_MS = 3000;

    @Override
    public void onInitializeClient() {
        Path dll = resolveNativeLibrary();
        if (dll == null) {
            System.err.println("[ewo-hud] could not locate ewo_jni.dll — HUD spike disabled. "
                    + "Set -Dewo.jni.path=<path> or build crates/ewo-jni.");
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
            // Fallback arm point: fires when the JVM begins orderly shutdown.
            // The primary arm is MinecraftShutdownMixin (destroy() RETURN),
            // which runs earlier — but a hang on an exit path that skips
            // destroy() still gets caught here.
            Runtime.getRuntime().addShutdownHook(
                    new Thread(EwoHudMod::armExitWatchdog, "ewo-exit-watchdog-arm"));
        } catch (Throwable t) {
            System.err.println("[ewo-hud] failed to load native bridge: " + t);
            t.printStackTrace();
        }
    }

    /**
     * Arm the exit watchdog: a daemon timer that hard-terminates the process
     * if JVM teardown gets stuck.
     *
     * <p>The JVM's shutdown can deadlock in native teardown —
     * DLL_PROCESS_DETACH under the Windows loader lock (the HUD's second GL
     * context + the WinRT SMTC media thread are the suspects) — leaving a
     * windowless zombie java.exe that holds the dll + instance files and
     * blocks the next launch. Vanilla's own shutdown watchdog exists for
     * exactly this but crashes on an internal bug before it can act.
     *
     * <p>The timer sleeps {@link #WATCHDOG_GRACE_MS}; if the process is still
     * alive when it wakes, teardown is stuck and {@link EwoHudNative#nativeForceExit}
     * ({@code TerminateProcess}, which skips DLL detach) kills it outright. On
     * a clean exit the process dies first and the daemon dies with it, so a
     * healthy shutdown is never interrupted.
     *
     * <p>Idempotent and safe to call from multiple arm points — the earliest
     * caller wins. Arm points, earliest first:
     * <ol>
     *   <li>{@link dev.lewlone.ewohud.mixin.MinecraftShutdownMixin} — at
     *       {@code Minecraft.destroy()} RETURN, the game's final teardown,
     *       just before the JVM's own native shutdown begins.</li>
     *   <li>A JVM shutdown hook (see {@link #onInitializeClient}) — the
     *       fallback for any exit path that doesn't run {@code destroy()}.</li>
     * </ol>
     * The launcher-side reaper ({@code launch::reaper}) is the final backstop
     * if the process still lingers.
     */
    public static void armExitWatchdog() {
        if (!nativeReady) {
            return; // no native kill path available — nothing to arm
        }
        if (!watchdogArmed.compareAndSet(false, true)) {
            return; // already armed by an earlier point
        }
        Thread killer = new Thread(() -> {
            try {
                Thread.sleep(WATCHDOG_GRACE_MS);
            } catch (InterruptedException ignored) {
                return;
            }
            System.err.println("[ewo-hud] exit watchdog: JVM teardown stuck after "
                    + WATCHDOG_GRACE_MS + "ms — terminating hard");
            EwoHudNative.nativeForceExit();
        }, "ewo-exit-watchdog");
        killer.setDaemon(true);
        killer.start();
    }

    private static final String DLL_NAME = "ewo_jni.dll";

    /**
     * Locate {@code ewo_jni.dll}, in order:
     * <ol>
     *   <li>an explicit {@code -Dewo.jni.path} (or legacy {@code -Dewo.hud.nativePath});</li>
     *   <li>a dll next to the mod jar, then in the game (instance) dir;</li>
     *   <li>the EwoClientV3 cargo outputs — release before debug, except that a
     *       debug build newer than the release build wins (the active dev loop).</li>
     * </ol>
     */
    private static Path resolveNativeLibrary() {
        for (String prop : new String[] { "ewo.jni.path", "ewo.hud.nativePath" }) {
            String override = System.getProperty(prop);
            if (override != null && !override.isEmpty()) {
                Path p = Paths.get(override);
                if (Files.isRegularFile(p)) {
                    return found(p, "-D" + prop);
                }
                System.err.println("[ewo-hud] " + prop + " set but no file there: " + p);
            }
        }

        try {
            Path jar = Paths.get(EwoHudMod.class.getProtectionDomain().getCodeSource()
                    .getLocation().toURI());
            Path beside = (Files.isDirectory(jar) ? jar : jar.getParent()).resolve(DLL_NAME);
            if (Files.isRegularFile(beside)) {
                return found(beside, "beside the mod jar");
            }
        } catch (Exception ignored) {
            // No usable code source (e.g. a dev classpath) — fall through.
        }
        Path gameDir = Paths.get(System.getProperty("user.dir", ".")).resolve(DLL_NAME);
        if (Files.isRegularFile(gameDir)) {
            return found(gameDir, "game dir");
        }

        Path target = Paths.get(System.getProperty("user.home", "."))
                .resolve("Desktop").resolve("EwoClientV3").resolve("target");
        Path release = target.resolve("release").resolve(DLL_NAME);
        Path debug = target.resolve("debug").resolve(DLL_NAME);
        boolean hasRelease = Files.isRegularFile(release);
        boolean hasDebug = Files.isRegularFile(debug);
        if (hasRelease && hasDebug) {
            return mtime(debug) > mtime(release)
                    ? found(debug, "cargo debug (newer than release)")
                    : found(release, "cargo release");
        }
        if (hasRelease) {
            return found(release, "cargo release");
        }
        if (hasDebug) {
            return found(debug, "cargo debug");
        }
        return null;
    }

    private static Path found(Path p, String how) {
        System.err.println("[ewo-hud] using " + DLL_NAME + " from " + how + ": " + p);
        return p;
    }

    private static long mtime(Path p) {
        try {
            return Files.getLastModifiedTime(p).toMillis();
        } catch (Exception e) {
            return 0L;
        }
    }
}
