package dev.lewlone.ewohud;

import java.util.ArrayDeque;
import java.util.Deque;

import net.minecraft.client.Minecraft;
import net.minecraft.client.Options;

/**
 * Rolling-window click-per-second tracker for the left + right mouse buttons.
 *
 * <p>Polled once per frame from {@link EwoHudData#capture}, before the buffer
 * read. Counts rising edges only — holding the button down is one click, not
 * many — which matches how AxolotlClient + Lunar CPS counters report.
 *
 * <p>{@code keyAttack.isDown()} / {@code keyUse.isDown()} return {@code false}
 * while any GUI screen is open, so menu clicks correctly don't register.
 */
public final class EwoClickTracker {

    private EwoClickTracker() {}

    /** Rolling-window width in milliseconds — clicks within this window count. */
    private static final long WINDOW_MS = 1000L;

    private static final Deque<Long> LEFT = new ArrayDeque<>();
    private static final Deque<Long> RIGHT = new ArrayDeque<>();
    private static boolean leftWasDown;
    private static boolean rightWasDown;

    /** Sample current button state; record rising edges; prune old entries. */
    public static void tick() {
        Minecraft mc = Minecraft.getInstance();
        Options o = mc != null ? mc.options : null;
        boolean leftDown = o != null && o.keyAttack.isDown();
        boolean rightDown = o != null && o.keyUse.isDown();
        long now = System.currentTimeMillis();
        if (leftDown && !leftWasDown) {
            LEFT.addLast(now);
        }
        if (rightDown && !rightWasDown) {
            RIGHT.addLast(now);
        }
        leftWasDown = leftDown;
        rightWasDown = rightDown;
        prune(LEFT, now);
        prune(RIGHT, now);
    }

    public static int leftCps() {
        return LEFT.size();
    }

    public static int rightCps() {
        return RIGHT.size();
    }

    private static void prune(Deque<Long> q, long now) {
        while (!q.isEmpty() && now - q.peekFirst() > WINDOW_MS) {
            q.removeFirst();
        }
    }
}
