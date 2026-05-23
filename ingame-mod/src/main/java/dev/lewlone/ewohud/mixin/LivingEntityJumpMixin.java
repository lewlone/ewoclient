package dev.lewlone.ewohud.mixin;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.pvp.EwoJumpReset;
import net.minecraft.client.Minecraft;
import net.minecraft.world.entity.LivingEntity;

/**
 * Routes the local player's jump into the PvP-Utils jump-reset tracker.
 *
 * <p>26.x note: the method was {@code LivingEntity.jump()} in 1.21.x and was
 * renamed to {@code jumpFromGround()} in 26.x — that's the only API drift
 * needed for the port. Injects HEAD so the tracker sees the jump <i>before</i>
 * the engine starts integrating velocity for the frame, matching the source
 * mod's timing.
 */
@Mixin(LivingEntity.class)
public abstract class LivingEntityJumpMixin {

    @Inject(method = "jumpFromGround", at = @At("HEAD"))
    private void ewo$onJump(CallbackInfo ci) {
        LivingEntity self = (LivingEntity) (Object) this;
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player != self) {
            // Only the local player — never other entities jumping nearby.
            return;
        }
        EwoJumpReset.onJump(self.tickCount);
    }
}
