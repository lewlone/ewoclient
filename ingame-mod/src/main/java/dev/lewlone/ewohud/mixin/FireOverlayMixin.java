package dev.lewlone.ewohud.mixin;

import net.minecraft.client.renderer.ScreenEffectRenderer;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoModuleData;

/**
 * No Fire Overlay — <b>Minecraft 26.1 line</b>.
 *
 * <p>{@code renderFire} is the private static method that draws the fire
 * quads when the player is on fire. Cancelling at HEAD skips the render;
 * the player is still on fire (still takes damage), only the screen overlay
 * is hidden.
 *
 * <p>26.2 renamed it to {@code submitFire} (submit-node pipeline) —
 * {@link FireOverlayMixin262} covers that; {@code EwoMixinPlugin} applies
 * exactly one of the two per runtime. Split out of
 * {@link ScreenEffectRendererMixin} because the pumpkin inject in there is
 * version-agnostic and plugin selection is per-class.
 */
@Mixin(ScreenEffectRenderer.class)
public class FireOverlayMixin {

    // Handler declares no target-arg capture (only CallbackInfo) — the
    // 26.1 signature's MultiBufferSource type no longer exists in 26.2,
    // and this file must still compile against the 26.2 build classpath.
    @Inject(method = "renderFire", at = @At("HEAD"), cancellable = true)
    private static void ewo$noFireOverlay(CallbackInfo ci) {
        if (EwoModuleData.enabled(EwoModuleData.NO_FIRE_OVERLAY)) {
            ci.cancel();
        }
    }
}
