package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.entity.TamableAnimal;
import net.minecraft.world.entity.monster.Enemy;
import net.minecraft.world.entity.npc.villager.Villager;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.phys.EntityHitResult;
import net.minecraft.world.phys.HitResult;

/**
 * Phase H — Triggerbot.
 *
 * <p>Fully autonomous melee: each frame, if the crosshair is on a living
 * entity, within reach, and our attack-strength is at-or-above the
 * threshold, fires a real attack via {@code mc.gameMode.attack(player,
 * entity)} + {@code player.swing}. <i>Doesn't aim</i> — that's still the
 * user's responsibility. Doesn't synthesise rotation packets or extend
 * vanilla reach.
 *
 * <p>Target filter (vanilla classification only):
 * <ul>
 *   <li>0 = any living entity</li>
 *   <li>1 = players only</li>
 *   <li>2 = hostile mobs + players (default) — Monsters (via {@link Enemy}),
 *       skipping {@link Villager} and tamed {@link TamableAnimal} owned by
 *       the local player</li>
 * </ul>
 *
 * <p>Server can't distinguish from a really good human clicker — every
 * packet sent is the same as a real left-click. Intended for semi-anarchy
 * use; public competitive servers (Hypixel etc.) detect cadence + ban.
 */
public final class EwoTriggerbot {

    private EwoTriggerbot() {}

    /** Wall-clock ms of the last auto-fire — throttle to one shot per tick. */
    private static long lastFireMs;

    public static void tick() {
        if (!EwoModuleData.enabled(EwoModuleData.TRIGGERBOT)) {
            return;
        }

        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.options == null
                || mc.gameMode == null || mc.screen != null) {
            return;
        }

        // Require-attack-held mode: only fire while the user is holding
        // attack. Acts as a "ready-to-swing" gate so the bot doesn't
        // surprise-swing when you're not committed to combat.
        boolean requireHeld = EwoModuleData.setting(EwoModuleData.TRIGGERBOT, 3) >= 0.5f;
        if (requireHeld && !mc.options.keyAttack.isDown()) {
            return;
        }

        HitResult hr = mc.hitResult;
        if (!(hr instanceof EntityHitResult ehr)) return;
        if (!(ehr.getEntity() instanceof LivingEntity target)) return;
        if (target == mc.player) return;

        LocalPlayer player = mc.player;
        float reach = EwoModuleData.setting(EwoModuleData.TRIGGERBOT, 0);
        if (player.distanceTo(target) > reach) return;

        float minCharge = EwoModuleData.setting(EwoModuleData.TRIGGERBOT, 1);
        if (player.getAttackStrengthScale(0f) < minCharge) return;

        int filter = (int) EwoModuleData.setting(EwoModuleData.TRIGGERBOT, 2);
        if (!passesFilter(target, player, filter)) return;

        long now = System.currentTimeMillis();
        if (now - lastFireMs < 60L) return; // one shot per tick max
        lastFireMs = now;

        mc.gameMode.attack(player, target);
        player.swing(InteractionHand.MAIN_HAND);
    }

    /** Filter the target against the configured class set. */
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
