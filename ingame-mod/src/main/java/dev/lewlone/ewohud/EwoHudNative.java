package dev.lewlone.ewohud;

import java.nio.ByteBuffer;

/**
 * JNI surface of the {@code ewo-jni} native library (Rust + Skia).
 *
 * <p>Phase E. The native methods are implemented in
 * {@code crates/ewo-jni/src/lib.rs}; the symbol names are derived from this
 * class's fully-qualified name, so neither side may be renamed independently.
 *
 * <p>All methods must be invoked on Minecraft's render thread.
 */
public final class EwoHudNative {
    private EwoHudNative() {}

    /** Liveness check: proves the cdylib loaded and JNI linkage works. */
    public static native void nativeHello();

    /**
     * Register the shared JVM&rarr;Rust data block. Called once at mod init.
     * Rust resolves the direct buffer's address and reads it every frame
     * thereafter with no further JNI marshaling.
     *
     * @param buffer a direct {@link ByteBuffer} ({@link EwoHudData#allocate()}).
     */
    public static native void nativeInit(ByteBuffer buffer);

    /** Paint + composite one HUD frame from the shared data block. */
    public static native void nativeRender();

    // ── Overlay input (E4) — forwarded only while the overlay is open ──────

    /** Cursor moved to `(x, y)` in window pixels. */
    public static native void nativeMouseMove(double x, double y);

    /** Mouse `button` pressed/released at `(x, y)` in window pixels. */
    public static native void nativeMouseButton(int button, boolean pressed, double x, double y);

    /** Scroll wheel moved by `(dx, dy)`. */
    public static native void nativeMouseScroll(double dx, double dy);

    /** Key `key` (GLFW code) pressed/released with `modifiers`. */
    public static native void nativeKey(int key, boolean pressed, int modifiers);

    // ── Modules (Phase G) ─────────────────────────────────────────────────

    /**
     * Register the Rust&rarr;JVM module-state block. Called once at mod init.
     * Rust writes it every frame; the mod reads module enabled/settings state
     * through the buffer with no further JNI marshaling.
     *
     * @param buffer a direct {@link ByteBuffer} ({@link EwoModuleData#allocate()}).
     */
    public static native void nativeInitModules(ByteBuffer buffer);

    /**
     * Flip module {@code index}'s enabled flag — driven by a module keybind.
     * Rust owns module state; the new value comes back through the buffer.
     */
    public static native void nativeModuleToggle(int index);
}
