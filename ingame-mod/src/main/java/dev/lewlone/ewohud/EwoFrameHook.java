package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;

/**
 * The per-frame HUD drive, shared by the version-specific frame mixins.
 *
 * <p>Minecraft 26.2 deleted {@code RenderSystem.flipFrame} (the 26.1 hook
 * point) and moved end-of-frame to {@code Minecraft.renderFrame} →
 * {@code GpuSurface.present()}, so the frame hook now has two mixin
 * variants — {@code EwoHudMixin} (26.1, flipFrame HEAD) and
 * {@code EwoHudMixin262} (26.2+, before present) — selected at runtime by
 * {@code EwoMixinPlugin}. Both call here; this class is the single copy of
 * what happens once per frame.
 */
public final class EwoFrameHook {

    private EwoFrameHook() {}

    /** Consecutive frames that threw. Reset by any clean frame. */
    private static int consecutiveFailures;
    /** Failures logged so far, so a flapping fault cannot flood the log. */
    private static int failuresLogged;

    /** Consecutive failing frames after which the native bridge is switched off. */
    private static final int MAX_CONSECUTIVE_FAILURES = 5;
    private static final int MAX_FAILURES_LOGGED = 10;

    /**
     * Once per frame, from the version-specific frame mixins. Never throws:
     * this runs inside Minecraft's render loop, and an escaping
     * {@code UnsatisfiedLinkError} (a stale {@code ewo_jni.dll} — there is no
     * ABI handshake) or NPE would crash the game. After
     * {@link #MAX_CONSECUTIVE_FAILURES} bad frames in a row the native bridge
     * is disabled for the rest of the session and that is logged once.
     */
    public static void run() {
        if (!EwoHudMod.nativeReady) {
            return;
        }
        try {
            runFrame();
            consecutiveFailures = 0;
        } catch (Throwable t) {
            consecutiveFailures++;
            if (failuresLogged < MAX_FAILURES_LOGGED) {
                failuresLogged++;
                System.err.println("[ewo-hud] frame hook failed (" + consecutiveFailures
                        + " in a row): " + t);
                if (failuresLogged == 1) {
                    t.printStackTrace();
                }
            }
            if (consecutiveFailures >= MAX_CONSECUTIVE_FAILURES) {
                EwoHudMod.disableNativeBridge("frame hook failed "
                        + consecutiveFailures + " frames in a row; last: " + t);
            }
        }
    }

    private static void runFrame() {
        EwoSkinExport.tick();
        EwoTargetSkin.tick();
        // Before the cursor forward below: the mouse mixin gates on this, and
        // both want to see the same frame's state.
        EwoQuickEdit.tick();
        // Forward the cursor position to Rust each frame whenever a *vanilla*
        // screen has the cursor unlocked (inventory / pause / chat / etc.) so
        // the in-world Media widget can render hover state on its transport
        // buttons. Skip for our own EwoOverlayScreen (it forwards via
        // mouseMoved already), and during gameplay (screen == null) flag the
        // cursor as offscreen so the hover state clears.
        Minecraft mc = Minecraft.getInstance();
        if (mc != null && EwoCompat.screen(mc) != null && !(EwoCompat.screen(mc) instanceof EwoOverlayScreen)) {
            EwoHudNative.nativeMouseMove(mc.mouseHandler.xpos(), mc.mouseHandler.ypos());
        } else if (mc != null && EwoCompat.screen(mc) == null) {
            EwoHudNative.nativeMouseMove(-9999.0, -9999.0);
        }
        // The three per-frame render-thread calls. When the profiler is on,
        // time each; the off-path is byte-for-byte what it was before.
        if (EwoPerf.on()) {
            long t0 = System.nanoTime();
            EwoHudData.capture();
            long t1 = System.nanoTime();
            EwoHudNative.nativeRender();
            long t2 = System.nanoTime();
            EwoModules.tick();
            EwoPerf.record(t1 - t0, t2 - t1, System.nanoTime() - t2);
        } else {
            EwoHudData.capture();
            EwoHudNative.nativeRender();
            EwoModules.tick();
        }
    }
}
