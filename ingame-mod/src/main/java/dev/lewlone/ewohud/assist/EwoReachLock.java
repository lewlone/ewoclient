package dev.lewlone.ewohud.assist;

import dev.lewlone.ewohud.EwoCompat;

import dev.lewlone.ewohud.EwoModuleData;
import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.phys.EntityHitResult;
import net.minecraft.world.phys.HitResult;

/**
 * Reach Lock module (spacing assist).
 *
 * <p>Sword PvP optimal spacing: just past your hit range, in your opponent's
 * dead-zone. Walking too close gives them an easier hit-back; staying just
 * outside their reach but inside yours is the sweet spot.
 *
 * <p>This module automates the release: each frame, if there's a living
 * entity under the crosshair AND its distance is at-or-below
 * {@code max_distance}, force {@code keyUp.setDown(false)}. The user can
 * still walk backward (S) or sideways (A/D); only forward is gated.
 *
 * <p>Most invasive assist module — it overrides direct user input.
 * Default-off; user has to opt in deliberately.
 */
public final class EwoReachLock {

    private EwoReachLock() {}

    private static boolean overriding;

    public static void tick() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.options == null) {
            return;
        }

        boolean wantOverride = false;
        if (EwoModuleData.enabled(AssistSlots.REACH_LOCK)
                && mc.player != null && EwoCompat.screen(mc) == null) {
            float maxDist = EwoModuleData.setting(AssistSlots.REACH_LOCK, 0);
            HitResult hr = mc.hitResult;
            if (hr instanceof EntityHitResult ehr
                    && ehr.getEntity() instanceof LivingEntity le) {
                LocalPlayer player = mc.player;
                if (player.distanceTo(le) <= maxDist) {
                    wantOverride = true;
                }
            }
        }

        if (wantOverride) {
            mc.options.keyUp.setDown(false);
            overriding = true;
        } else if (overriding) {
            overriding = false;
        }
    }
}
