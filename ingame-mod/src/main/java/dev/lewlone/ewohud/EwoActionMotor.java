package dev.lewlone.ewohud;

import java.util.ArrayDeque;
import java.util.Deque;
import java.util.Random;

/**
 * Phase H — the shared action queue + tick driver for the auto-action modules.
 *
 * <p>Auto-action modules (Auto Tool, Auto Totem, Legit Elytra Swap) need to
 * fire a sequence of <i>real</i> client-side actions — number-key swap, open
 * inventory, click slot, close inventory — with realistic delays + jitter
 * between steps so the cadence reads as human, not bot.
 *
 * <p>The motor itself is small: a {@link Deque} of {@link Step}s, a wall-clock
 * "next-run-at" cursor, and a per-step jitter on the delay. Each step is just
 * a {@link Runnable} plus the post-step delay; step actions are responsible
 * for their own pre-flight checks and can call {@link #abort()} themselves
 * when the world state has shifted out from under them (the user closed the
 * inventory, the target item is gone, etc).
 *
 * <p>Only one sequence runs at a time — modules consult {@link #busy()} before
 * queueing. {@link #tick()} is called once per frame from
 * {@link EwoModules#tick()}.
 */
public final class EwoActionMotor {

    private EwoActionMotor() {}

    /** Per-step jitter as a fraction of the nominal delay. ±15% reads as a
     *  hurried-but-relaxed cadence — fast enough for combat, varied enough
     *  not to look like a tick-aligned macro. */
    private static final float JITTER_FRAC = 0.15f;

    private static final Deque<Step> QUEUE = new ArrayDeque<>();
    /** Wall-clock millis when the head step is eligible to run. */
    private static long nextRunAt = 0L;
    private static final Random RAND = new Random();

    private record Step(Runnable action, int delayMsAfter) {}

    /** True while a sequence is queued or mid-flight. Modules consult this
     *  before queueing a new one — sequences must not overlap. */
    public static boolean busy() {
        return !QUEUE.isEmpty();
    }

    /** Drop every pending step. Safe to call from inside a step's action. */
    public static void abort() {
        QUEUE.clear();
        nextRunAt = 0L;
    }

    /**
     * Append a step. The first step in a fresh sequence runs on the next
     * {@link #tick()}; subsequent steps wait their predecessor's
     * {@code delayMsAfter} (with jitter).
     */
    public static void enqueue(Runnable action, int delayMsAfter) {
        if (QUEUE.isEmpty()) {
            nextRunAt = System.currentTimeMillis();
        }
        QUEUE.addLast(new Step(action, Math.max(0, delayMsAfter)));
    }

    /** Drain ready steps. May fire multiple steps in one tick if their
     *  cumulative jittered delays land inside the frame interval. */
    public static void tick() {
        long now = System.currentTimeMillis();
        while (!QUEUE.isEmpty() && now >= nextRunAt) {
            Step step = QUEUE.pollFirst();
            try {
                step.action.run();
            } catch (Throwable t) {
                System.err.println("[ewo-hud] action motor step failed: " + t);
                t.printStackTrace();
                QUEUE.clear();
                nextRunAt = 0L;
                return;
            }
            nextRunAt = now + jitter(step.delayMsAfter);
        }
    }

    private static int jitter(int delayMs) {
        if (delayMs <= 0) {
            return 0;
        }
        float span = delayMs * JITTER_FRAC;
        float offset = (RAND.nextFloat() * 2f - 1f) * span;
        return Math.max(0, Math.round(delayMs + offset));
    }
}
