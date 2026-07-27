package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.screens.Screen;

import org.lwjgl.glfw.GLFW;

/**
 * Quick edit — move and resize HUD widgets from any screen that frees the
 * cursor, without opening the EwoClient overlay.
 *
 * <p>Hold {@code Alt} while an inventory, the pause menu, chat, or any other
 * vanilla screen is up: widget outlines and resize grips appear over it and
 * clicks go to the HUD instead of the screen. Release Alt and the screen
 * behaves normally again.
 *
 * <h2>Why hold-a-modifier rather than a mode</h2>
 * A toggled mode can be left on by accident, and the failure is silent and
 * confusing — your inventory stops accepting clicks and nothing says why.
 * Holding a key cannot get stuck: let go and it is over. It also keeps normal
 * clicks working, so quick edit never competes with the inventory for the
 * same pixels.
 *
 * <h2>Why per-frame state rather than events</h2>
 * Rust owns the layout, so Rust owns the mode; this class only reports whether
 * the conditions hold. Reporting state (rather than sending enter/leave
 * events) means a dropped frame or an odd screen transition cannot strand the
 * mode on — the next frame corrects it.
 */
public final class EwoQuickEdit {

    private EwoQuickEdit() {}

    /** Last value pushed to Rust — the JNI call is skipped when unchanged. */
    private static boolean lastOn = false;

    /**
     * Whether quick edit is active this frame. Cached so the mouse mixin can
     * ask without repeating the checks.
     */
    private static boolean active = false;

    /** Called once per frame from {@link EwoFrameHook}. */
    public static void tick() {
        boolean on = compute();
        active = on;
        if (on != lastOn) {
            lastOn = on;
            EwoHudNative.nativeQuickEdit(on);
        }
    }

    /** Quick edit is live right now — the mouse mixin's gate. */
    public static boolean isActive() {
        return active;
    }

    private static boolean compute() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null) {
            return false;
        }
        Screen screen = EwoCompat.screen(mc);
        // Gameplay: the cursor is grabbed, there is nothing to click with.
        // Our own overlay: the real HUD editor is a tab away, and its input
        // path already owns the mouse.
        if (screen == null || screen instanceof EwoOverlayScreen) {
            return false;
        }
        long window = (mc.getWindow() != null) ? mc.getWindow().handle() : 0L;
        if (window == 0L) {
            return false;
        }
        // Raw GLFW rather than a KeyMapping: Alt is a modifier, not a bindable
        // action, and this has to read as "is it down right now" on a frame
        // where no key event fired. Same approach as EwoFreeLook.
        return GLFW.glfwGetKey(window, GLFW.GLFW_KEY_LEFT_ALT) == GLFW.GLFW_PRESS
                || GLFW.glfwGetKey(window, GLFW.GLFW_KEY_RIGHT_ALT) == GLFW.GLFW_PRESS;
    }
}
