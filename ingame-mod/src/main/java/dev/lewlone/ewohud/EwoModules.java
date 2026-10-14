package dev.lewlone.ewohud;

import net.minecraft.client.KeyMapping;
import net.minecraft.client.Minecraft;

/**
 * Drives the legit module effects from the {@link EwoModuleData} channel and
 * delegates to the {@link EwoExtensionHook} driver when one is registered.
 *
 * <p>{@link #tick} runs once per frame from {@code EwoHudMixin}, after Rust
 * has refreshed the module-state block. The render-override modules (Full
 * Bright, FOV, No Damage Tilt, No View Bob) are mixins that read the channel
 * directly; this class drives the modules that need a per-frame nudge — at
 * present that's Toggle Sprint and Toggle Sneak. FreeLook polls itself.
 *
 * <p>The pvp build ships an extra client entrypoint (declared only in
 * {@code fabric-pvp.mod.json}) that registers its per-frame driver and key
 * router with {@link EwoExtensionHook}. Nothing here names that code, so the
 * legit jar carries no trace of it (no class names, no reflection strings);
 * with nothing registered the hook is a no-op.
 */
public final class EwoModules {
    private EwoModules() {}

    /** Guards the "channel live" line so it logs exactly once. */
    private static boolean announced;

    /** Whether this class is currently forcing the sprint / sneak key down, so
     *  it can release the key exactly once when the module is switched off. */
    private static boolean sprintForced;
    private static boolean sneakForced;

    /** Legit toggle modules — id paired with buffer index. FreeLook is
     *  excluded: its key is hold-to-activate (see {@code EwoFreeLook}), not
     *  a toggle. Toggles owned by a registered extension are routed via
     *  {@link EwoExtensionHook#keyPress}. */
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
                    + (EwoExtensionHook.active() ? " (pvp build)" : " (legit build)"));
        }
        applyMovement();
        EwoFreeLook.update();
        EwoExtensionHook.tick();
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
     * off to the registered extension, if any. Returns true if consumed.
     */
    public static boolean handleKeyPress(int glfwKey) {
        for (int i = 0; i < TOGGLE_IDS.length; i++) {
            if (EwoKeybinds.code(TOGGLE_IDS[i]) == glfwKey) {
                EwoHudNative.nativeModuleToggle(TOGGLE_INDEX[i]);
                return true;
            }
        }
        return EwoExtensionHook.keyPress(glfwKey);
    }
}
