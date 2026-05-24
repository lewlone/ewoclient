package dev.lewlone.ewohud.mixin;

import com.mojang.blaze3d.vertex.PoseStack;
import net.minecraft.client.renderer.MultiBufferSource;
import net.minecraft.client.renderer.ScreenEffectRenderer;
import net.minecraft.client.renderer.texture.TextureAtlasSprite;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.block.state.BlockState;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

import dev.lewlone.ewohud.EwoModuleData;

/**
 * Screen-overlay suppressors.
 *
 * <p>{@code renderFire} (No Fire Overlay) is the private static method that
 * draws the fire quads when the player is on fire. Cancelling at HEAD skips
 * the render; the player is still on fire (still takes damage), only the
 * screen overlay is hidden.
 *
 * <p>{@code getViewBlockingState} (No Pumpkin Overlay) finds the
 * {@link BlockState} that's currently covering the player's view — the
 * pumpkin block when they're wearing a carved pumpkin on their head. By
 * cancelling to {@code null} when the module is on, the renderScreenEffect
 * caller sees "no view-blocking block" and skips the pumpkin overlay
 * specifically. Underwater + fire paths come from elsewhere and stay live.
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

    @Inject(method = "getViewBlockingState", at = @At("HEAD"), cancellable = true)
    private static void ewo$noPumpkinOverlay(Player player, CallbackInfoReturnable<BlockState> cir) {
        if (EwoModuleData.enabled(EwoModuleData.NO_PUMPKIN_OVERLAY)) {
            cir.setReturnValue(null);
        }
    }
}
