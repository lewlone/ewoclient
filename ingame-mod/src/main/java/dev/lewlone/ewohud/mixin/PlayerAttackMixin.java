package dev.lewlone.ewohud.mixin;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoComboTracker;
import dev.lewlone.ewohud.EwoModuleData;
import dev.lewlone.ewohud.EwoSprintTap;
import dev.lewlone.ewohud.pvp.EwoHitRange;
import net.minecraft.client.Minecraft;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.Player;

/**
 * Routes the local player's left-click attack into per-feature handlers.
 *
 * <p>Injects HEAD on {@code Player.attack(Entity)} so the hit-range distance
 * is sampled and the Knockback Maximizer sprint-engage runs <i>before</i>
 * vanilla's attack code consumes the sprint flag. Only fires for the local
 * client player — server-side or other-player attacks are ignored.
 *
 * <p>Hands off to:
 * <ul>
 *   <li>{@code EwoHitRange.onAttack} — PvP-Utils hit-range tracker</li>
 *   <li>Knockback Maximizer (inline) — if the module is on and the user is
 *       holding forward + not sprinting, force {@code setSprinting(true)} so
 *       this attack lands with the sprint flag and triggers vanilla's +1
 *       knockback level. Sprint Tap covers <i>subsequent</i> hits; this
 *       covers the leading edge.</li>
 *   <li>{@code EwoSprintTap.onAttack} — flags a sprint re-engage for next
 *       tick so the next hit also gets vanilla's sprint-knockback boost</li>
 * </ul>
 */
@Mixin(Player.class)
public abstract class PlayerAttackMixin {

    @Inject(method = "attack", at = @At("HEAD"))
    private void ewo$onAttack(Entity target, CallbackInfo ci) {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player != (Object) this) {
            return;
        }
        EwoHitRange.onAttack(target);
        EwoComboTracker.onAttack(target);
        if (EwoModuleData.enabled(EwoModuleData.KNOCKBACK_MAX)
                && mc.options != null && mc.options.keyUp.isDown()
                && !mc.player.isSprinting()) {
            // Vanilla reads isSprinting() inside attack() to decide if the
            // +1 knockback level applies; setting it here at HEAD means the
            // very next instruction sees the engaged state.
            mc.player.setSprinting(true);
        }
        EwoSprintTap.onAttack();
    }
}
