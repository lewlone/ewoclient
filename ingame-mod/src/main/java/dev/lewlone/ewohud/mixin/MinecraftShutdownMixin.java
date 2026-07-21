package dev.lewlone.ewohud.mixin;

import net.minecraft.client.Minecraft;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoHudMod;

/**
 * Primary arm point for the exit watchdog.
 *
 * <p>{@code Minecraft.destroy()} is the game's final resource teardown — it
 * runs after the render loop ends and just <em>before</em> the JVM begins
 * its own native shutdown, which is where the {@code DLL_PROCESS_DETACH}
 * deadlock that spawns a zombie {@code java.exe} bites (see
 * {@link EwoHudMod#armExitWatchdog}). Arming the kill-timer here starts it
 * sooner than the JVM shutdown hook can, and also catches a hang that
 * strikes before shutdown hooks run at all.
 *
 * <p>{@code require = 0}: if a future MC renames {@code destroy()} this
 * simply no-ops rather than failing the whole mixin config — the shutdown
 * hook and the launcher-side reaper remain as backstops.
 */
@Mixin(Minecraft.class)
public class MinecraftShutdownMixin {

    @Inject(method = "destroy", at = @At("RETURN"), require = 0)
    private void ewo$armExitWatchdog(CallbackInfo ci) {
        EwoHudMod.armExitWatchdog();
    }
}
