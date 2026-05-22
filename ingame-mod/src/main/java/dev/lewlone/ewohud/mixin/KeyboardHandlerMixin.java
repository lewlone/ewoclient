package dev.lewlone.ewohud.mixin;

import net.minecraft.client.KeyboardHandler;
import net.minecraft.client.Minecraft;
import net.minecraft.client.input.KeyEvent;
import org.lwjgl.glfw.GLFW;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoHudMod;
import dev.lewlone.ewohud.EwoKeybinds;
import dev.lewlone.ewohud.EwoOverlayScreen;

/**
 * Opens the EwoClient overlay on the toggle key (Right Shift).
 *
 * <p>Injects at the head of {@code KeyboardHandler.keyPress} — the GLFW key
 * callback. It opens the overlay only on a genuine key <i>press</i> (not a
 * release or auto-repeat) and only when no screen is already showing; while
 * {@link EwoOverlayScreen} is open, Right Shift routes to its {@code keyPressed}
 * (which closes it), so the two halves of the toggle never collide. The
 * press-only gate matters: without it the release event that follows a close
 * would see {@code screen == null} and immediately reopen the overlay.
 */
@Mixin(KeyboardHandler.class)
public class KeyboardHandlerMixin {

    @Inject(method = "keyPress", at = @At("HEAD"), cancellable = true)
    private void ewo$onKeyPress(long window, int action, KeyEvent event, CallbackInfo ci) {
        if (!EwoHudMod.nativeReady) {
            return;
        }
        if (action != GLFW.GLFW_PRESS) {
            return; // press only — ignore release / auto-repeat
        }
        if (event.key() != EwoKeybinds.overlayKey()) {
            return;
        }
        Minecraft mc = Minecraft.getInstance();
        if (mc.screen == null) {
            mc.setScreen(new EwoOverlayScreen());
            ci.cancel(); // don't let the open keypress fall through to the game
        }
    }
}
