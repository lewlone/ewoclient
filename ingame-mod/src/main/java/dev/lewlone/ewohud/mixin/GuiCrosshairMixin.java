package dev.lewlone.ewohud.mixin;

import net.minecraft.client.DeltaTracker;
import net.minecraft.client.gui.Gui;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoHudMod;
import dev.lewlone.ewohud.EwoHudNative;

/**
 * Suppresses the vanilla crosshair when the EwoClient custom crosshair is
 * enabled. Our crosshair is drawn into the offscreen Skia surface by Rust
 * and composited onto the framebuffer; vanilla's stays out of the way so
 * the two don't stack.
 *
 * <p>26.x's GUI rendering moved through {@code Gui.extractRenderState} →
 * {@code Gui.extractCrosshair} (the new render-state pipeline replaced the
 * old direct {@code renderCrosshair} call). Cancelling the extract stops
 * the crosshair primitive from being added to the render state at all.
 *
 * <p>The flag is read through the JNI bridge ({@code nativeIsCustomCrosshairEnabled})
 * each frame — Rust owns the config (it's the same field the editor mutates
 * in the CROSSHAIR overlay tab), so the mixin always sees the live state
 * without a buffer schema change. Cheap call: one boolean check, no heap.
 */
@Mixin(Gui.class)
public class GuiCrosshairMixin {

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
