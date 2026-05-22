package dev.lewlone.ewohud.mixin;

import net.minecraft.client.renderer.LightmapRenderStateExtractor;
import net.minecraft.client.renderer.state.LightmapRenderState;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoModuleData;

/**
 * Full Bright module (Phase G) — renders the world fully lit.
 *
 * <p>The 26.x lightmap is GPU-driven: {@code LightmapRenderStateExtractor}
 * fills a {@link LightmapRenderState} whose {@code brightness} field — derived
 * from the gamma option — feeds the lightmap shader. After the extract, this
 * cranks {@code brightness} well past the vanilla "Bright" maximum of 1, so
 * the shader lifts every light level to full: the gamma-slider trick,
 * uncapped. Vanilla {@code options.txt} is never written.
 */
@Mixin(LightmapRenderStateExtractor.class)
public class LightmapRenderStateExtractorMixin {

    /** Effective gamma when Full Bright is on — far past the vanilla cap of 1. */
    private static final float FULL_BRIGHT_LEVEL = 15.0f;

    @Inject(method = "extract", at = @At("RETURN"))
    private void ewo$fullbright(LightmapRenderState state, float partialTick, CallbackInfo ci) {
        if (EwoModuleData.enabled(EwoModuleData.FULLBRIGHT)) {
            state.brightness = FULL_BRIGHT_LEVEL;
        }
    }
}
