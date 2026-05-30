package dev.lewlone.ewohud.assist;

import dev.lewlone.ewohud.EwoModuleData;
import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;

/**
 * Auto Jump Reset module (sword PvP tech).
 *
 * <p>Jump-reset mechanic: when an opponent hits you while you're on the
 * ground, vanilla applies horizontal knockback. If you jump on the same
 * tick the hit lands, the jump's vertical impulse cancels most of the
 * horizontal knockback. Top-tier players time this manually; the perfect
 * window is sub-75ms wide.
 *
 * <p>This module turns that into a macro: each frame, observe the local
 * player's health. On a drop while on the ground and not in water, force
 * {@code keyJump.setDown(true)} for one tick — vanilla's {@code aiStep} sees
 * it on the next tick and calls {@code jumpFromGround()}.
 *
 * <p>Won't fire underwater (jumping doesn't reset knockback when swimming)
 * or while an inventory/menu is open.
 */
public final class EwoAutoJumpReset {

    private EwoAutoJumpReset() {}

    private static float lastHealth = -1f;
    private static boolean releasePending;

    public static void tick() {
        if (!EwoModuleData.enabled(AssistSlots.AUTO_JUMP_RESET)) {
            lastHealth = -1f;
            return;
        }

        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.options == null) {
            lastHealth = -1f;
            return;
        }
        if (mc.screen != null) {
            return;
        }

        LocalPlayer player = mc.player;

        if (releasePending) {
            mc.options.keyJump.setDown(false);
            releasePending = false;
        }

        float health = player.getHealth();
        boolean healthDropped = lastHealth > 0f && health + 0.05f < lastHealth;
        lastHealth = health;

        if (!healthDropped) {
            return;
        }
        if (!player.onGround() || player.isInWater() || player.isInLava()) {
            return;
        }

        if (mc.options.keyJump.isDown()) {
            return;
        }

        mc.options.keyJump.setDown(true);
        releasePending = true;
    }
}
