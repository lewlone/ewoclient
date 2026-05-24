package dev.lewlone.ewohud;

import net.minecraft.client.KeyMapping;
import net.minecraft.client.Minecraft;

/**
 * Applies the per-frame EwoClient module effects from the {@link EwoModuleData}
 * channel (Phase G).
 *
 * <p>{@link #tick()} runs once per frame from {@code EwoHudMixin}, after Rust
 * has refreshed the module-state block. The render-override modules (Full
 * Bright, FOV, No Damage Tilt, No View Bob) are mixins that read the channel
 * directly; this class drives the modules that need a per-frame nudge —
 * Toggle Sprint and Toggle Sneak.
 */
public final class EwoModules {
    private EwoModules() {}

    /** Guards the "channel live" line so it logs exactly once. */
    private static boolean announced;

    /** Whether this class is currently forcing the sprint / sneak key down, so
     *  it can release the key exactly once when the module is switched off. */
    private static boolean sprintForced;
    private static boolean sneakForced;

    /** Toggle modules — id paired with buffer index. FreeLook is excluded:
     *  its key is hold-to-activate (see {@code EwoFreeLook}), not a toggle. */
    private static final String[] TOGGLE_IDS = {
        "fullbright", "fov", "toggle_sprint", "toggle_sneak", "no_damage_tilt",
        "no_view_bob", "no_fire_overlay", "crosshair_on_reach",
        "auto_tool", "auto_totem", "hand_restock",
        "no_pumpkin_overlay", "hit_color",
        "sprint_tap", "auto_eat"
    };
    private static final int[] TOGGLE_INDEX = {
        EwoModuleData.FULLBRIGHT, EwoModuleData.FOV, EwoModuleData.TOGGLE_SPRINT,
        EwoModuleData.TOGGLE_SNEAK, EwoModuleData.NO_DAMAGE_TILT, EwoModuleData.NO_VIEW_BOB,
        EwoModuleData.NO_FIRE_OVERLAY, EwoModuleData.CROSSHAIR_ON_REACH,
        EwoModuleData.AUTO_TOOL, EwoModuleData.AUTO_TOTEM, EwoModuleData.HAND_RESTOCK,
        EwoModuleData.NO_PUMPKIN_OVERLAY, EwoModuleData.HIT_COLOR,
        EwoModuleData.SPRINT_TAP, EwoModuleData.AUTO_EAT
    };

    /** Per-frame module tick. Called from {@code flipFrame}, post-render. */
    public static void tick() {
        if (!announced && EwoModuleData.ready()) {
            announced = true;
            System.err.println("[ewo-hud] module channel live — schema "
                    + EwoModuleData.SCHEMA_VERSION + ", "
                    + EwoModuleData.moduleCount() + " modules");
        }
        applyMovement();
        EwoFreeLook.update();
        EwoAutoTool.tick();
        EwoAutoTotem.tick();
        EwoHandRestock.tick();
        EwoSprintTap.tick();
        EwoAutoEat.tick();
        EwoActionMotor.tick();
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

    /**
     * Force {@code key} down while {@code module} is enabled. Returns whether
     * the key is being forced; on the frame it changes from forced to not, the
     * key is released exactly once.
     */
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
     * If {@code glfwKey} is the keybind of a toggle module, flip that module
     * (via the native bridge — Rust owns module state) and return true.
     * Unbound modules report code 0, which never matches a real key. Called
     * from the keyboard handler on a key press.
     */
    public static boolean handleKeyPress(int glfwKey) {
        for (int i = 0; i < TOGGLE_IDS.length; i++) {
            if (EwoKeybinds.code(TOGGLE_IDS[i]) == glfwKey) {
                EwoHudNative.nativeModuleToggle(TOGGLE_INDEX[i]);
                return true;
            }
        }
        // Trigger modules — press fires the action once (no toggle). The
        // module's own enabled flag (settable in the launcher Modules tab)
        // gates whether the action actually runs; pressing while disabled
        // is just a no-op rather than a "toggle the disabled module on".
        if (glfwKey != 0 && EwoKeybinds.code("legit_elytra_swap") == glfwKey) {
            EwoLegitElytraSwap.trigger();
            return true;
        }
        return false;
    }
}
