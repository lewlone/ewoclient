package dev.lewlone.ewohud.mixin;

import net.minecraft.client.Minecraft;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoHudMod;

/**
 * 26.2+ twin of {@link MinecraftShutdownMixin}: arms the exit watchdog at the
 * game's final teardown.
 *
 * <p>26.2 removed {@code Minecraft.destroy()}; {@code Main} now calls
 * {@code exitWorldAndClose()} once the run loop ends. It is the same method
 * under a new name (disconnect the level, then {@code finally close()} the
 * renderer, sound, window and GLFW), so RETURN here is the same point 26.1's
 * {@code destroy()} RETURN was: after every resource is released and just
 * before the JVM begins its own native shutdown, where the
 * {@code DLL_PROCESS_DETACH} deadlock bites.
 *
 * <p>Hooking {@code close()} instead would arm the 3 s kill timer at its
 * start, racing the teardown it is meant to guard; {@code stop()} only clears
 * the {@code running} flag mid-session (the pause/exit button), so it is far
 * too early. {@code require = 0} for the same reason as the 26.1 mixin: a
 * future rename degrades to the shutdown-hook backstop rather than failing
 * the config (and {@code tools/check_mixin_targets.py} catches the rename).
 */
@Mixin(Minecraft.class)
public class MinecraftShutdownMixin262 {

    @Inject(method = "exitWorldAndClose", at = @At("RETURN"), require = 0)
    private void ewo$armExitWatchdog(CallbackInfo ci) {
        EwoHudMod.armExitWatchdog();
    }
}
