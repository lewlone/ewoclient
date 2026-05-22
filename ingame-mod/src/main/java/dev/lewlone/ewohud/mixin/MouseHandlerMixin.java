package dev.lewlone.ewohud.mixin;

import net.minecraft.client.MouseHandler;
import net.minecraft.client.player.LocalPlayer;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Redirect;

import dev.lewlone.ewohud.EwoFreeLook;

/**
 * FreeLook module (Phase G) — mouse-look diversion.
 *
 * <p>{@code MouseHandler.turnPlayer} applies the accumulated mouse delta to
 * {@code LocalPlayer.turn}. While FreeLook is active this redirects that call:
 * the delta drives the free camera ({@link EwoFreeLook}) instead of the
 * player, so the body's facing is frozen while the camera pans.
 */
@Mixin(MouseHandler.class)
public class MouseHandlerMixin {

    @Redirect(
            method = "turnPlayer",
            at = @At(
                    value = "INVOKE",
                    target = "Lnet/minecraft/client/player/LocalPlayer;turn:(DD)V"))
    private void ewo$turn(LocalPlayer player, double yawDelta, double pitchDelta) {
        if (EwoFreeLook.isActive()) {
            EwoFreeLook.addLook(yawDelta, pitchDelta);
        } else {
            player.turn(yawDelta, pitchDelta);
        }
    }
}
