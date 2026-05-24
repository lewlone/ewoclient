package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;

/**
 * Phase H — Auto Crit module.
 *
 * <p>Vanilla critical-hit rule: a melee attack lands as a crit (+50% damage)
 * when the attacker is mid-fall (vertical velocity &lt; 0) and not in water /
 * on a ladder / etc. The PvP technique is to jump-tap before each hit so the
 * swing connects on the descent. This module just holds the jump key down
 * for as long as the attack key is held — vanilla's {@code aiStep}
 * jump-cooldown (~10 ticks) gives you a bunny-hop cadence that lines up
 * with attack-strength cooldowns, so most hits land on the descent.
 *
 * <p>Notes:
 * <ul>
 *   <li>The bunny-hop is visible to other players; this is the most macro-
 *       looking module in the catalog.</li>
 *   <li>We only override the jump key when the user isn't already pressing
 *       it — manual jumps stay theirs.</li>
 *   <li>Release is paired with our own forced-down state so we never release
 *       a key the user was holding themselves.</li>
 * </ul>
 */
public final class EwoAutoCrit {

    private EwoAutoCrit() {}

    /** True while we're holding the jump key down for the user. */
    private static boolean forcedJump;

    public static void tick() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.options == null) {
            return;
        }

        boolean wantForce = EwoModuleData.enabled(EwoModuleData.AUTO_CRIT)
                && mc.player != null
                && mc.screen == null
                && mc.options.keyAttack.isDown();

        if (wantForce) {
            // Engage only if the user isn't already jumping themselves. If
            // they are, vanilla handles the jump and we stay out of the way.
            if (!forcedJump && !mc.options.keyJump.isDown()) {
                mc.options.keyJump.setDown(true);
                forcedJump = true;
            }
        } else if (forcedJump) {
            mc.options.keyJump.setDown(false);
            forcedJump = false;
        }
    }
}
