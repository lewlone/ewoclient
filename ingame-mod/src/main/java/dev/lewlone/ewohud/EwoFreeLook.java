package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.util.Mth;
import org.lwjgl.glfw.GLFW;

/**
 * FreeLook module (Phase G) — pan the camera freely while a key is held,
 * without turning the player's body.
 *
 * <p>While {@link #isActive()} the camera mixin ({@code CameraMixin}) drives
 * the camera from {@link #yaw()} / {@link #pitch()} instead of the player's
 * rotation, and the mouse mixin ({@code MouseHandlerMixin}) feeds look deltas
 * here instead of to {@code LocalPlayer.turn}. On release the camera snaps
 * back to wherever the body faces.
 */
public final class EwoFreeLook {
    private EwoFreeLook() {}

    /** {@code Entity.turn} scales the raw mouse delta by this before applying. */
    private static final float TURN_FACTOR = 0.15f;

    private static boolean active;
    private static float yaw;
    private static float pitch;

    /** Whether FreeLook is currently steering the camera. */
    public static boolean isActive() {
        return active;
    }

    /** The free camera's yaw (degrees). */
    public static float yaw() {
        return yaw;
    }

    /** The free camera's pitch (degrees, clamped to ±90). */
    public static float pitch() {
        return pitch;
    }

    /**
     * Per-frame update — decide whether FreeLook is active (module on, its key
     * held, in a world with no screen open) and snapshot the player's facing
     * on the frame it switches on so the free camera starts aligned with the
     * body.
     */
    public static void update() {
        Minecraft mc = Minecraft.getInstance();
        boolean want = false;
        if (mc != null && mc.player != null && mc.screen == null
                && EwoModuleData.enabled(EwoModuleData.FREELOOK)) {
            int key = EwoKeybinds.code("freelook");
            if (key != 0 && mc.getWindow() != null) {
                want = GLFW.glfwGetKey(mc.getWindow().handle(), key) == GLFW.GLFW_PRESS;
            }
        }
        if (want && !active) {
            // Rising edge — start the free camera from the body's facing.
            LocalPlayer player = mc.player;
            yaw = player.getYRot();
            pitch = player.getXRot();
        }
        active = want;
    }

    /**
     * Feed a raw mouse look delta into the free camera, applying the same
     * scaling {@code Entity.turn} would — so FreeLook tracks mouse sensitivity.
     * Called by the mouse mixin in place of {@code LocalPlayer.turn}.
     */
    public static void addLook(double yawDelta, double pitchDelta) {
        yaw += (float) yawDelta * TURN_FACTOR;
        pitch = Mth.clamp(pitch + (float) pitchDelta * TURN_FACTOR, -90.0f, 90.0f);
    }
}
