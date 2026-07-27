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

    /**
     * Quick-edit gate — `true` while the modifier is held over a cursor-free
     * vanilla screen (inventory, pause, chat), so HUD widgets can be dragged
     * and resized without opening the EwoClient overlay.
     *
     * <p>Called every frame from {@link EwoQuickEdit}. Rust owns the mode
     * because it owns the layout; Java only reports whether the conditions
     * hold. The transition out of the mode is where an in-progress drag is
     * committed, which is why this is a state report rather than an event.
     */
    public static native void nativeQuickEdit(boolean on);

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

    // ── Media (in-world widget transport) ─────────────────────────────────

    /**
     * Forward a left-click that landed while a *vanilla* screen is open
     * (inventory / pause menu / chat / etc.). Rust hit-tests the in-world
     * Media widget's transport buttons; if the click landed on one, the
     * action fires and this returns {@code 1} so the caller can cancel the
     * vanilla screen's click. {@code 0} = "not consumed — let vanilla have it."
     *
     * @param button GLFW button code (0 = left, 1 = right, …)
     * @param x      cursor x in raw window pixels
     * @param y      cursor y in raw window pixels
     */
    public static native byte nativeMediaTryClick(int button, double x, double y);

    // ── Crosshair (vanilla suppression) ──────────────────────────────────

    /**
     * Returns {@code 1} when the CROSSHAIR overlay tab has the custom
     * crosshair enabled and the {@link dev.lewlone.ewohud.mixin.GuiCrosshairMixin}
     * should cancel vanilla's {@code Gui.extractCrosshair}; {@code 0}
     * otherwise. Rust owns the config — the editor in the overlay mutates
     * the same value this reads.
     */
    public static native byte nativeIsCustomCrosshairEnabled();

    /**
     * Hard-terminates the process ({@code TerminateProcess} on Windows —
     * skips DLL detach, so it works even when native teardown is
     * deadlocked). Called only by the exit watchdog armed in
     * {@link EwoHudMod}, seconds after orderly JVM shutdown began.
     */
    public static native void nativeForceExit();
}
