package dev.lewlone.ewohud.mixin;

import net.minecraft.client.DeltaTracker;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.client.gui.Hud;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoHudMod;
import dev.lewlone.ewohud.EwoHudNative;

/**
 * Suppresses the vanilla crosshair when the EwoClient custom crosshair is
 * enabled — <b>Minecraft 26.2+</b> variant.
 *
 * <p>26.2 split the old {@code Gui} class: {@code Gui} is now the screen /
 * overlay manager, and the HUD half (including
 * {@code extractCrosshair(GuiGraphicsExtractor, DeltaTracker)}, byte-identical
 * signature) moved to the new {@link Hud} class. Same inject, new home.
 * {@code GuiCrosshairMixin} covers the 26.1 line; {@code EwoMixinPlugin}
 * applies exactly one of the two per runtime.
 */
@Mixin(Hud.class)
public class GuiCrosshairMixin262 {

    @Inject(method = "extractCrosshair", at = @At("HEAD"), cancellable = true)
    private void ewo$suppressVanillaCrosshair(
            GuiGraphicsExtractor extractor,
            DeltaTracker deltaTracker,
            CallbackInfo ci) {
        // Only suppress once the cdylib is loaded + initialised — pre-init
        // frames still want the vanilla crosshair so the user isn't stuck
        // without one during boot.
        if (!EwoHudMod.nativeReady) {
            return;
        }
        if (EwoHudNative.nativeIsCustomCrosshairEnabled() != 0) {
            ci.cancel();
        }
    }
}
