package dev.lewlone.ewohud.mixin;

import com.mojang.blaze3d.TracyFrameCapture;
import com.mojang.blaze3d.systems.RenderSystem;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoFrameHook;

/**
 * Drives the in-game HUD paint once per frame — <b>Minecraft 26.1 line</b>.
 *
 * <p>Injects at the head of {@link RenderSystem#flipFrame} — Minecraft's
 * buffer-swap call — so the window's default framebuffer already holds the
 * finished frame when the Skia HUD composites on top. A universal
 * end-of-frame hook: it fires on the title screen, in menus and in-game.
 *
 * <p>26.2 deleted {@code flipFrame}; {@link EwoHudMixin262} is the same hook
 * against the new {@code Minecraft.renderFrame} / {@code GpuSurface.present}
 * path. {@link EwoMixinPlugin} applies exactly one of the two per runtime.
 */
@Mixin(RenderSystem.class)
public class EwoHudMixin {

    @Inject(method = "flipFrame", at = @At("HEAD"))
    private static void ewo$onFlipFrame(TracyFrameCapture tracyFrameCapture, CallbackInfo ci) {
        EwoFrameHook.run();
    }
}
