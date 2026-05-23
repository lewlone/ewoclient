package dev.lewlone.ewohud.pvp;

import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;

/**
 * Defensive jump-reset tracker. Ported verbatim from the source mod's
 * {@code JumpResetModule}; the only differences are (a) the result lives in a
 * static singleton readable from Rust through {@code EwoHudData}, not a Java
 * HUD renderer, and (b) the source mod's {@code MinecraftClient} is the
 * Mojmap {@code Minecraft} / {@code LocalPlayer} pair.
 *
 * <p>Measures the tick offset between taking damage and jumping. Perfect =
 * jump exactly one tick after the hit (0&nbsp;ms offset). Tiers degrade with
 * a 100&nbsp;ms threshold &mdash; matches the source mod's classification, so
 * any muscle memory you have transfers.
 *
 * <p>Hot path is allocation-free; called from {@code EwoHudData.capture}
 * which runs once per render frame.
 */
public final class EwoJumpReset {

    public enum Tier {
        NONE, PERFECT, SLIGHTLY_EARLY, EARLY, SLIGHTLY_LATE, LATE
    }

    /** Tick ages last recorded. {@code Integer.MIN_VALUE} for "never seen". */
    private static int hurtAge = Integer.MIN_VALUE;
    private static int jumpAge = Integer.MIN_VALUE;

    /** Health on the previous tick — for the damage-drop edge. */
    private static float previousHealth = -1f;

    /** Current result + display state. */
    private static Tier currentTier = Tier.NONE;
    private static int currentOffsetMs = 0;
    private static boolean hasResult = false;
    /** Wall-clock seconds left on the fade — driven by real time, NOT frame
     *  count, so the widget stays visible the same duration at 60 fps or 500. */
    private static float displaySecondsRemaining = 0f;
    /** Wall-clock duration of the fade, in seconds (1.0 = the source mod's
     *  20-tick default). The config still names it "fadeTicks" — convert there. */
    private static float displaySecondsTotal = 1f;
    /** Last wall-clock tick time (ns), for delta-time fade. */
    private static long lastTickNanos = 0L;
    /** Latches a fresh result for the once-per-result sound trigger.
     *  Set by {@link #onJump} / {@link #checkEarlyJump}; cleared ONLY by
     *  {@link #consumeNewResultLatch}. Don't clear in {@link #tick} — the
     *  consumer in {@code EwoPvpModule.tick} runs AFTER tick(), and clearing
     *  here would erase the latch the mixin just set. */
    private static boolean newResultThisTick = false;

    private static EwoPvpConfig config = new EwoPvpConfig();

    private static final int SLIGHTLY_THRESHOLD_MS = 100;

    private EwoJumpReset() {}

    public static void setConfig(EwoPvpConfig cfg) {
        if (cfg != null) {
            config = cfg;
            // "fadeTicks" in config is the source mod's 20 ticks/sec model;
            // convert to wall-clock seconds (20 ticks = 1.0 s).
            displaySecondsTotal = cfg.jumpResetFadeTicks / 20f;
            if (displaySecondsTotal <= 0f) displaySecondsTotal = 1f;
        }
    }

    /** Per-frame tick. Detects health drops (the local player just took damage)
     *  and decays the fade timer in real time. Called from
     *  {@code EwoHudData.capture}.
     *
     *  <p>Does NOT clear {@link #newResultThisTick} — that's the latch the
     *  mixin sets on a jump / damage edge, and the sound consumer reads it
     *  AFTER tick() returns. Clearing here would erase a result the user
     *  just produced, which is the exact bug that suppressed sounds. */
    public static void tick() {
        if (config == null || !config.jumpResetEnabled) {
            previousHealth = -1f;
            displaySecondsRemaining = 0f;
            return;
        }

        Minecraft mc = Minecraft.getInstance();
        LocalPlayer player = mc != null ? mc.player : null;
        if (player == null) {
            previousHealth = -1f;
            displaySecondsRemaining = 0f;
            return;
        }

        // Damage detection by health drop. Runs once per render frame, which is
        // a few ticks slower than the source mod's END_CLIENT_TICK; the same
        // 1-tick compensation in `checkEarlyJump` carries over and behaves the
        // same in practice because Minecraft's damage event is itself a
        // ~1-tick-delayed signal regardless of when we sample it.
        float currentHealth = player.getHealth();
        if (previousHealth > 0 && currentHealth < previousHealth) {
            int playerAge = player.tickCount;
            hurtAge = playerAge;
            checkEarlyJump(playerAge);
            if (config.jumpResetShowAllHits && !hasResult) {
                currentTier = Tier.NONE;
                currentOffsetMs = 0;
                hasResult = true;
                displaySecondsRemaining = displaySecondsTotal;
                newResultThisTick = true;
            }
        }
        previousHealth = currentHealth;

        // Wall-clock fade decrement. lastTickNanos = 0 on first tick — bail
        // and seed it, so the first frame doesn't burn the whole fade.
        long now = System.nanoTime();
        if (lastTickNanos != 0L && displaySecondsRemaining > 0f) {
            float dt = (now - lastTickNanos) / 1_000_000_000f;
            displaySecondsRemaining -= dt;
            if (displaySecondsRemaining < 0f) displaySecondsRemaining = 0f;
        }
        lastTickNanos = now;
    }

    /** Mixin entry: the local player just jumped (LivingEntity.jumpFromGround
     *  HEAD). Tier-classifies the offset against any recent hurt. */
    public static void onJump(int playerTickAge) {
        if (config == null || !config.jumpResetEnabled) return;
        jumpAge = playerTickAge;

        int tickDiff = jumpAge - hurtAge;
        if (tickDiff < 0 || tickDiff > config.jumpResetProximityWindowTicks) {
            return;
        }
        // Offset from the ideal: ideal is hurtAge + 1, so tickDiff=1 → 0ms.
        int offsetTicks = tickDiff - 1;
        currentOffsetMs = offsetTicks * 50;
        currentTier = classify(currentOffsetMs);
        hasResult = true;
        displaySecondsRemaining = displaySecondsTotal;
        newResultThisTick = true;
    }

    /** Damage arrived after a jump — classify as early. Same 1-tick
     *  compensation as the source mod. */
    private static void checkEarlyJump(int playerAge) {
        int ticksSinceJump = playerAge - jumpAge;
        if (ticksSinceJump < 0 || ticksSinceJump > config.jumpResetProximityWindowTicks) {
            return;
        }
        int adjustedTicks = ticksSinceJump - 1;
        if (adjustedTicks <= 0) {
            currentOffsetMs = 0;
            currentTier = Tier.PERFECT;
        } else {
            currentOffsetMs = -(adjustedTicks * 50);
            currentTier = classify(currentOffsetMs);
        }
        hasResult = true;
        displaySecondsRemaining = displaySecondsTotal;
        newResultThisTick = true;
    }

    private static Tier classify(int offsetMs) {
        if (offsetMs == 0) return Tier.PERFECT;
        if (offsetMs > 0 && offsetMs <= SLIGHTLY_THRESHOLD_MS) return Tier.SLIGHTLY_LATE;
        if (offsetMs > SLIGHTLY_THRESHOLD_MS) return Tier.LATE;
        if (offsetMs >= -SLIGHTLY_THRESHOLD_MS) return Tier.SLIGHTLY_EARLY;
        return Tier.EARLY;
    }

    // ── Read-only state exposed to EwoHudData + sound trigger ─────────────

    public static boolean hasResult() {
        return hasResult && displaySecondsRemaining > 0f;
    }

    public static Tier currentTier() { return currentTier; }
    public static int currentOffsetMs() { return currentOffsetMs; }

    /** "Age" of the current result, encoded in the same units the wire schema
     *  expects (1 unit = 50 ms = one source-mod tick). Reading the fade as
     *  age + total lets the Rust side compute the 0..1 fade fraction. */
    public static int ageTicks() {
        float elapsed = displaySecondsTotal - displaySecondsRemaining;
        return Math.max(0, (int) (elapsed * 20f));
    }

    public static int fadeTotalTicks() {
        return Math.max(1, (int) (displaySecondsTotal * 20f));
    }

    /** Consume the "new result this frame" latch — used by the sound trigger
     *  in {@code EwoPvpModule.tick} so a single result fires its sound exactly
     *  once. */
    public static boolean consumeNewResultLatch() {
        boolean v = newResultThisTick;
        newResultThisTick = false;
        return v;
    }

    /** Reset all state. Called when the world changes / on /respawn so old
     *  ages don't leak into the next session. */
    public static void resetSession() {
        hurtAge = Integer.MIN_VALUE;
        jumpAge = Integer.MIN_VALUE;
        previousHealth = -1f;
        currentTier = Tier.NONE;
        currentOffsetMs = 0;
        hasResult = false;
        displaySecondsRemaining = 0f;
        lastTickNanos = 0L;
        newResultThisTick = false;
    }
}
