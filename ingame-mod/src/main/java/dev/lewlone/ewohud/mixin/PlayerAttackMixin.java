package dev.lewlone.ewohud.mixin;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoComboTracker;
import dev.lewlone.ewohud.pvp.EwoHitRange;
import net.minecraft.client.Minecraft;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.Player;

/**
 * Routes the local player's left-click attack into the legit observer
 * handoffs — PvP-Utils hit-range tracker + Combo counter.
 *
 * <p>Pvp-build adds {@code assist.mixin.PlayerAttackAssistMixin} alongside
 * this one (multiple HEAD injects on the same method coexist); that mixin
 * handles Knockback Maximizer + Sprint Tap. This file stays legit so its
 * source can ship in both legit and pvp jars.
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
    }
}
