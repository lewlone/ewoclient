package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.phys.EntityHitResult;
import net.minecraft.world.phys.HitResult;

/**
 * Phase H — Auto Hit Timing.
 *
 * <p>Vanilla 1.9+ attack-strength cooldown: each attack reduces damage based
 * on how recently the previous attack landed. Spam-clicking lands lots of
 * low-charge hits ({@code getAttackStrengthScale &lt;&lt; 1}); waiting for full
 * charge between clicks lands fewer but full-damage hits. The skill ceiling
 * is "perfect cadence" — clicking exactly when charge hits 1.0.
 *
 * <p>This module turns hold-attack into perfect-cadence auto-fire: while
 * {@code keyAttack} is held continuously and the crosshair has a living
 * entity, fires one attack each time {@code getAttackStrengthScale} reaches
 * the threshold. Each fire goes through {@code mc.gameMode.attack(player,
 * entity)} and {@code player.swing(MAIN_HAND)} — exactly what
 * {@code Minecraft.startAttack} calls on a real left-click.
 *
 * <p>Doesn't fight manual clicks — the cooldown reset is whoever swings
 * last (vanilla or us); the auto-fire just resumes from there.
 */
public final class EwoAutoHitTiming {

    private EwoAutoHitTiming() {}

    /** Frame on which we last auto-fired. Used to throttle to at most one
     *  auto-fire per game tick — keeps us from spamming if the threshold is
     *  set very low and {@code getAttackStrengthScale} happens to oscillate. */
    private static long lastFireMs;

    public static void tick() {
        if (!EwoModuleData.enabled(EwoModuleData.AUTO_HIT_TIMING)) {
            return;
        }

        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.options == null
                || mc.gameMode == null || mc.screen != null) {
            return;
        }
        // Only fire while the user is holding attack — single-clicks pass
        // through to vanilla untouched.
        if (!mc.options.keyAttack.isDown()) {
            return;
        }
        // Need a living entity under the crosshair to attack.
        HitResult hr = mc.hitResult;
        if (!(hr instanceof EntityHitResult ehr)
                || !(ehr.getEntity() instanceof LivingEntity)) {
            return;
        }

        LocalPlayer player = mc.player;
        float threshold = EwoModuleData.setting(EwoModuleData.AUTO_HIT_TIMING, 0);
        if (player.getAttackStrengthScale(0f) < threshold) {
            return;
        }

        // Throttle: one auto-fire per ~50 ms (one tick). Prevents double-
        // firing if a frame happens to read charge ≥ threshold twice in a row.
        long now = System.currentTimeMillis();
        if (now - lastFireMs < 60L) {
            return;
        }
        lastFireMs = now;

        // Real attack — same two calls Minecraft.startAttack makes on a
        // left-click against an entity. mc.gameMode.attack sends the
        // ServerboundInteractPacket + resets attack-strength.
        mc.gameMode.attack(player, ehr.getEntity());
        player.swing(InteractionHand.MAIN_HAND);
    }
}
