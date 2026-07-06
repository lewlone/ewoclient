package dev.lewlone.ewohud.mixin;

import net.minecraft.client.renderer.ScreenEffectRenderer;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.block.state.BlockState;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;

import dev.lewlone.ewohud.EwoModuleData;

/**
 * No Pumpkin Overlay — version-agnostic (26.1 and 26.2 verify identical).
 *
 * <p>{@code getViewBlockingState} finds the {@link BlockState} that's
 * currently covering the player's view — the pumpkin block when they're
 * wearing a carved pumpkin on their head. By cancelling to {@code null} when
 * the module is on, the caller sees "no view-blocking block" and skips the
 * pumpkin overlay specifically. Underwater + fire paths come from elsewhere
 * and stay live.
 *
 * <p>The fire-overlay suppressor that used to live here moved to
 * {@link FireOverlayMixin} / {@link FireOverlayMixin262} — 26.2 renamed its
 * target method, and {@code EwoMixinPlugin} selects mixins per-class, so the
 * versioned inject couldn't share this class.
 */
@Mixin(ScreenEffectRenderer.class)
public class ScreenEffectRendererMixin {

    @Inject(method = "getViewBlockingState", at = @At("HEAD"), cancellable = true)
    private static void ewo$noPumpkinOverlay(Player player, CallbackInfoReturnable<BlockState> cir) {
        if (EwoModuleData.enabled(EwoModuleData.NO_PUMPKIN_OVERLAY)) {
            cir.setReturnValue(null);
        }
    }
}
