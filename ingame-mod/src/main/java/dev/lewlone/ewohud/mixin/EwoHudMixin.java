package dev.lewlone.ewohud.mixin;

import com.mojang.blaze3d.TracyFrameCapture;
import com.mojang.blaze3d.systems.RenderSystem;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoHudData;
import dev.lewlone.ewohud.EwoHudMod;
import dev.lewlone.ewohud.EwoHudNative;
import dev.lewlone.ewohud.EwoModules;
import dev.lewlone.ewohud.EwoSkinExport;

/**
 * Drives the in-game HUD paint once per frame.
 *
 * <p>Injects at the head of {@link RenderSystem#flipFrame} — Minecraft's
 * buffer-swap call — so the window's default framebuffer already holds the
 * finished frame when the Skia HUD composites on top. A universal
 * end-of-frame hook: it fires on the title screen, in menus and in-game.
 *
 * <p>Each frame the handler captures live game data into the shared block
 * ({@link EwoHudData#capture}) and then asks Rust to paint + composite
 * ({@link EwoHudNative#nativeRender}).
 */
@Mixin(RenderSystem.class)
public class EwoHudMixin {

    @Inject(method = "flipFrame", at = @At("HEAD"))
    private static void ewo$onFlipFrame(TracyFrameCapture tracyFrameCapture, CallbackInfo ci) {
        if (!EwoHudMod.nativeReady) {
            return;
        }
        EwoSkinExport.tick();
        EwoHudData.capture();
        EwoHudNative.nativeRender();
        EwoModules.tick();
    }
}
