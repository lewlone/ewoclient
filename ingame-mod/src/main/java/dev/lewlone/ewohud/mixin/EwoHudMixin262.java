package dev.lewlone.ewohud.mixin;

import net.minecraft.client.Minecraft;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import dev.lewlone.ewohud.EwoFrameHook;

/**
 * Drives the in-game HUD paint once per frame — <b>Minecraft 26.2+</b>.
 *
 * <p>26.2 deleted {@code RenderSystem.flipFrame} and moved end-of-frame into
 * {@code Minecraft.renderFrame(boolean)}: the frame is encoded, the command
 * encoder submits, and the new {@code GpuSurface} swapchain abstraction
 * presents. Injecting immediately before the {@code GpuSurface.present()}
 * call keeps the old semantics exactly — the default framebuffer holds the
 * finished frame (MC's GL work is already submitted) and the Skia HUD
 * composites on top before the swap. Verified against the 26.2 client jar:
 * {@code present()} is invoked directly in {@code renderFrame}'s body.
 *
 * <p>{@code GpuSurface} only appears in the annotation string, so this class
 * also compiles against a 26.1 classpath; {@code EwoMixinPlugin} makes sure
 * it is only <i>applied</i> on 26.2+.
 *
 * <p>Note: this hook (and the HUD's second-GL-context painting model as a
 * whole) assumes the GL backend. 26.2's opt-in Vulkan renderer presents
 * through a different surface implementation — keep the Graphics API setting
 * on Default/OpenGL when the HUD is active.
 */
@Mixin(Minecraft.class)
public class EwoHudMixin262 {

    @Inject(
            method = "renderFrame",
            at = @At(
                    value = "INVOKE",
                    target = "Lcom/mojang/blaze3d/systems/GpuSurface;present()V"))
    private void ewo$beforePresent(boolean fullRender, CallbackInfo ci) {
        EwoFrameHook.run();
    }
}
