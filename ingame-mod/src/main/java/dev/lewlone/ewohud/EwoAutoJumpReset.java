package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;

/**
 * Phase H — Auto Jump Reset module (sword PvP tech).
 *
 * <p>Jump-reset mechanic: when an opponent hits you while you're on the
 * ground, vanilla applies horizontal knockback. If you jump on the same
 * tick the hit lands, the jump's vertical impulse cancels most of the
 * horizontal knockback, leaving you nearly in place instead of getting
 * combo'd. Top-tier players time this manually; the perfect window is
 * sub-75ms wide, which is why the PvP-Utils "Jump Reset" indicator
 * exists to score your timing.
 *
 * <p>This module turns that into a macro: each frame, observe the local
 * player's health. On a drop (took damage) while on the ground and not in
 * water, force {@code keyJump.setDown(true)} for one tick — vanilla's
 * {@code aiStep} sees it on the next tick and calls {@code jumpFromGround()}.
 * Release the key the tick after, so the user's natural jump key state
 * isn't permanently overridden.
 *
 * <p>Won't fire underwater (jumping doesn't reset knockback when swimming)
 * or while an inventory/menu is open.
 */
public final class EwoAutoJumpReset {

    private EwoAutoJumpReset() {}

    /** Last frame's health — drop = took damage. -1 = no baseline yet. */
    private static float lastHealth = -1f;
    /** True the frame after we set keyJump down; release on the next tick. */
    private static boolean releasePending;

    public static void tick() {
        if (!EwoModuleData.enabled(EwoModuleData.AUTO_JUMP_RESET)) {
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

        // First — release the key one tick after we set it.
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
        // Jump-reset only applies on solid ground out of water. In other states
        // the impulse direction is wrong / disabled.
        if (!player.onGround() || player.isInWater() || player.isInLava()) {
            return;
        }

        // Don't fight an explicit user jump — if they're already pressing
        // jump, vanilla will jumpFromGround naturally next tick.
        if (mc.options.keyJump.isDown()) {
            return;
        }

        mc.options.keyJump.setDown(true);
        releasePending = true;
    }
}
