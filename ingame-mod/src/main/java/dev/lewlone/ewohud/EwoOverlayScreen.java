package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.input.KeyEvent;
import net.minecraft.client.input.MouseButtonEvent;
import net.minecraft.network.chat.Component;

import org.lwjgl.glfw.GLFW;

/**
 * The in-game overlay screen (Phase E, E4+).
 *
 * <p>Used purely as an <b>input sink + cursor manager</b>: while it is the
 * active screen, Minecraft frees the cursor and routes all mouse/keyboard to
 * it, and the game world receives no input. It renders nothing itself —
 * {@link #extractRenderState} is empty; the Velvet overlay is painted by the
 * Skia HUD (the {@code flipFrame} composite). Its presence <i>is</i> the
 * "overlay open" state.
 *
 * <p>Opened by {@link dev.lewlone.ewohud.mixin.KeyboardHandlerMixin} on the
 * toggle key; closed from here (toggle key or Esc). Input events are converted
 * from Minecraft's GUI-scaled coordinates to window pixels — the space the
 * Skia HUD draws in — and forwarded to Rust.
 */
public final class EwoOverlayScreen extends Screen {

    public EwoOverlayScreen() {
        super(Component.literal("EwoClient Overlay"));
    }

    /** Don't pause singleplayer while the overlay is open. */
    @Override
    public boolean isPauseScreen() {
        return false;
    }

    /** Render nothing — the Skia HUD paints the overlay over the framebuffer. */
    @Override
    public void extractRenderState(GuiGraphicsExtractor extractor, int mouseX, int mouseY,
                                   float partialTick) {
        // intentionally empty
    }

    /** GUI units → window pixels (the Skia HUD's coordinate space). */
    private static int scale() {
        return Minecraft.getInstance().getWindow().getGuiScale();
    }

    @Override
    public void mouseMoved(double x, double y) {
        int s = scale();
        EwoHudNative.nativeMouseMove(x * s, y * s);
    }

    @Override
    public boolean mouseClicked(MouseButtonEvent event, boolean doubleClick) {
        int s = scale();
        EwoHudNative.nativeMouseButton(event.button(), true, event.x() * s, event.y() * s);
        return true;
    }

    @Override
    public boolean mouseReleased(MouseButtonEvent event) {
        int s = scale();
        EwoHudNative.nativeMouseButton(event.button(), false, event.x() * s, event.y() * s);
        return true;
    }

    @Override
    public boolean mouseDragged(MouseButtonEvent event, double dragX, double dragY) {
        int s = scale();
        EwoHudNative.nativeMouseMove(event.x() * s, event.y() * s);
        return true;
    }

    @Override
    public boolean mouseScrolled(double x, double y, double scrollX, double scrollY) {
        EwoHudNative.nativeMouseScroll(scrollX, scrollY);
        return true;
    }

    @Override
    public boolean keyPressed(KeyEvent event) {
        // The overlay-open key (Right Shift by default — see EwoKeybinds) or
        // Esc closes the overlay. This screen overrides keyPressed wholesale,
        // so Esc must be handled here — the default shouldCloseOnEsc path
        // never runs.
        int key = event.key();
        if (key == EwoKeybinds.overlayKey() || key == GLFW.GLFW_KEY_ESCAPE) {
            this.onClose();
            return true;
        }
        EwoHudNative.nativeKey(key, true, event.modifiers());
        return true;
    }

    @Override
    public boolean keyReleased(KeyEvent event) {
        EwoHudNative.nativeKey(event.key(), false, event.modifiers());
        return true;
    }
}
