package dev.lewlone.ewohud.mixin;

import net.minecraft.world.entity.LivingEntity;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoIndicators;

/**
 * Forwards LivingEntity status events to {@link EwoIndicators} so the totem-
 * pop counter can tick per-entity (status 35 = totem-of-undying activation).
 *
 * <p>{@code handleEntityEvent(byte)} fires on every entity for every entity-
 * status packet — totem use, hurt, death, particle ticks, all of them. We only
 * forward status 35; everything else is ignored by {@code EwoIndicators}.
 * Non-cancellable HEAD inject: vanilla still runs its own handler (animation,
 * sound) untouched.
 */
@Mixin(LivingEntity.class)
public abstract class LivingEntityStatusMixin {

    @Inject(method = "handleEntityEvent", at = @At("HEAD"))
    private void ewo$captureStatus(byte status, CallbackInfo ci) {
        if (status == 35) {
            LivingEntity self = (LivingEntity) (Object) this;
            EwoIndicators.onEntityStatus(self.getId(), status);
        }
    }
}
