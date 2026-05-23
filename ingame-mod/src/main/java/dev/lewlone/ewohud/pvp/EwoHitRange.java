package dev.lewlone.ewohud.pvp;

import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;

/**
 * Hit-range tracker — measures the eye-to-hitbox distance on every attack,
 * matches it against the configured zones, and records the result for the
 * Rust HUD widget. Ported from the source mod's {@code HitRangeModule}.
 *
 * <p>Eye-to-nearest-point-on-hitbox is the same formula Minecraft uses for
 * reach, so a "3.00 blocks" reading here means "a max-range vanilla hit" —
 * the source mod's whole appeal.
 */
public final class EwoHitRange {

    private static float lastDistance = -1f;
    private static int matchedZoneColor = 0;
    private static boolean hasResult = false;
    /** Wall-clock seconds left on the fade — driven by real time, NOT frame
     *  count, so widget duration is independent of FPS. */
    private static float displaySecondsRemaining = 0f;
    private static float displaySecondsTotal = 1f;
    private static long lastTickNanos = 0L;
    /** Latches a fresh result for the once-per-result sound trigger.
     *  Cleared only by {@link #consumeNewResultLatch} — NOT in {@link #tick},
     *  because the sound consumer runs AFTER tick() and would read a cleared
     *  latch. */
    private static boolean newResultThisTick = false;

    private static EwoPvpConfig config = new EwoPvpConfig();

    private EwoHitRange() {}

    public static void setConfig(EwoPvpConfig cfg) {
        if (cfg != null) {
            config = cfg;
            // "fadeTicks" → wall-clock seconds at 20 ticks/sec.
            displaySecondsTotal = cfg.hitRangeFadeTicks / 20f;
            if (displaySecondsTotal <= 0f) displaySecondsTotal = 1f;
        }
    }

    /** Per-frame tick — drain the wall-clock fade timer. */
    public static void tick() {
        if (config == null || !config.hitRangeEnabled) {
            displaySecondsRemaining = 0f;
            return;
        }
        long now = System.nanoTime();
        if (lastTickNanos != 0L && displaySecondsRemaining > 0f) {
            float dt = (now - lastTickNanos) / 1_000_000_000f;
            displaySecondsRemaining -= dt;
            if (displaySecondsRemaining < 0f) displaySecondsRemaining = 0f;
        }
        lastTickNanos = now;
    }

    /** Mixin entry: the local player just hit something (Player.attack HEAD). */
    public static void onAttack(Entity target) {
        if (config == null || !config.hitRangeEnabled || target == null) return;
        Minecraft mc = Minecraft.getInstance();
        LocalPlayer player = mc != null ? mc.player : null;
        if (player == null) return;

        // Eye → nearest point on the target's hitbox — matches vanilla reach.
        Vec3 eye = player.getEyePosition();
        AABB box = target.getBoundingBox();
        double cx = Math.max(box.minX, Math.min(eye.x, box.maxX));
        double cy = Math.max(box.minY, Math.min(eye.y, box.maxY));
        double cz = Math.max(box.minZ, Math.min(eye.z, box.maxZ));
        float distance = (float) eye.distanceTo(new Vec3(cx, cy, cz));

        EwoPvpConfig.Zone zone = config.findMatchingZone(distance);
        if (zone == null) {
            // Hit didn't match any enabled zone — keep the prior widget
            // result alone (so a non-max hit doesn't blank a recent max-reach
            // celebration) and just don't trigger a new display.
            return;
        }

        lastDistance = distance;
        matchedZoneColor = zone.color;
        hasResult = true;
        displaySecondsRemaining = displaySecondsTotal;
        newResultThisTick = true;
    }

    // ── Read-only state exposed to EwoHudData ─────────────────────────────

    public static boolean hasResult() {
        return hasResult && displaySecondsRemaining > 0f;
    }

    public static float lastDistance() { return lastDistance; }
    public static int matchedZoneColor() { return matchedZoneColor; }
    public static int ageTicks() {
        float elapsed = displaySecondsTotal - displaySecondsRemaining;
        return Math.max(0, (int) (elapsed * 20f));
    }
    public static int fadeTotalTicks() {
        return Math.max(1, (int) (displaySecondsTotal * 20f));
    }

    /** The zone matched by the latest hit (or null). For the sound trigger. */
    public static EwoPvpConfig.Zone matchedZone() {
        if (config == null) return null;
        return config.findMatchingZone(lastDistance);
    }

    public static boolean consumeNewResultLatch() {
        boolean v = newResultThisTick;
        newResultThisTick = false;
        return v;
    }

    public static void resetSession() {
        lastDistance = -1f;
        matchedZoneColor = 0;
        hasResult = false;
        displaySecondsRemaining = 0f;
        lastTickNanos = 0L;
        newResultThisTick = false;
    }
}
