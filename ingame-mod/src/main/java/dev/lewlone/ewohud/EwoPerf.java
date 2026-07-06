package dev.lewlone.ewohud;

import java.io.BufferedWriter;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.StandardOpenOption;
import java.util.Locale;

/**
 * Opt-in render-thread profiler for the Java half of the HUD frame.
 *
 * <p>The Rust profiler ({@code crates/ewo-jni/src/perf.rs}) covers everything
 * inside {@code nativeRender}; this covers the Java work that runs every frame
 * <em>around</em> it — {@link EwoHudData#capture}, the JNI call itself, and
 * {@link EwoModules#tick}. Together they account for the full per-frame
 * render-thread cost the in-game client adds.
 *
 * <p>Gated by the same sentinel as the Rust side: enabled only if
 * {@code %TEMP%/ewo-perf.on} exists at class load, so normal play pays nothing.
 * Every {@link #WINDOW} frames a JSON-Lines summary is appended to
 * {@code %TEMP%/ewo-perf-java.jsonl} (truncated once at startup).
 */
public final class EwoPerf {
    private EwoPerf() {}

    private static final Path TMP = Paths.get(System.getProperty("java.io.tmpdir"));
    private static final boolean ON = Files.exists(TMP.resolve("ewo-perf.on"));
    private static final Path OUT = TMP.resolve("ewo-perf-java.jsonl");
    private static final int WINDOW = 300;

    private static long captureSum, renderSum, modulesSum;
    private static long captureMax, renderMax, modulesMax;
    private static int n;
    private static final long START = System.nanoTime();

    static {
        if (ON) {
            try {
                Files.write(OUT, new byte[0]); // fresh file per session
            } catch (IOException ignored) {
            }
        }
    }

    /** Whether instrumentation is active — callers gate their nanoTime calls. */
    public static boolean on() {
        return ON;
    }

    /** Record one frame's Java-side section durations (nanoseconds). */
    public static void record(long captureNs, long renderNs, long modulesNs) {
        captureSum += captureNs;
        renderSum += renderNs;
        modulesSum += modulesNs;
        if (captureNs > captureMax) captureMax = captureNs;
        if (renderNs > renderMax) renderMax = renderNs;
        if (modulesNs > modulesMax) modulesMax = modulesNs;
        if (++n >= WINDOW) {
            flush();
        }
    }

    private static void flush() {
        double cMean = captureSum / (double) n / 1000.0;
        double rMean = renderSum / (double) n / 1000.0;
        double mMean = modulesSum / (double) n / 1000.0;
        String line = String.format(
            Locale.US,
            "{\"t\":%.1f,\"n\":%d,\"capture_us\":{\"mean\":%.2f,\"max\":%.2f},"
                + "\"native_render_us\":{\"mean\":%.2f,\"max\":%.2f},"
                + "\"modules_us\":{\"mean\":%.2f,\"max\":%.2f}}",
            (System.nanoTime() - START) / 1e9, n,
            cMean, captureMax / 1000.0,
            rMean, renderMax / 1000.0,
            mMean, modulesMax / 1000.0);
        try (BufferedWriter w = Files.newBufferedWriter(
                OUT, StandardOpenOption.CREATE, StandardOpenOption.APPEND)) {
            w.write(line);
            w.newLine();
        } catch (IOException ignored) {
        }
        captureSum = renderSum = modulesSum = 0;
        captureMax = renderMax = modulesMax = 0;
        n = 0;
    }
}
