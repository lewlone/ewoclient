package dev.lewlone.ewohud;

import net.minecraft.client.KeyMapping;
import net.minecraft.client.Minecraft;
import net.minecraft.util.Mth;
import net.minecraft.world.phys.Vec3;
import org.lwjgl.glfw.GLFW;

/**
 * FreeLook module (Phase G) — a spectator-style free camera.
 *
 * <p>While the module's key is held the camera <i>detaches</i> from the player:
 * the mouse pans it ({@link #addLook}, fed by {@code MouseHandlerMixin} in place
 * of {@code LocalPlayer.turn}) and WASD / space / sneak fly it ({@link
 * #flyCamera}). {@code CameraMixin} drives the real camera from {@link #yaw()} /
 * {@link #pitch()} and {@link #camX()}/{@link #camY()}/{@link #camZ()}, and
 * forces the third-person flag so the player model renders where it stands.
 *
 * <p>The body is frozen meanwhile — the six movement keys are held down=false so
 * the character stays where it was; on release they are resynced to the real
 * keyboard and the camera snaps back to first person.
 *
 * <p>Note: the camera flies freely, including through blocks (like vanilla
 * spectator mode). In multiplayer that is a line-of-sight advantage — a leash
 * or block collision would be the legit-client compromise if that matters.
 */
public final class EwoFreeLook {
    private EwoFreeLook() {}

    /** {@code Entity.turn} scales the raw mouse delta by this before applying. */
    private static final float TURN_FACTOR = 0.15f;
    /** Base fly speed, blocks per second. */
    private static final double FLY_SPEED = 11.0;
    /** Fly-speed multiplier while the sprint key (Left Control) is held. */
    private static final double FLY_BOOST = 3.5;
    /** Largest frame delta integrated in one step — clamps post-stall jumps. */
    private static final float MAX_DT = 0.1f;

    private static boolean active;
    private static float yaw;
    private static float pitch;
    private static double camX;
    private static double camY;
    private static double camZ;
    /** {@code System.nanoTime()} of the previous update, for the fly timestep. */
    private static long lastNanos;

    /** Whether the free camera is currently steering the view. */
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

    /** The free camera's world X. */
    public static double camX() {
        return camX;
    }

    /** The free camera's world Y. */
    public static double camY() {
        return camY;
    }

    /** The free camera's world Z. */
    public static double camZ() {
        return camZ;
    }

    /**
     * Per-frame update — decide whether the free camera is active (module on,
     * its key held, in a world with no screen open), detach it at the player's
     * eye on the frame it switches on, then fly it and freeze the body.
     */
    public static void update() {
        Minecraft mc = Minecraft.getInstance();
        long window = (mc != null && mc.getWindow() != null) ? mc.getWindow().handle() : 0L;
        int key = EwoKeybinds.code("freelook");
        boolean keyDown = key != 0 && window != 0L
                && GLFW.glfwGetKey(window, key) == GLFW.GLFW_PRESS;
        boolean want = keyDown
                && EwoModuleData.enabled(EwoModuleData.FREELOOK)
                && mc != null && mc.player != null && EwoCompat.screen(mc) == null;

        long now = System.nanoTime();
        float dt = lastNanos == 0L ? 0f : Math.min((now - lastNanos) / 1.0e9f, MAX_DT);
        lastNanos = now;

        if (want && !active) {
            // Rising edge — detach the camera at the player's eye, facing as the
            // body faces, so the view starts exactly where first person was.
            Vec3 eye = mc.player.getEyePosition();
            camX = eye.x;
            camY = eye.y;
            camZ = eye.z;
            yaw = mc.player.getYRot();
            pitch = mc.player.getXRot();
        }

        if (want) {
            flyCamera(window, dt);
            freezeBody(mc, false);
        } else if (active) {
            // Falling edge — resync the movement keys to the real keyboard so a
            // key still held through the release keeps driving the body.
            freezeBody(mc, true);
        }
        active = want;
    }

    /**
     * Feed a raw mouse look delta into the free camera, applying the same
     * scaling {@code Entity.turn} would — so the camera tracks mouse sensitivity.
     * Called by the mouse mixin in place of {@code LocalPlayer.turn}.
     */
    public static void addLook(double yawDelta, double pitchDelta) {
        yaw += (float) yawDelta * TURN_FACTOR;
        pitch = Mth.clamp(pitch + (float) pitchDelta * TURN_FACTOR, -90.0f, 90.0f);
    }

    /**
     * Integrate the free camera's position from WASD (look-relative), space /
     * sneak (world up / down) and Left Control (speed boost). The move vector is
     * normalised so diagonals are not faster.
     */
    private static void flyCamera(long window, float dt) {
        if (window == 0L || dt <= 0f) {
            return;
        }
        double yawRad = Math.toRadians(yaw);
        double pitchRad = Math.toRadians(pitch);
        double cosPitch = Math.cos(pitchRad);
        // Look direction + the horizontal right vector (Minecraft convention).
        double fx = -Math.sin(yawRad) * cosPitch;
        double fy = -Math.sin(pitchRad);
        double fz = Math.cos(yawRad) * cosPitch;
        double rx = -Math.cos(yawRad);
        double rz = -Math.sin(yawRad);

        double mx = 0, my = 0, mz = 0;
        if (down(window, GLFW.GLFW_KEY_W)) { mx += fx; my += fy; mz += fz; }
        if (down(window, GLFW.GLFW_KEY_S)) { mx -= fx; my -= fy; mz -= fz; }
        if (down(window, GLFW.GLFW_KEY_D)) { mx += rx; mz += rz; }
        if (down(window, GLFW.GLFW_KEY_A)) { mx -= rx; mz -= rz; }
        if (down(window, GLFW.GLFW_KEY_SPACE)) { my += 1.0; }
        if (down(window, GLFW.GLFW_KEY_LEFT_SHIFT)) { my -= 1.0; }

        double len = Math.sqrt(mx * mx + my * my + mz * mz);
        if (len < 1.0e-6) {
            return;
        }
        double step = (down(window, GLFW.GLFW_KEY_LEFT_CONTROL) ? FLY_SPEED * FLY_BOOST : FLY_SPEED)
                * dt / len;
        camX += mx * step;
        camY += my * step;
        camZ += mz * step;
    }

    /**
     * Hold the six movement keys down=false so the body never walks while the
     * free camera is active. With {@code release} the keys are instead set to
     * their real keyboard state — the one-shot resync on the falling edge.
     */
    private static void freezeBody(Minecraft mc, boolean release) {
        if (mc == null || mc.options == null) {
            return;
        }
        long window = (mc.getWindow() != null) ? mc.getWindow().handle() : 0L;
        hold(mc.options.keyUp, window, GLFW.GLFW_KEY_W, release);
        hold(mc.options.keyDown, window, GLFW.GLFW_KEY_S, release);
        hold(mc.options.keyLeft, window, GLFW.GLFW_KEY_A, release);
        hold(mc.options.keyRight, window, GLFW.GLFW_KEY_D, release);
        hold(mc.options.keyJump, window, GLFW.GLFW_KEY_SPACE, release);
        hold(mc.options.keyShift, window, GLFW.GLFW_KEY_LEFT_SHIFT, release);
    }

    /** Force {@code key} down=false, or (on release) to its real keyboard state. */
    private static void hold(KeyMapping key, long window, int code, boolean release) {
        key.setDown(release && window != 0L && GLFW.glfwGetKey(window, code) == GLFW.GLFW_PRESS);
    }

    /** Whether GLFW reports physical key {@code code} held on {@code window}. */
    private static boolean down(long window, int code) {
        return GLFW.glfwGetKey(window, code) == GLFW.GLFW_PRESS;
    }
}
