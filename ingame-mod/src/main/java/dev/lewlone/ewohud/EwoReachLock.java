package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.phys.EntityHitResult;
import net.minecraft.world.phys.HitResult;

/**
 * Phase H — Reach Lock (spacing assist).
 *
 * <p>Sword PvP optimal spacing: just past your hit range, in your opponent's
 * dead-zone. Walking too close gives them an easier hit-back; staying just
 * outside their reach but inside yours is the sweet spot. The mechanic to
 * stay there is "release W when you've closed enough" — manually pulsing
 * forward is what skilled players do.
 *
 * <p>This module automates the release: each frame, if there's a living
 * entity under the crosshair AND its distance is at-or-below
 * {@code max_distance}, force {@code keyUp.setDown(false)}. The user can
 * still walk backward (S) or sideways (A/D); only forward is gated. When
 * they back off past the threshold or look elsewhere, the override clears
 * and W works normally again.
 *
 * <p>Most invasive macro in the catalog — it overrides direct user input.
 * Default-off; user has to opt in deliberately.
 */
public final class EwoReachLock {

    private EwoReachLock() {}

    /** True while we are forcing keyUp down/false; lets us know to release
     *  the override when conditions stop matching. */
    private static boolean overriding;

    public static void tick() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.options == null) {
            return;
        }

        boolean wantOverride = false;
        if (EwoModuleData.enabled(EwoModuleData.REACH_LOCK)
                && mc.player != null && mc.screen == null) {
            float maxDist = EwoModuleData.setting(EwoModuleData.REACH_LOCK, 0);
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
            // Always set every tick so a manual press doesn't sneak through —
            // the override is "you cannot move forward while in the dead-zone."
            mc.options.keyUp.setDown(false);
            overriding = true;
        } else if (overriding) {
            // Conditions stopped — drop the override and let vanilla's
            // KeyMapping sync from the next physical key event. Known
            // limitation: a user who was holding W through the override
            // (no release event for vanilla to sync from) feels W stuck
            // until they release+repress. Acceptable for v1; a proper fix
            // would query GLFW directly via an Accessor mixin on
            // KeyMapping's protected `key` field.
            overriding = false;
        }
    }
}
