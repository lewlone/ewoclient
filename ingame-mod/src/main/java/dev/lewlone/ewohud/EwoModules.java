package dev.lewlone.ewohud;

/**
 * Applies EwoClient module effects from the {@link EwoModuleData} channel
 * (Phase G).
 *
 * <p>{@link #tick()} runs once per frame from {@code EwoHudMixin}, after Rust
 * has refreshed the module-state block. G1 only confirms the channel is live;
 * the per-frame effects (Toggle Sprint / Sneak) and the render-override mixins
 * (Full Bright, FOV, …) land in later Phase G steps.
 */
public final class EwoModules {
    private EwoModules() {}

    /** Guards the "channel live" line so it logs exactly once. */
    private static boolean announced;

    /** Per-frame module tick. Called from {@code flipFrame}, post-render. */
    public static void tick() {
        if (!announced && EwoModuleData.ready()) {
            announced = true;
            System.err.println("[ewo-hud] module channel live — schema "
                    + EwoModuleData.SCHEMA_VERSION + ", "
                    + EwoModuleData.moduleCount() + " modules");
        }
    }
}
