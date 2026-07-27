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

    public static void run() {
        if (!EwoHudMod.nativeReady) {
            return;
        }
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
