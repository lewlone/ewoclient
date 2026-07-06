package dev.lewlone.ewohud.mixin;

import dev.lewlone.ewohud.EwoCompat;

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
import dev.lewlone.ewohud.EwoModules;
import dev.lewlone.ewohud.EwoOverlayScreen;

/**
 * Routes EwoClient key presses (Phase F5c + Phase G).
 *
 * <p>Injects at the head of {@code KeyboardHandler.keyPress} — the GLFW key
 * callback — on a genuine key <i>press</i> (not a release or auto-repeat):
 *
 * <ul>
 *   <li>the overlay key (Right Shift by default) opens {@link EwoOverlayScreen}
 *       when no screen is showing; while the overlay is open, the key routes to
 *       the screen's own {@code keyPressed}, which closes it;</li>
 *   <li>a module's keybind toggles that module via the native bridge — only
 *       in-world, so a bound key still types normally in chat or menus.</li>
 * </ul>
 *
 * <p>FreeLook's key is not handled here: it is hold-to-activate, polled each
 * frame by {@code EwoFreeLook}. A press this handler acts on is cancelled so
 * it doesn't fall through to the game.
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
        int key = event.key();
        Minecraft mc = Minecraft.getInstance();

        // Overlay toggle key — opens the overlay when no screen is showing.
        if (key == EwoKeybinds.overlayKey()) {
            if (EwoCompat.screen(mc) == null) {
                EwoCompat.setScreen(mc, new EwoOverlayScreen());
                ci.cancel(); // don't let the open keypress fall through to the game
            }
            return;
        }

        // Module toggle keys — only in-world (no screen open), so a bound key
        // still types normally in chat or menus.
        if (EwoCompat.screen(mc) == null && EwoModules.handleKeyPress(key)) {
            ci.cancel();
        }
    }
}
