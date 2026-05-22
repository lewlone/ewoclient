package dev.lewlone.ewohud.mixin;

import net.minecraft.client.Camera;
import net.minecraft.client.OptionInstance;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Redirect;

import dev.lewlone.ewohud.EwoModuleData;

/**
 * FOV Control module (Phase G) — overrides the field of view.
 *
 * <p>{@code Camera.calculateFov} reads the base FOV from {@code options.fov()}
 * and layers the speed / death / fluid modifiers on top. This redirects only
 * that one base read: when the module is on the user's chosen FOV replaces the
 * vanilla setting — past the 110° cap if they want — while the natural FOV
 * effects still apply on top. Vanilla {@code options.txt} is never written, so
 * toggling the module off restores the vanilla FOV exactly.
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
}
