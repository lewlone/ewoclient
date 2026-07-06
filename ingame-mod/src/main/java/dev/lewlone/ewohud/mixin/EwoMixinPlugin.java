package dev.lewlone.ewohud.mixin;

import java.util.List;
import java.util.Set;

import org.objectweb.asm.tree.ClassNode;
import org.spongepowered.asm.mixin.extensibility.IMixinConfigPlugin;
import org.spongepowered.asm.mixin.extensibility.IMixinInfo;

/**
 * Runtime mixin selection: one ewo-hud jar serves both the 26.1 and 26.2
 * manifest lines, so version-specific mixins ship side by side and this
 * plugin applies exactly one variant per runtime.
 *
 * <p>Convention: a mixin class named with the {@code 262} suffix applies on
 * MC 26.2+ only; its counterpart in {@link #LEGACY_ONLY} applies below 26.2.
 * Everything else applies everywhere.
 *
 * <p>Version detection order:
 * <ol>
 *   <li>{@code -Dewo.mc.version} — the EwoClient launcher passes the
 *       instance's exact version id on every launch. Authoritative.</li>
 *   <li>The Fabric loader's builtin {@code minecraft} mod container —
 *       covers a jar launched outside the EwoClient launcher.</li>
 *   <li>Neither resolvable → assume the 26.1 line (the older, safer set:
 *       a wrong guess there fails soft because 26.1 injectors simply find
 *       no target on newer versions only when require would trip — whereas
 *       applying 26.2 mixins on 26.1 is guaranteed wrong).</li>
 * </ol>
 */
public class EwoMixinPlugin implements IMixinConfigPlugin {

    /** Mixins that must NOT apply on 26.2+ (their {@code 262} twin does). */
    private static final Set<String> LEGACY_ONLY =
            Set.of("EwoHudMixin", "GuiCrosshairMixin", "FireOverlayMixin");

    private boolean is262Plus;

    @Override
    public void onLoad(String mixinPackage) {
        String version = detectVersion();
        this.is262Plus = version != null && lineAtLeast(version, 26, 2);
        System.out.println(
                "[ewo-hud] mixin plugin: mc version = "
                        + (version == null ? "unknown (assuming 26.1 line)" : version)
                        + " → " + (is262Plus ? "26.2+" : "26.1") + " mixin set");
    }

    private static String detectVersion() {
        String prop = System.getProperty("ewo.mc.version");
        if (prop != null && !prop.isBlank()) {
            return prop;
        }
        try {
            return net.fabricmc.loader.api.FabricLoader.getInstance()
                    .getModContainer("minecraft")
                    .map(c -> c.getMetadata().getVersion().getFriendlyString())
                    .orElse(null);
        } catch (Throwable t) {
            return null;
        }
    }

    /**
     * Whether a version id's line is at least {@code major.minor}. Tolerant
     * of suffixes ("26.2.1", "26.2-pre-3", "26.2-rc-1" all count as 26.2).
     */
    static boolean lineAtLeast(String version, int major, int minor) {
        String[] parts = version.split("[^0-9]+");
        int maj = parts.length > 0 && !parts[0].isEmpty() ? Integer.parseInt(parts[0]) : 0;
        int min = parts.length > 1 && !parts[1].isEmpty() ? Integer.parseInt(parts[1]) : 0;
        return maj > major || (maj == major && min >= minor);
    }

    @Override
    public boolean shouldApplyMixin(String targetClassName, String mixinClassName) {
        String simple = mixinClassName.substring(mixinClassName.lastIndexOf('.') + 1);
        if (simple.endsWith("262")) {
            return is262Plus;
        }
        if (LEGACY_ONLY.contains(simple)) {
            return !is262Plus;
        }
        return true;
    }

    @Override
    public String getRefMapperConfig() {
        return null;
    }

    @Override
    public void acceptTargets(Set<String> myTargets, Set<String> otherTargets) {}

    @Override
    public List<String> getMixins() {
        return null;
    }

    @Override
    public void preApply(String targetClassName, ClassNode targetClass, String mixinClassName, IMixinInfo mixinInfo) {}

    @Override
    public void postApply(String targetClassName, ClassNode targetClass, String mixinClassName, IMixinInfo mixinInfo) {}
}
