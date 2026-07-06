package dev.lewlone.ewohud.assist;

import dev.lewlone.ewohud.EwoCompat;

import dev.lewlone.ewohud.EwoModuleData;
import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;

/**
 * Sprint Tap module.
 *
 * <p>Vanilla's "knockback boost" rule: when you hit an entity while sprinting,
 * the server adds an extra knockback level to the hit AND consumes your
 * sprint client-side. Without a manual re-engage, your subsequent hits land
 * un-sprinted.
 *
 * <p>This module re-engages sprint on the very next tick after each attack
 * by calling {@code LocalPlayer.setSprinting(true)} — only when the player
 * is still holding forward. The {@code PlayerAttackAssistMixin} hands off
 * here on each attack edge.
 */
public final class EwoSprintTap {

    private EwoSprintTap() {}

    private static boolean retapPending;

    /** Called from {@code PlayerAttackAssistMixin} on each local-player attack. */
    public static void onAttack() {
        if (EwoModuleData.enabled(AssistSlots.SPRINT_TAP)) {
            retapPending = true;
        }
    }

    public static void tick() {
        if (!retapPending) {
            return;
        }
        retapPending = false;

        if (!EwoModuleData.enabled(AssistSlots.SPRINT_TAP)) {
            return;
        }
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.options == null) {
            return;
        }
        if (EwoCompat.screen(mc) != null) {
            return;
        }
        if (!mc.options.keyUp.isDown()) {
            return;
        }
        LocalPlayer player = mc.player;
        if (!player.isSprinting()) {
            player.setSprinting(true);
        }
    }
}
