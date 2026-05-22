package dev.lewlone.ewohud.mixin;

import net.minecraft.client.Camera;
import net.minecraft.client.OptionInstance;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.ModifyVariable;
import org.spongepowered.asm.mixin.injection.Redirect;

import dev.lewlone.ewohud.EwoFreeLook;
import dev.lewlone.ewohud.EwoModuleData;

/**
 * FOV Control + FreeLook camera hooks (Phase G).
 *
 * <p><b>FOV Control</b> redirects the lone {@code options.fov()} read in
 * {@code Camera.calculateFov}, so the module's slider value replaces the base
 * FOV — past the 110° cap — while the speed/death/fluid effects still layer on
 * top. Vanilla {@code options.txt} is never written.
 *
 * <p><b>FreeLook</b> overrides the two arguments to {@code Camera.setRotation}
 * while the free camera is active, so the camera follows {@link EwoFreeLook}'s
 * yaw/pitch instead of the player's — the body's facing stays frozen, and the
 * camera snaps back to it on release.
 */
@Mixin(Camera.class)
public class CameraMixin {

    @Redirect(
            method = "calculateFov",
            at = @At(
                    value = "INVOKE",
                    target = "Lnet/minecraft/client/OptionInstance;get()Ljava/lang/Object;"))
    private Object ewo$fov(OptionInstance instance) {
        if (EwoModuleData.enabled(EwoModuleData.FOV)) {
            return Integer.valueOf(Math.round(EwoModuleData.setting(EwoModuleData.FOV, 0)));
        }
        return instance.get();
    }

    @ModifyVariable(method = "setRotation", at = @At("HEAD"), argsOnly = true, ordinal = 0)
    private float ewo$freeLookYaw(float yaw) {
        return EwoFreeLook.isActive() ? EwoFreeLook.yaw() : yaw;
    }

    @ModifyVariable(method = "setRotation", at = @At("HEAD"), argsOnly = true, ordinal = 1)
    private float ewo$freeLookPitch(float pitch) {
        return EwoFreeLook.isActive() ? EwoFreeLook.pitch() : pitch;
    }
}
