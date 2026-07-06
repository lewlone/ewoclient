package dev.lewlone.ewohud;

import java.lang.invoke.MethodHandle;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;

import net.minecraft.client.Camera;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.renderer.GameRenderer;

/**
 * Version compat for the handful of Minecraft members that moved between
 * the 26.1 and 26.2 lines. One ewo-hud jar serves both manifest lines, so
 * shared (non-mixin) code must not hold a compiled reference to a member
 * that only exists on one side — it would {@code NoSuchFieldError} /
 * {@code NoSuchMethodError} at runtime on the other.
 *
 * <p>The jar compiles against the <b>26.2</b> jar (see build.ps1), so the
 * 26.2 shapes are plain direct calls here; the 26.1 shapes are reached via
 * {@link MethodHandle}s probed once at class-init (static-final handles
 * JIT-inline to near-direct cost). The probe itself decides the version:
 * if {@code Minecraft.screen} exists as a field, this is the 26.1 line.
 *
 * <p>The moved members (26.1 → 26.2):
 * <ul>
 *   <li>{@code Minecraft.screen} (public field) → {@code Minecraft.gui.screen()}
 *       (the Gui class split; Gui is now the screen manager)</li>
 *   <li>{@code Minecraft.setScreen(Screen)} → {@code Gui.setScreen(Screen)}</li>
 *   <li>{@code GameRenderer.getMainCamera()} → {@code GameRenderer.mainCamera()}</li>
 * </ul>
 */
public final class EwoCompat {

    /** Non-null exactly on the 26.1 line (probed at class-init). */
    private static final MethodHandle LEGACY_SCREEN_GET;
    private static final MethodHandle LEGACY_SET_SCREEN;
    private static final MethodHandle LEGACY_MAIN_CAMERA;

    static {
        MethodHandle screenGet = null;
        MethodHandle setScreen = null;
        MethodHandle mainCamera = null;
        try {
            MethodHandles.Lookup lookup = MethodHandles.publicLookup();
            screenGet = lookup.findGetter(Minecraft.class, "screen", Screen.class);
            setScreen = lookup.findVirtual(
                    Minecraft.class, "setScreen", MethodType.methodType(void.class, Screen.class));
            mainCamera = lookup.findVirtual(
                    GameRenderer.class, "getMainCamera", MethodType.methodType(Camera.class));
        } catch (ReflectiveOperationException e) {
            // 26.2+: the members moved; the direct 26.2 paths below apply.
            screenGet = null;
            setScreen = null;
            mainCamera = null;
        }
        LEGACY_SCREEN_GET = screenGet;
        LEGACY_SET_SCREEN = setScreen;
        LEGACY_MAIN_CAMERA = mainCamera;
    }

    private EwoCompat() {}

    /** The currently-open screen, or null during gameplay. */
    public static Screen screen(Minecraft mc) {
        if (LEGACY_SCREEN_GET == null) {
            return mc.gui.screen();
        }
        try {
            return (Screen) LEGACY_SCREEN_GET.invokeExact(mc);
        } catch (Throwable t) {
            throw new IllegalStateException("EwoCompat.screen", t);
        }
    }

    public static void setScreen(Minecraft mc, Screen screen) {
        if (LEGACY_SET_SCREEN == null) {
            mc.gui.setScreen(screen);
            return;
        }
        try {
            LEGACY_SET_SCREEN.invokeExact(mc, screen);
        } catch (Throwable t) {
            throw new IllegalStateException("EwoCompat.setScreen", t);
        }
    }

    public static Camera mainCamera(GameRenderer gameRenderer) {
        if (LEGACY_MAIN_CAMERA == null) {
            return gameRenderer.mainCamera();
        }
        try {
            return (Camera) LEGACY_MAIN_CAMERA.invokeExact(gameRenderer);
        } catch (Throwable t) {
            throw new IllegalStateException("EwoCompat.mainCamera", t);
        }
    }
}
