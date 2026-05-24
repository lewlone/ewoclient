package dev.lewlone.ewohud.mixin;

import net.minecraft.client.renderer.entity.LivingEntityRenderer;
import net.minecraft.client.renderer.entity.state.LivingEntityRenderState;
import net.minecraft.client.renderer.texture.OverlayTexture;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

import dev.lewlone.ewohud.EwoModuleData;

/**
 * Hit Color module — v1 ships as a hurt-flash suppressor.
 *
 * <p>{@code LivingEntityRenderer.getOverlayCoords} packs the (u, v) the entity
 * shader samples from {@code overlay.png}. When the entity's render state
 * has {@code hurtTime > 0}, vanilla returns a packed value whose v-row holds
 * the red flash; otherwise it returns {@link OverlayTexture#NO_OVERLAY}.
 *
 * <p>Forcing {@code NO_OVERLAY} when the module is on means the shader
 * never samples the red row, so no hurt-flash renders. Damage events
 * themselves are untouched.
 *
 * <p>Polish-item follow-up: pack a custom overlay coord that samples a
 * recolored row (rose / champagne / lavender) instead of suppressing —
 * needs a recolored {@code overlay.png} variant shipped with the mod.
 */
@Mixin(LivingEntityRenderer.class)
public class LivingEntityRendererMixin {

    @Inject(method = "getOverlayCoords", at = @At("HEAD"), cancellable = true)
    private static void ewo$hitColor(
            LivingEntityRenderState state,
            float u,
            CallbackInfoReturnable<Integer> cir) {
        if (EwoModuleData.enabled(EwoModuleData.HIT_COLOR)) {
            cir.setReturnValue(OverlayTexture.NO_OVERLAY);
        }
    }
}
