package dev.lewlone.ewohud.assist;

import dev.lewlone.ewohud.EwoHudNative;
import dev.lewlone.ewohud.EwoKeybinds;

/**
 * The assist (PvP / packet-touching) modules' per-frame driver + key router.
 *
 * <p>{@link EwoModules#tick} resolves this class via reflection at class-load
 * time; if it isn't on the classpath (legit build), the bridge is a no-op
 * and none of the assist code runs. Likewise {@link EwoModules#handleKeyPress}
 * delegates key-press routing here only for the assist-side modules.
 *
 * <p>This split means the assist package can be excluded from the legit jar
 * wholesale — its class names, slot constants, and behaviour vanish from
 * the runtime entirely.
 */
public final class EwoAssist {

    private EwoAssist() {}

    /** Module ids of the assist toggles (press flips enabled flag). */
    private static final String[] TOGGLE_IDS = {
        "auto_tool", "auto_totem", "hand_restock",
        "sprint_tap", "auto_eat",
        "auto_mace_swap", "auto_jump_reset",
        "reach_lock", "auto_hit_timing", "knockback_max",
        "swing_cadence"
    };
    /** Matching shared-buffer slot for each toggle id, in the same order. */
    private static final int[] TOGGLE_INDEX = {
        AssistSlots.AUTO_TOOL, AssistSlots.AUTO_TOTEM, AssistSlots.HAND_RESTOCK,
        AssistSlots.SPRINT_TAP, AssistSlots.AUTO_EAT,
        AssistSlots.AUTO_MACE_SWAP, AssistSlots.AUTO_JUMP_RESET,
        AssistSlots.REACH_LOCK, AssistSlots.AUTO_HIT_TIMING, AssistSlots.KNOCKBACK_MAX,
        AssistSlots.SWING_CADENCE
    };

    /** Per-frame tick. Drives every continuously-evaluating assist module
     *  plus the {@link EwoActionMotor} step pump. */
    public static void tick() {
        EwoAutoTool.tick();
        EwoAutoTotem.tick();
        EwoHandRestock.tick();
        EwoSprintTap.tick();
        EwoAutoEat.tick();
        EwoAutoMaceSwap.tick();
        EwoAutoJumpReset.tick();
        EwoReachLock.tick();
        EwoAutoHitTiming.tick();
        EwoSwingCadence.tick();
        EwoActionMotor.tick();
    }

    /** Route a key press into an assist toggle or trigger. Returns true if
     *  the key was consumed. */
    public static boolean handleKeyPress(int glfwKey) {
        for (int i = 0; i < TOGGLE_IDS.length; i++) {
            if (EwoKeybinds.code(TOGGLE_IDS[i]) == glfwKey) {
                EwoHudNative.nativeModuleToggle(TOGGLE_INDEX[i]);
                return true;
            }
        }
        if (glfwKey == 0) {
            return false;
        }
        if (EwoKeybinds.code("legit_elytra_swap") == glfwKey) {
            EwoLegitElytraSwap.trigger();
            return true;
        }
        if (EwoKeybinds.code("auto_pearl") == glfwKey) {
            EwoAutoPearl.trigger();
            return true;
        }
        if (EwoKeybinds.code("riptide_boost") == glfwKey) {
            EwoRiptideBoost.trigger();
            return true;
        }
        return false;
    }
}
