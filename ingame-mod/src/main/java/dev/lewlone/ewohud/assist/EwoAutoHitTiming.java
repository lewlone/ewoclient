package dev.lewlone.ewohud.assist;

import dev.lewlone.ewohud.EwoModuleData;
import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.phys.EntityHitResult;
import net.minecraft.world.phys.HitResult;

/**
 * Auto Hit Timing module.
 *
 * <p>Vanilla 1.9+ attack-strength cooldown: each attack reduces damage based
 * on how recently the previous attack landed. Spam-clicking lands lots of
 * low-charge hits; waiting for full charge lands fewer but full-damage hits.
 *
 * <p>This module turns hold-attack into perfect-cadence auto-fire: while
 * {@code keyAttack} is held continuously and the crosshair has a living
 * entity, fires one attack each time {@code getAttackStrengthScale} reaches
 * the threshold. Each fire goes through {@code mc.gameMode.attack(player,
 * entity)} and {@code player.swing(MAIN_HAND)} — exactly what
 * {@code Minecraft.startAttack} calls on a real left-click.
 */
public final class EwoAutoHitTiming {

    private EwoAutoHitTiming() {}

    private static long lastFireMs;

    public static void tick() {
        if (!EwoModuleData.enabled(AssistSlots.AUTO_HIT_TIMING)) {
            return;
        }

        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.options == null
                || mc.gameMode == null || mc.screen != null) {
            return;
        }
        if (!mc.options.keyAttack.isDown()) {
            return;
        }
        HitResult hr = mc.hitResult;
        if (!(hr instanceof EntityHitResult ehr)
                || !(ehr.getEntity() instanceof LivingEntity)) {
            return;
        }

        LocalPlayer player = mc.player;
        float threshold = EwoModuleData.setting(AssistSlots.AUTO_HIT_TIMING, 0);
        if (player.getAttackStrengthScale(0f) < threshold) {
            return;
        }

        long now = System.currentTimeMillis();
        if (now - lastFireMs < 60L) {
            return;
        }
        lastFireMs = now;

        mc.gameMode.attack(player, ehr.getEntity());
        player.swing(InteractionHand.MAIN_HAND);
    }
}
