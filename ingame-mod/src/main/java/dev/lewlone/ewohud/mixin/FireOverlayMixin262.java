package dev.lewlone.ewohud.mixin;

import net.minecraft.client.renderer.ScreenEffectRenderer;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoModuleData;

/**
 * No Fire Overlay — <b>Minecraft 26.2+</b> variant.
 *
 * <p>26.2 moved {@code ScreenEffectRenderer} onto the submit-node pipeline:
 * {@code renderFire(PoseStack, MultiBufferSource, TextureAtlasSprite)} became
 * {@code submitFire(PoseStack, SubmitNodeCollector, TextureAtlasSprite)}.
 * Same cancel-at-HEAD suppression, new signature. {@link FireOverlayMixin}
 * covers the 26.1 line; {@code EwoMixinPlugin} applies exactly one of the two
 * per runtime.
 */
@Mixin(ScreenEffectRenderer.class)
public class FireOverlayMixin262 {

    // No target-arg capture — mirrors FireOverlayMixin; neither handler
    // reads the args, and skipping capture keeps version-specific types
    // out of both signatures.
    @Inject(method = "submitFire", at = @At("HEAD"), cancellable = true)
    private static void ewo$noFireOverlay(CallbackInfo ci) {
        if (EwoModuleData.enabled(EwoModuleData.NO_FIRE_OVERLAY)) {
            ci.cancel();
        }
    }
}
