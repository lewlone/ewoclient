package dev.lewlone.ewohud.assist.mixin;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoModuleData;
import dev.lewlone.ewohud.assist.AssistSlots;
import dev.lewlone.ewohud.assist.EwoSprintTap;
import net.minecraft.client.Minecraft;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.Player;

/**
 * Pvp-only player-attack hook. Loaded via {@code ewohud-pvp.mixins.json},
 * which the {@code -Pvp} build of {@code ingame-mod} ships and the default
 * build does not.
 *
 * <p>Injects HEAD on {@code Player.attack(Entity)} alongside the legit
 * {@code PlayerAttackMixin} (multiple HEAD injects on the same method are
 * fine). This one handles the assist-specific bits:
 *
 * <ul>
 *   <li>Knockback Maximizer — if the module is on and the user is holding
 *       forward + not sprinting, force {@code setSprinting(true)} so this
 *       attack lands with the sprint flag and triggers vanilla's +1
 *       knockback level.</li>
 *   <li>{@link EwoSprintTap#onAttack} — flags a sprint re-engage for next
 *       tick so the next hit also gets vanilla's sprint-knockback boost.</li>
 * </ul>
 */
@Mixin(Player.class)
public abstract class PlayerAttackAssistMixin {

    @Inject(method = "attack", at = @At("HEAD"))
    private void ewo$onAssistAttack(Entity target, CallbackInfo ci) {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player != (Object) this) {
            return;
        }
        if (EwoModuleData.enabled(AssistSlots.KNOCKBACK_MAX)
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
