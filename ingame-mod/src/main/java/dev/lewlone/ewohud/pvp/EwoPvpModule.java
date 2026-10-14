package dev.lewlone.ewohud.pvp;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.attribute.BasicFileAttributes;
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
    /** The file the live config was last loaded from. */
    private static Path lastLoadedPath;
    /** mtime of {@code pvp.toml} when the live config was last loaded. */
    private static FileTime lastLoadedMtime;
    /** How often the hot-reload poll runs. Wall-clock, not per frame: at 240 fps
     *  a frame counter polled the disk 24 times a second for an editor that
     *  only changes the file when a human clicks. */
    private static final long POLL_INTERVAL_NS = 250_000_000L;
    private static long lastPollNs = System.nanoTime() - POLL_INTERVAL_NS;
    /** Active profile name as last read from {@code profiles.toml}, and that
     *  file's (mtime, size) at the time. {@code profiles.toml} is re-read only
     *  when its stamp changes — it used to be read in full on every poll. */
    private static String cachedProfile = "Default";
    private static FileTime profilesMtime;
    private static long profilesSize = -1L;

    private EwoPvpModule() {}

    /** Lazy init — runs on the first per-frame tick once Minecraft is alive,
     *  not on classload (so a missing pvp.toml is created in the right
     *  profile directory). */
    private static void ensureLoaded() {
        if (loaded) return;
        loaded = true;
        config = EwoPvpConfig.load();
        lastLoadedPath = pvpTomlPath();
        lastLoadedMtime = currentMtime(lastLoadedPath);
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

    /** Hot-reload poll — a few times a second, stat {@code pvp.toml} (and
     *  {@code profiles.toml}, to notice a profile switch) and reload if the
     *  file or the active profile changed. Only attribute reads happen on the
     *  hot path; nothing is read until an mtime actually differs. */
    private static void pollConfigReload() {
        long nowNs = System.nanoTime();
        if (nowNs - lastPollNs < POLL_INTERVAL_NS) return;
        lastPollNs = nowNs;
        Path path = pvpTomlPath();
        FileTime now = currentMtime(path);
        if (now == null) return;
        boolean switchedProfile = !path.equals(lastLoadedPath);
        if (switchedProfile || lastLoadedMtime == null || now.compareTo(lastLoadedMtime) != 0) {
            lastLoadedPath = path;
            lastLoadedMtime = now;
            EwoPvpConfig reloaded = EwoPvpConfig.load();
            EwoJumpReset.setConfig(reloaded);
            EwoHitRange.setConfig(reloaded);
            config = reloaded;
        }
    }

    /** mtime of {@code p}, or null if it is unset or cannot be statted (one
     *  syscall; a missing file is the common "no config yet" case). */
    private static FileTime currentMtime(Path p) {
        if (p == null) return null;
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
        BasicFileAttributes attrs;
        try {
            attrs = Files.readAttributes(p, BasicFileAttributes.class);
        } catch (IOException e) {
            profilesMtime = null;
            profilesSize = -1L;
            cachedProfile = "Default";
            return cachedProfile;
        }
        if (attrs.lastModifiedTime().equals(profilesMtime) && attrs.size() == profilesSize) {
            return cachedProfile; // unchanged since the last read
        }
        String active = "Default";
        try {
            for (String line : Files.readAllLines(p)) {
                String t = line.trim();
                if (t.startsWith("active") && t.contains("=")) {
                    String v = t.substring(t.indexOf('=') + 1).trim();
                    if (v.length() >= 2 && v.startsWith("\"") && v.endsWith("\"")) {
                        v = v.substring(1, v.length() - 1);
                    }
                    active = v;
                    break;
                }
            }
        } catch (IOException ignored) {
            // Unreadable right now (mid-rewrite): keep the previous answer and
            // retry on the next poll rather than caching a guess.
            return cachedProfile;
        }
        profilesMtime = attrs.lastModifiedTime();
        profilesSize = attrs.size();
        cachedProfile = active;
        return active;
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
