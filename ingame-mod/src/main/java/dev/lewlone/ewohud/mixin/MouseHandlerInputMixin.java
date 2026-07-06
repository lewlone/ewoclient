package dev.lewlone.ewohud.mixin;

import dev.lewlone.ewohud.EwoCompat;

import net.minecraft.client.Minecraft;
import net.minecraft.client.MouseHandler;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.input.MouseButtonInfo;

import org.lwjgl.glfw.GLFW;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoHudNative;
import dev.lewlone.ewohud.EwoOverlayScreen;

/**
 * Forwards left-clicks landing on the in-world Media widget's transport
 * buttons to Rust whenever a *vanilla* screen has the cursor unlocked
 * (inventory, pause, chat, etc.) — so users can click prev / play / next
 * without opening the EwoClient overlay.
 *
 * <p>Injects at the head of {@code MouseHandler.onButton}, the GLFW mouse-
 * button callback Minecraft installs (named {@code onButton} in 26.x — what
 * older versions called {@code onPress}). The native side hit-tests the
 * widget's bounds; if a transport button matches the click is consumed and
 * the vanilla screen never sees the press. Otherwise the call falls through
 * untouched and vanilla handles it normally.
 *
 * <p>The press only forwards when {@code EwoCompat.screen(mc) != null} (cursor is free
 * for the user to actually click) and the screen isn't our own
 * {@link EwoOverlayScreen} (which routes its own input through
 * {@code nativeMouseButton} already).
 */
@Mixin(MouseHandler.class)
public class MouseHandlerInputMixin {

    @Shadow private double xpos;
    @Shadow private double ypos;

    @Inject(method = "onButton", at = @At("HEAD"), cancellable = true)
    private void ewo$onButton(long windowHandle, MouseButtonInfo info, int action,
                              CallbackInfo ci) {
        // Press only — releases of the same button shouldn't double-dispatch.
        if (action != GLFW.GLFW_PRESS) {
            return;
        }
        Minecraft mc = Minecraft.getInstance();
        if (mc == null) {
            return;
        }
        Screen screen = EwoCompat.screen(mc);
        // Gameplay (cursor grabbed) → vanilla path. Overlay open → already
        // forwarded via EwoOverlayScreen. Otherwise: vanilla menu, hit-test.
        if (screen == null || screen instanceof EwoOverlayScreen) {
            return;
        }
        if (EwoHudNative.nativeMediaTryClick(info.button(), this.xpos, this.ypos) != 0) {
            ci.cancel();
        }
    }
}
