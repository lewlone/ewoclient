package dev.lewlone.ewohud;

import java.lang.reflect.Method;
import java.util.function.IntPredicate;

import net.minecraft.client.KeyMapping;
import net.minecraft.client.Minecraft;

/**
 * Drives the legit module effects from the {@link EwoModuleData} channel and
 * bridges to the assist (PvP) driver when present.
 *
 * <p>{@link #tick} runs once per frame from {@code EwoHudMixin}, after Rust
 * has refreshed the module-state block. The render-override modules (Full
 * Bright, FOV, No Damage Tilt, No View Bob) are mixins that read the channel
 * directly; this class drives the modules that need a per-frame nudge — at
 * present that's Toggle Sprint and Toggle Sneak. FreeLook polls itself.
 *
 * <p>If the assist package is on the classpath (pvp build), an
 * {@link #ASSIST_TICK} runnable + {@link #ASSIST_KEYPRESS} predicate are
 * resolved at class-load time via reflection, and the legit side delegates
 * to them. In the legit build the assist class is absent, both fields
 * resolve to {@code null}, and none of the assist code runs.
 */
public final class EwoModules {
    private EwoModules() {}

    /** Guards the "channel live" line so it logs exactly once. */
    private static boolean announced;

    /** Whether this class is currently forcing the sprint / sneak key down, so
     *  it can release the key exactly once when the module is switched off. */
    private static boolean sprintForced;
    private static boolean sneakForced;

    /** Assist bridge — resolved at class load via reflection, {@code null}
     *  when {@code dev.lewlone.ewohud.assist.EwoAssist} isn't on the
     *  classpath (legit build). */
    private static final Runnable ASSIST_TICK = resolveAssistTick();
    private static final IntPredicate ASSIST_KEYPRESS = resolveAssistKeyPress();

    /** Legit toggle modules — id paired with buffer index. FreeLook is
     *  excluded: its key is hold-to-activate (see {@code EwoFreeLook}), not
     *  a toggle. Assist toggles live on the assist driver and are routed
     *  via {@link #ASSIST_KEYPRESS}. */
    private static final String[] TOGGLE_IDS = {
        "fullbright", "fov", "toggle_sprint", "toggle_sneak",
        "no_damage_tilt", "no_view_bob",
        "no_fire_overlay", "crosshair_on_reach",
        "no_pumpkin_overlay", "hit_color",
        "hit_indicator"
    };
    private static final int[] TOGGLE_INDEX = {
        EwoModuleData.FULLBRIGHT, EwoModuleData.FOV,
        EwoModuleData.TOGGLE_SPRINT, EwoModuleData.TOGGLE_SNEAK,
        EwoModuleData.NO_DAMAGE_TILT, EwoModuleData.NO_VIEW_BOB,
        EwoModuleData.NO_FIRE_OVERLAY, EwoModuleData.CROSSHAIR_ON_REACH,
        EwoModuleData.NO_PUMPKIN_OVERLAY, EwoModuleData.HIT_COLOR,
        EwoModuleData.HIT_INDICATOR
    };

    /** Per-frame module tick. Called from {@code flipFrame}, post-render. */
    public static void tick() {
        if (!announced && EwoModuleData.ready()) {
            announced = true;
            System.err.println("[ewo-hud] module channel live — schema "
                    + EwoModuleData.SCHEMA_VERSION + ", "
                    + EwoModuleData.moduleCount() + " modules"
                    + (ASSIST_TICK == null ? " (legit build)" : " (pvp build)"));
        }
        applyMovement();
        EwoFreeLook.update();
        if (ASSIST_TICK != null) {
            ASSIST_TICK.run();
        }
    }

    /**
     * Toggle Sprint / Toggle Sneak — hold the movement key down for the player
     * while the module is on, and release it once when the module turns off so
     * the key never sticks.
     */
    private static void applyMovement() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.options == null) {
            return;
        }
        sprintForced = hold(mc.options.keySprint, EwoModuleData.TOGGLE_SPRINT, sprintForced);
        sneakForced = hold(mc.options.keyShift, EwoModuleData.TOGGLE_SNEAK, sneakForced);
    }

    private static boolean hold(KeyMapping key, int module, boolean wasForced) {
        if (EwoModuleData.enabled(module)) {
            key.setDown(true);
            return true;
        }
        if (wasForced) {
            key.setDown(false);
        }
        return false;
    }

    /**
     * If {@code glfwKey} matches the keybind of a legit toggle module, flip
     * it (via the native bridge — Rust owns module state). Otherwise hand
     * off to the assist bridge if present. Returns true if consumed.
     */
    public static boolean handleKeyPress(int glfwKey) {
        for (int i = 0; i < TOGGLE_IDS.length; i++) {
            if (EwoKeybinds.code(TOGGLE_IDS[i]) == glfwKey) {
                EwoHudNative.nativeModuleToggle(TOGGLE_INDEX[i]);
                return true;
            }
        }
        if (ASSIST_KEYPRESS != null) {
            return ASSIST_KEYPRESS.test(glfwKey);
        }
        return false;
    }

    /** Resolves {@code EwoAssist.tick()} via reflection. Returns {@code null}
     *  in a legit build where the assist class isn't on the classpath. */
    private static Runnable resolveAssistTick() {
        try {
            Class<?> cls = Class.forName("dev.lewlone.ewohud.assist.EwoAssist");
            Method m = cls.getMethod("tick");
            return () -> {
                try {
                    m.invoke(null);
                } catch (Throwable t) {
                    System.err.println("[ewo-hud] assist tick failed: " + t);
                }
            };
        } catch (ClassNotFoundException ignored) {
            // Legit build — no assist driver. Fine.
            return null;
        } catch (NoSuchMethodException e) {
            System.err.println("[ewo-hud] EwoAssist present but missing tick(): " + e);
            return null;
        }
    }

    /** Resolves {@code EwoAssist.handleKeyPress(int)} via reflection. */
    private static IntPredicate resolveAssistKeyPress() {
        try {
            Class<?> cls = Class.forName("dev.lewlone.ewohud.assist.EwoAssist");
            Method m = cls.getMethod("handleKeyPress", int.class);
            return key -> {
                try {
                    Object res = m.invoke(null, key);
                    return res instanceof Boolean b && b;
                } catch (Throwable t) {
                    System.err.println("[ewo-hud] assist handleKeyPress failed: " + t);
                    return false;
                }
            };
        } catch (ClassNotFoundException ignored) {
            return null;
        } catch (NoSuchMethodException e) {
            System.err.println("[ewo-hud] EwoAssist present but missing handleKeyPress(int): " + e);
            return null;
        }
    }
}
