package dev.lewlone.ewohud;

import java.util.function.IntPredicate;

/**
 * Registration point for an optional module driver (the pvp build's).
 *
 * <p>Direction of dependency matters here: the driver registers <em>into</em>
 * this class, from its own client entrypoint, which only
 * {@code fabric-pvp.mod.json} declares. Non-driver code therefore never
 * mentions the driver's package, not even as a string for
 * {@code Class.forName}, and the legit jar contains no trace of it (the
 * post-ban refactor's goal; {@code ingame-mod/build.ps1} asserts it).
 *
 * <p>With nothing registered every method here is a no-op.
 */
public final class EwoExtensionHook {
    private EwoExtensionHook() {}

    private static volatile Runnable tick;
    private static volatile IntPredicate keyPress;

    /** Called once by the extension's entrypoint. */
    public static void register(Runnable perFrameTick, IntPredicate keyRouter) {
        tick = perFrameTick;
        keyPress = keyRouter;
    }

    /** Whether an extension registered itself. */
    static boolean active() {
        return tick != null;
    }

    /** Per-frame drive; a throwing extension is logged, never propagated. */
    static void tick() {
        Runnable t = tick;
        if (t == null) {
            return;
        }
        try {
            t.run();
        } catch (Throwable e) {
            System.err.println("[ewo-hud] extension tick failed: " + e);
        }
    }

    /** Route a key press to the extension; true if it consumed the key. */
    static boolean keyPress(int glfwKey) {
        IntPredicate k = keyPress;
        if (k == null) {
            return false;
        }
        try {
            return k.test(glfwKey);
        } catch (Throwable e) {
            System.err.println("[ewo-hud] extension key router failed: " + e);
            return false;
        }
    }
}
