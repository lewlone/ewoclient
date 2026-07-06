package dev.lewlone.ewohud.mixin;

import net.minecraft.client.renderer.LightmapRenderStateExtractor;
import net.minecraft.client.renderer.state.LightmapRenderState;
import org.joml.Vector3f;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoModuleData;

/**
 * Full Bright module (Phase G) — renders the world fully lit.
 *
 * <p>The 26.x lightmap is GPU-driven: {@code LightmapRenderStateExtractor}
 * fills a {@link LightmapRenderState} UBO that the lightmap shader consumes.
 * The shader computes {@code color = max(AmbientColor, nightVision) + sky +
 * block}, then {@code clamp(color, 0, 1)} before the brightness mix — so
 * forcing {@code ambientColor} to white makes every lightmap texel clamp to
 * exactly 1.0: fully lit, textures intact, nothing overdriven.
 *
 * <p>The previous implementation cranked {@code brightness = 15}. On 26.2
 * that field is a 0..1 mix factor toward a brightening curve, and 15 pushed
 * lightmap texels far past 1.0 — the whole world multiplied to clipped
 * white. The ambient-white approach reads identically on 26.1 and 26.2
 * (the state class is field-for-field the same) and replaces it on both.
 *
 * <p>{@code darknessEffectScale} is zeroed too — the shader subtracts it
 * after the ambient add, so the Darkness effect would otherwise still dim a
 * "fully bright" world. Vanilla {@code options.txt} is never written.
 */
@Mixin(LightmapRenderStateExtractor.class)
public class LightmapRenderStateExtractorMixin {

    private static final Vector3f EWO_FULL_BRIGHT_AMBIENT = new Vector3f(1.0f, 1.0f, 1.0f);

    @Inject(method = "extract", at = @At("RETURN"))
    private void ewo$fullbright(LightmapRenderState state, float partialTick, CallbackInfo ci) {
        if (EwoModuleData.enabled(EwoModuleData.FULLBRIGHT)) {
            state.ambientColor = EWO_FULL_BRIGHT_AMBIENT;
            state.darknessEffectScale = 0.0f;
        }
    }
}
