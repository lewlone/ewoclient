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
import dev.lewlone.ewohud.EwoOverlayScreen;

/**
 * Opens the EwoClient overlay on the toggle key (Right Shift).
 *
 * <p>Injects at the head of {@code KeyboardHandler.keyPress} — the GLFW key
 * callback. It opens the overlay only when no screen is already showing; while
 * {@link EwoOverlayScreen} is open, Right Shift routes to its {@code keyPressed}
 * (which closes it), so the two halves of the toggle never collide. A
 * key-release or auto-repeat is harmless — by then a screen is open, so the
 * open condition is already false.
 */
@Mixin(KeyboardHandler.class)
public class KeyboardHandlerMixin {

    @Inject(method = "keyPress", at = @At("HEAD"), cancellable = true)
    private void ewo$onKeyPress(long window, int action, KeyEvent event, CallbackInfo ci) {
        if (!EwoHudMod.nativeReady) {
            return;
        }
        if (event.key() != GLFW.GLFW_KEY_RIGHT_SHIFT) {
            return;
        }
        Minecraft mc = Minecraft.getInstance();
        if (mc.screen == null) {
            mc.setScreen(new EwoOverlayScreen());
            ci.cancel(); // don't let the open keypress fall through to the game
        }
    }
}
