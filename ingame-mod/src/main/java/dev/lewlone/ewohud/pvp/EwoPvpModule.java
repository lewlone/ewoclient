package dev.lewlone.ewohud.pvp;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.attribute.FileTime;

/**
 * PvP Utils — the top-level driver that owns the active config and routes
 * tick/sound events between the trackers.
 *
 * <p>Per-frame flow (called from {@code EwoHudData.capture}):
 * <ol>
 *   <li>{@link #pollConfigReload} checks the active profile's {@code pvp.toml}
 *       mtime; if it changed since the last load (the in-game overlay editor
 *       just wrote it, or the launcher's PvP tab did), the config reloads
 *       live so the edit applies without a relaunch;</li>
 *   <li>Both trackers tick their fade timers + the jump-reset tracker
 *       samples the local player's health for the damage edge;</li>
 *   <li>If either tracker latched a fresh result this frame, the
 *       configured tier/zone sound fires;</li>
 *   <li>{@code EwoHudData} writes the read-only state into the shared block.</li>
 * </ol>
 */
public final class EwoPvpModule {

    private static EwoPvpConfig config = new EwoPvpConfig();
    private static boolean loaded;
    /** mtime of {@code pvp.toml} when the live config was last loaded. */
    private static FileTime lastLoadedMtime;
    /** Ticks since the last mtime poll — we only stat() every few frames. */
    private static int pollCounter;

    private EwoPvpModule() {}

    /** Lazy init — runs on the first per-frame tick once Minecraft is alive,
     *  not on classload (so a missing pvp.toml is created in the right
     *  profile directory). */
    private static void ensureLoaded() {
        if (loaded) return;
        loaded = true;
        config = EwoPvpConfig.load();
        lastLoadedMtime = currentMtime();
        EwoJumpReset.setConfig(config);
        EwoHitRange.setConfig(config);
    }

    /** Swap the live config — called from the editor UIs after a write to
     *  pvp.toml so the change applies without a relaunch. */
    public static void replaceConfig(EwoPvpConfig cfg) {
        if (cfg == null) return;
        config = cfg;
        EwoJumpReset.setConfig(cfg);
        EwoHitRange.setConfig(cfg);
    }

    public static EwoPvpConfig config() {
        ensureLoaded();
        return config;
    }

    /** Hot-reload poll — every ~10 frames, stat {@code pvp.toml} and reload
     *  if its mtime changed. Once-per-second-ish granularity is plenty for an
     *  editor that drives this from human clicks. */
    private static void pollConfigReload() {
        pollCounter++;
        if (pollCounter < 10) return;
        pollCounter = 0;
        FileTime now = currentMtime();
        if (now == null) return;
        if (lastLoadedMtime == null || now.compareTo(lastLoadedMtime) != 0) {
            lastLoadedMtime = now;
            EwoPvpConfig reloaded = EwoPvpConfig.load();
            EwoJumpReset.setConfig(reloaded);
            EwoHitRange.setConfig(reloaded);
            config = reloaded;
        }
    }

    private static FileTime currentMtime() {
        Path p = pvpTomlPath();
        if (p == null || !Files.exists(p)) return null;
        try {
            return Files.getLastModifiedTime(p);
        } catch (IOException e) {
            return null;
        }
    }

    private static Path pvpTomlPath() {
        String appdata = System.getenv("APPDATA");
        if (appdata == null || appdata.isEmpty()) return null;
        return Paths.get(appdata, "EwoClient", "profiles", activeProfile(), "pvp.toml");
    }

    private static String activeProfile() {
        // Same default as EwoPvpConfig; profiles.toml is read straight here
        // to avoid a circular reference to that class's private helper.
        String appdata = System.getenv("APPDATA");
        if (appdata == null) return "Default";
        Path p = Paths.get(appdata, "EwoClient", "profiles.toml");
        if (!Files.isRegularFile(p)) return "Default";
        try {
            for (String line : Files.readAllLines(p)) {
                String t = line.trim();
                if (t.startsWith("active") && t.contains("=")) {
                    String v = t.substring(t.indexOf('=') + 1).trim();
                    if (v.length() >= 2 && v.startsWith("\"") && v.endsWith("\"")) {
                        return v.substring(1, v.length() - 1);
                    }
                    return v;
                }
            }
        } catch (IOException ignored) {
        }
        return "Default";
    }

    /** Run the per-frame trackers + fire sounds. Called from
     *  {@code EwoHudData.capture}, before the data block is written. */
    public static void tick() {
        ensureLoaded();
        pollConfigReload();
        EwoJumpReset.tick();
        EwoHitRange.tick();

        // Sound triggers — once per fresh result.
        if (EwoJumpReset.consumeNewResultLatch()) {
            EwoJumpReset.Tier tier = EwoJumpReset.currentTier();
            EwoPvpConfig.SoundSlot slot = config.soundForTier(tier);
            if (slot != null) slot.play();
        }
        if (EwoHitRange.consumeNewResultLatch()) {
            EwoPvpConfig.Zone zone = EwoHitRange.matchedZone();
            if (zone != null && zone.enabled) {
                zone.sound.play(zone.volume, zone.pitch);
            }
        }
    }

    /** Clear all tracker state — call on disconnect / world change. */
    public static void resetSession() {
        EwoJumpReset.resetSession();
        EwoHitRange.resetSession();
    }
}
