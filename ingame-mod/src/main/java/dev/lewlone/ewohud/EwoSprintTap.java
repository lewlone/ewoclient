package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;

/**
 * Phase H — Sprint Tap module.
 *
 * <p>Vanilla's "knockback boost" rule: when you hit an entity while sprinting,
 * the server adds an extra knockback level to the hit AND consumes your
 * sprint client-side (sets {@code isSprinting() = false}). Without a manual
 * re-engage, your subsequent hits land un-sprinted until you release+
 * repress the sprint key (or double-tap forward).
 *
 * <p>This module re-engages sprint on the very next tick after each attack
 * by calling {@code LocalPlayer.setSprinting(true)} — only when the player
 * is still holding forward, so we don't engage sprint they don't want.
 * That's the same call vanilla makes when its own sprint-trigger logic
 * decides you should be sprinting; the next outgoing movement packet
 * carries the sprint flag, the server sees it, the next hit gets the boost.
 *
 * <p>{@link PlayerAttackMixin} hands off here on each attack edge.
 */
public final class EwoSprintTap {

    private EwoSprintTap() {}

    /** True after an attack, until the next {@link #tick()} runs the re-engage. */
    private static boolean retapPending;

    /** Called from {@code PlayerAttackMixin} on each local-player attack. */
    public static void onAttack() {
        if (EwoModuleData.enabled(EwoModuleData.SPRINT_TAP)) {
            retapPending = true;
        }
    }

    /** Per-frame check — runs the deferred re-engage if one is pending and
     *  the player is still pressing forward. Bail conditions reset the flag
     *  so the next attack starts clean. */
    public static void tick() {
        if (!retapPending) {
            return;
        }
        retapPending = false;

        if (!EwoModuleData.enabled(EwoModuleData.SPRINT_TAP)) {
            return;
        }
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.options == null) {
            return;
        }
        if (mc.screen != null) {
            return;
        }
        // Only re-engage if user is actually trying to keep sprinting —
        // forward key held. Otherwise leave them in whatever state they want.
        if (!mc.options.keyUp.isDown()) {
            return;
        }
        LocalPlayer player = mc.player;
        if (!player.isSprinting()) {
            player.setSprinting(true);
        }
    }
}
