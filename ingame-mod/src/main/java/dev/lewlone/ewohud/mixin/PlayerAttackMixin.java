package dev.lewlone.ewohud.mixin;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.pvp.EwoHitRange;
import net.minecraft.client.Minecraft;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.Player;

/**
 * Routes the local player's left-click attack into the PvP-Utils hit-range
 * tracker. Injects HEAD on {@code Player.attack(Entity)} so the distance is
 * sampled before the attack's velocity-modifying logic runs. Only fires for
 * the local client player — server-side or other-player attacks are ignored.
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
    }
}
