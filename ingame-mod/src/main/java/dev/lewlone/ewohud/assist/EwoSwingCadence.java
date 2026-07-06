package dev.lewlone.ewohud.assist;

import dev.lewlone.ewohud.EwoCompat;

import java.util.Random;

import dev.lewlone.ewohud.EwoModuleData;
import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.entity.TamableAnimal;
import net.minecraft.world.entity.monster.Enemy;
import net.minecraft.world.entity.npc.villager.Villager;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.phys.EntityHitResult;
import net.minecraft.world.phys.HitResult;

/**
 * Swing Cadence module.
 *
 * <p>Auto-fires the next swing as soon as the attack-strength meter fills
 * while the crosshair is on a living entity in reach. Doesn't aim — that's
 * still the user's job. Doesn't synthesise rotation packets or extend
 * vanilla reach.
 *
 * <p>Three layers of humanization sit on top of the bare auto-fire core:
 *
 * <ol>
 *   <li><b>Minimum inter-fire interval</b> ({@code min_interval_ms}, default
 *       200 ms ≈ 5 hits/sec cap). Bare every-tick autofire reads as a bot
 *       to cadence detectors; capped autofire at human-reachable cadence
 *       (5-6 hits/sec is on the upper edge of fast butterfly-clicking)
 *       does not.</li>
 *   <li><b>±jitter</b> ({@code jitter_ms}, default 30 ms). Re-randomized
 *       after each fire. Successive fires don't land on the same tick
 *       offset, so the cadence isn't bit-exact.</li>
 *   <li><b>Target-acquired reaction</b> ({@code reaction_ms}, default 80 ms).
 *       When a <i>new</i> entity enters the crosshair, wait this many ms
 *       before the first fire. Models human neuromuscular reaction time;
 *       a real player can't fire the instant a crosshair touches a target.</li>
 * </ol>
 *
 * <p>(Class identity: this is the renamed Triggerbot. Same behaviour, plus
 * the humanization above. The name change drops the obvious class-name
 * fingerprint a class-scan-driven AC would match on.)
 */
public final class EwoSwingCadence {

    private EwoSwingCadence() {}

    /** Wall-clock ms of the last auto-fire. */
    private static long lastFireMs;
    /** Last target seen under the crosshair — used to detect "new target". */
    private static Entity lastTarget;
    /** Wall-clock ms when the current target was first acquired (reaction-delay anchor). */
    private static long targetAcquiredAtMs;
    /** Current jitter offset on top of the min_interval — re-randomized after each fire. */
    private static int currentJitterMs;
    private static final Random RAND = new Random();

    public static void tick() {
        if (!EwoModuleData.enabled(AssistSlots.SWING_CADENCE)) {
            lastTarget = null;
            return;
        }

        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.options == null
                || mc.gameMode == null || EwoCompat.screen(mc) != null) {
            lastTarget = null;
            return;
        }

        // Require-attack-held mode: only fire while the user is holding
        // attack. Acts as a "ready-to-swing" gate so the bot doesn't
        // surprise-swing when you're not committed to combat.
        boolean requireHeld = EwoModuleData.setting(AssistSlots.SWING_CADENCE, 3) >= 0.5f;
        if (requireHeld && !mc.options.keyAttack.isDown()) {
            lastTarget = null;
            return;
        }

        HitResult hr = mc.hitResult;
        if (!(hr instanceof EntityHitResult ehr)) {
            lastTarget = null;
            return;
        }
        if (!(ehr.getEntity() instanceof LivingEntity target)) {
            lastTarget = null;
            return;
        }
        if (target == mc.player) {
            lastTarget = null;
            return;
        }

        LocalPlayer player = mc.player;
        float reach = EwoModuleData.setting(AssistSlots.SWING_CADENCE, 0);
        if (player.distanceTo(target) > reach) {
            // Out of reach — but don't clear lastTarget here. A target that
            // dips in-and-out of reach during combat is still "the same
            // target" — clearing here would re-trigger the reaction delay
            // every time they took a half-step back. We only clear when
            // the entity changes.
            return;
        }

        float minCharge = EwoModuleData.setting(AssistSlots.SWING_CADENCE, 1);
        if (player.getAttackStrengthScale(0f) < minCharge) return;

        int filter = (int) EwoModuleData.setting(AssistSlots.SWING_CADENCE, 2);
        if (!passesFilter(target, player, filter)) {
            lastTarget = null;
            return;
        }

        // Hurt-time awareness — while the target is in vanilla iframes a
        // fresh attack is a waste of a charged hit. Skip until the window
        // closes. (Don't clear lastTarget; same target, just paused.)
        if (target.hurtTime > 0) return;

        // Prefer-crits — when on, wait through the upward phase of a jump so
        // the next swing lands as a vanilla crit. Don't clear lastTarget;
        // the jump-ascent is a normal pause in the engagement.
        boolean preferCrits = EwoModuleData.setting(AssistSlots.SWING_CADENCE, 4) >= 0.5f;
        if (preferCrits
                && !player.onGround()
                && player.getDeltaMovement().y > 0.0) {
            return;
        }

        long now = System.currentTimeMillis();

        // Target-acquired reaction delay. When the entity under the crosshair
        // changes, anchor the new "target acquired" time and hold fire for
        // `reaction_ms`. This models a real player's neuromuscular delay —
        // they can't shoot the instant their crosshair drifts onto a target.
        if (target != lastTarget) {
            lastTarget = target;
            targetAcquiredAtMs = now;
            // Roll a fresh jitter offset for the first shot too — keeps the
            // first-shot timing from being deterministic across encounters.
            currentJitterMs = rollJitter();
        }
        float reactionMs = EwoModuleData.setting(AssistSlots.SWING_CADENCE, 7);
        if (now - targetAcquiredAtMs < (long) reactionMs) {
            return;
        }

        // Minimum inter-fire interval + ±jitter. The interval caps the fire
        // rate at a human-reachable cadence; the jitter breaks up the
        // tick-aligned regularity a cadence detector watches for.
        float minIntervalMs = EwoModuleData.setting(AssistSlots.SWING_CADENCE, 5);
        long requiredInterval = Math.max(0L, Math.round(minIntervalMs) + currentJitterMs);
        if (now - lastFireMs < requiredInterval) {
            return;
        }
        lastFireMs = now;
        // Re-roll jitter for the next inter-fire interval.
        currentJitterMs = rollJitter();

        mc.gameMode.attack(player, target);
        player.swing(InteractionHand.MAIN_HAND);
    }

    /** Random offset in ±jitter_ms range, or 0 if jitter is disabled. */
    private static int rollJitter() {
        float jitterMs = EwoModuleData.setting(AssistSlots.SWING_CADENCE, 6);
        if (jitterMs <= 0f) return 0;
        return Math.round((RAND.nextFloat() * 2f - 1f) * jitterMs);
    }

    private static boolean passesFilter(LivingEntity target, LocalPlayer player, int filter) {
        switch (filter) {
            case 1: // players only
                return target instanceof Player;
            case 2: // hostile mobs + players, skipping villagers + own pets
                if (target instanceof Player) return true;
                if (target instanceof Villager) return false;
                if (target instanceof TamableAnimal tame
                        && tame.getOwnerReference() != null
                        && tame.getOwnerReference().getUUID().equals(player.getUUID())) {
                    return false; // your own pet
                }
                return target instanceof Enemy;
            case 0:
            default: // any living entity
                return true;
        }
    }
}
