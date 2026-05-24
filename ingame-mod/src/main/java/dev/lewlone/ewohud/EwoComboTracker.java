package dev.lewlone.ewohud;

import net.minecraft.client.player.LocalPlayer;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.LivingEntity;

/**
 * Consecutive-hit counter for the Combo Counter HUD widget.
 *
 * <p>Increments on each {@link Player#attack} that hits a {@link LivingEntity}
 * (wired from {@link mixin.PlayerAttackMixin}). Resets to zero on:
 * <ul>
 *   <li>The local player taking damage (health drop detected per-frame)</li>
 *   <li>{@link #TIMEOUT_MS} elapsed since the last hit — the combo "expires"
 *       if you stop attacking</li>
 * </ul>
 *
 * <p>Pure static state — the count is read out per-frame from
 * {@link EwoHudData#capture} and written to the shared block for the Rust
 * widget to render.
 */
public final class EwoComboTracker {

    private EwoComboTracker() {}

    /** Combo resets if this many ms pass with no hit landed. */
    private static final long TIMEOUT_MS = 5000L;

    private static int count = 0;
    private static long lastHitMs = 0L;
    /** Health last frame — for the "took damage = reset" trigger. */
    private static float lastHealth = -1f;

    /** PlayerAttackMixin hands off to here for each local-player attack. */
    public static void onAttack(Entity target) {
        if (target instanceof LivingEntity) {
            count++;
            lastHitMs = System.currentTimeMillis();
        }
    }

    /** Per-frame tick — checks the two reset conditions. */
    public static void tick(LocalPlayer player) {
        if (player == null) {
            count = 0;
            lastHealth = -1f;
            return;
        }
        float health = player.getHealth();
        if (lastHealth > 0f && health + 0.05f < lastHealth) {
            count = 0;
        }
        lastHealth = health;
        long now = System.currentTimeMillis();
        if (lastHitMs > 0L && now - lastHitMs > TIMEOUT_MS) {
            count = 0;
        }
    }

    public static int count() {
        return count;
    }

    public static float ageSec() {
        if (lastHitMs == 0L) {
            return 99f;
        }
        return (System.currentTimeMillis() - lastHitMs) / 1000f;
    }
}
