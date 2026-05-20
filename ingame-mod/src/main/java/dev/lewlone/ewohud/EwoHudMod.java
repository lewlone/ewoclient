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
            // Allocate the shared JVM->Rust data block and register it once.
            EwoHudNative.nativeInit(EwoHudData.allocate());
            nativeReady = true;
            System.err.println("[ewo-hud] native bridge loaded: " + dll);
        } catch (Throwable t) {
            System.err.println("[ewo-hud] failed to load native bridge: " + t);
            t.printStackTrace();
        }
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
