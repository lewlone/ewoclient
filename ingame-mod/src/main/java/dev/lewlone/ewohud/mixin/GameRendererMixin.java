package dev.lewlone.ewohud.mixin;

import com.mojang.blaze3d.vertex.PoseStack;
import net.minecraft.client.renderer.GameRenderer;
import net.minecraft.client.renderer.state.level.CameraRenderState;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoModuleData;

/**
 * No Damage Tilt + No View Bob modules (Phase G) — camera-comfort toggles.
 *
 * <p>{@code GameRenderer.bobHurt} tilts the camera when the player takes
 * damage; {@code bobView} applies the walking view-bob. Each module cancels
 * its method at HEAD when enabled, so the camera motion is simply skipped —
 * non-destructive, and the vanilla View Bobbing option is left untouched.
 */
@Mixin(GameRenderer.class)
public class GameRendererMixin {

    @Inject(method = "bobHurt", at = @At("HEAD"), cancellable = true)
    private void ewo$noDamageTilt(CameraRenderState camera, PoseStack pose, CallbackInfo ci) {
        if (EwoModuleData.enabled(EwoModuleData.NO_DAMAGE_TILT)) {
            ci.cancel();
        }
    }

    @Inject(method = "bobView", at = @At("HEAD"), cancellable = true)
    private void ewo$noViewBob(CameraRenderState camera, PoseStack pose, CallbackInfo ci) {
        if (EwoModuleData.enabled(EwoModuleData.NO_VIEW_BOB)) {
            ci.cancel();
        }
    }
}
