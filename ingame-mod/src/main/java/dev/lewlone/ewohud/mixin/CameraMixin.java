package dev.lewlone.ewohud.mixin;

import net.minecraft.client.Camera;
import net.minecraft.client.CameraType;
import net.minecraft.client.OptionInstance;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.ModifyVariable;
import org.spongepowered.asm.mixin.injection.Redirect;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

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
 * <p><b>FreeLook</b> turns the camera into a free-flying spectator view while
 * its key is held:
 * <ul>
 *   <li>{@code setRotation}'s two arguments are overridden so the camera follows
 *       {@link EwoFreeLook}'s yaw/pitch instead of the player's;</li>
 *   <li>the {@code isFirstPerson} check in {@code alignWithEntity} is forced
 *       false, so the camera detaches (third-person flag) and the player model
 *       renders where the frozen body stands;</li>
 *   <li>the camera position is overwritten at the tail of {@code alignWithEntity}
 *       with {@link EwoFreeLook}'s flown position.</li>
 * </ul>
 * All of it reverts the instant the key is released.
 */
@Mixin(Camera.class)
public abstract class CameraMixin {

    /** Camera's own position setter — used to place the free camera. */
    @Shadow
    protected abstract void setPosition(double x, double y, double z);

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

    /**
     * Force {@code alignWithEntity}'s first-person test false while FreeLook is
     * active, so the camera takes the detached (third-person) path and the
     * player model is drawn where the body stands.
     */
    @Redirect(
            method = "alignWithEntity",
            at = @At(
                    value = "INVOKE",
                    target = "Lnet/minecraft/client/CameraType;isFirstPerson()Z"))
    private boolean ewo$freeLookDetach(CameraType type) {
        return !EwoFreeLook.isActive() && type.isFirstPerson();
    }

    /**
     * Place the free camera. {@code alignWithEntity} has just positioned the
     * camera on the body; while FreeLook is active that is overwritten with the
     * flown position before the frame's frustum / perspective are built.
     */
    @Inject(method = "alignWithEntity", at = @At("RETURN"))
    private void ewo$freeLookPosition(float partialTick, CallbackInfo ci) {
        if (EwoFreeLook.isActive()) {
            this.setPosition(EwoFreeLook.camX(), EwoFreeLook.camY(), EwoFreeLook.camZ());
        }
    }
}
