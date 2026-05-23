package dev.lewlone.ewohud.mixin;

import com.mojang.blaze3d.vertex.PoseStack;
import net.minecraft.client.renderer.MultiBufferSource;
import net.minecraft.client.renderer.ScreenEffectRenderer;
import net.minecraft.client.renderer.texture.TextureAtlasSprite;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoModuleData;

/**
 * No Fire Overlay module (Phase G) — hides the screen-filling fire texture.
 *
 * <p>{@code ScreenEffectRenderer.renderFire} is the private static method that
 * draws the fire quads when the player is on fire. Cancelling at HEAD skips
 * the entire render; the player is still on fire (still takes damage), only
 * the screen overlay is hidden. Underwater and pumpkin overlays are untouched.
 */
@Mixin(ScreenEffectRenderer.class)
public class ScreenEffectRendererMixin {

    @Inject(method = "renderFire", at = @At("HEAD"), cancellable = true)
    private static void ewo$noFireOverlay(
            PoseStack pose,
            MultiBufferSource buffers,
            TextureAtlasSprite sprite,
            CallbackInfo ci) {
        if (EwoModuleData.enabled(EwoModuleData.NO_FIRE_OVERLAY)) {
            ci.cancel();
        }
    }
}
