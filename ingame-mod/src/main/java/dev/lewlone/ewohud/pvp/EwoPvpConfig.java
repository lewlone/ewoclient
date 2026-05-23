package dev.lewlone.ewohud.pvp;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.List;

/**
 * Per-profile PvP-Utils configuration — lives at
 * {@code %APPDATA%/EwoClient/profiles/<active>/pvp.toml}, paralleling
 * {@code hud.toml} and {@code modules.toml}.
 *
 * <p>Pure data. The Java trackers and the Rust HUD widgets both consult this;
 * Sprint 2's launcher / overlay tabs will edit it. Auto-creates the file on
 * first run with sensible Velvet defaults, so the feature works
 * out-of-the-box without anyone touching a TOML editor.
 *
 * <p>Hand-rolled parser (no toml4j dependency) — same shape as
 * {@code hud.rs::HudLayout::load}.
 */
public final class EwoPvpConfig {

    public static final int SCHEMA_VERSION = 1;

    /** Module-level enable + display tuning. */
    public boolean jumpResetEnabled = true;
    public boolean jumpResetBarEnabled = true;
    public int jumpResetProximityWindowTicks = 6;   // 300 ms
    public int jumpResetFadeTicks = 20;             // 1 s
    public int jumpResetBarMaxRangeMs = 300;
    public boolean jumpResetShowAllHits = false;

    public boolean hitRangeEnabled = true;
    public int hitRangeFadeTicks = 20;              // 1 s

    /** Velvet-themed tier colours (RRGGBB). Defaults match {@code hud.rs}'s
     *  palette — rose for perfect, champ for slightly off, ember for late. */
    public int colorPerfect       = 0xE8D4A8; // champ — the celebratory tier
    public int colorSlightlyLate  = 0xE5B8C5; // rose
    public int colorLate          = 0xC96A7A; // ember
    public int colorSlightlyEarly = 0xC9A5D4; // lavender
    public int colorEarly         = 0xB47491; // berry
    public int colorNoReset       = 0x9A8087; // mauve

    /** One per tier — five-tier sound config the source mod modelled. */
    public final SoundSlot soundPerfect       = new SoundSlot(true, EwoPvpSounds.BELL,  2.0f, 1.0f);
    public final SoundSlot soundSlightlyLate  = new SoundSlot(true, EwoPvpSounds.PLING, 1.5f, 0.8f);
    public final SoundSlot soundLate          = new SoundSlot(true, EwoPvpSounds.PLING, 0.8f, 0.8f);
    public final SoundSlot soundSlightlyEarly = new SoundSlot(true, EwoPvpSounds.BASS,  1.5f, 0.8f);
    public final SoundSlot soundEarly         = new SoundSlot(true, EwoPvpSounds.BASS,  0.8f, 0.8f);

    /** Hit-range zones — up to three non-overlapping ranges. */
    public final Zone zone1 = new Zone(false, 0.0f, 2.0f, EwoPvpSounds.HARP,  1.2f, 0.6f, 0xC9A5D4); // lav
    public final Zone zone2 = new Zone(false, 2.5f, 2.8f, EwoPvpSounds.PLING, 1.5f, 0.8f, 0xE8D4A8); // champ
    public final Zone zone3 = new Zone(true,  2.9f, 3.0f, EwoPvpSounds.BELL,  2.0f, 1.0f, 0xE5B8C5); // rose

    public static final class SoundSlot {
        public boolean enabled;
        public EwoPvpSounds sound;
        public float pitch;
        public float volume;

        public SoundSlot(boolean enabled, EwoPvpSounds sound, float pitch, float volume) {
            this.enabled = enabled;
            this.sound = sound;
            this.pitch = pitch;
            this.volume = volume;
        }

        public void play() {
            if (enabled) sound.play(volume, pitch);
        }
    }

    public static final class Zone {
        public boolean enabled;
        public float minDist;
        public float maxDist;
        public EwoPvpSounds sound;
        public float pitch;
        public float volume;
        public int color;

        public Zone(boolean enabled, float minDist, float maxDist,
                    EwoPvpSounds sound, float pitch, float volume, int color) {
            this.enabled = enabled;
            this.minDist = minDist;
            this.maxDist = maxDist;
            this.sound = sound;
            this.pitch = pitch;
            this.volume = volume;
            this.color = color;
        }

        public boolean contains(float d) {
            return d >= minDist && d <= maxDist;
        }
    }

    /** Find which enabled zone — if any — this distance falls into.
     *  Zone 3 (max-reach) wins ties, then zone 2, then zone 1. */
    public Zone findMatchingZone(float distance) {
        if (zone3.enabled && zone3.contains(distance)) return zone3;
        if (zone2.enabled && zone2.contains(distance)) return zone2;
        if (zone1.enabled && zone1.contains(distance)) return zone1;
        return null;
    }

    public SoundSlot soundForTier(EwoJumpReset.Tier tier) {
        return switch (tier) {
            case PERFECT -> soundPerfect;
            case SLIGHTLY_LATE -> soundSlightlyLate;
            case LATE -> soundLate;
            case SLIGHTLY_EARLY -> soundSlightlyEarly;
            case EARLY -> soundEarly;
            case NONE -> null;
        };
    }

    public int colorForTier(EwoJumpReset.Tier tier) {
        return switch (tier) {
            case PERFECT -> colorPerfect;
            case SLIGHTLY_LATE -> colorSlightlyLate;
            case LATE -> colorLate;
            case SLIGHTLY_EARLY -> colorSlightlyEarly;
            case EARLY -> colorEarly;
            case NONE -> colorNoReset;
        };
    }

    // ──────────────────────────────────────────────────────────────────────
    // Persistence — mirror of how Rust's hud.rs loads / saves hud.toml.
    // The Java mod side is the writer of last resort when no UI exists yet
    // (Sprint 1); Sprint 2's launcher tab will be the primary editor.
    // ──────────────────────────────────────────────────────────────────────

    /** Load the active profile's pvp.toml, falling back to defaults —
     *  and writing the defaults to disk if the file was missing. */
    public static EwoPvpConfig load() {
        EwoPvpConfig cfg = new EwoPvpConfig();
        Path path = configPath();
        if (path == null) return cfg;
        if (!Files.isRegularFile(path)) {
            // First run on this profile — persist the defaults so the
            // user has something to hand-edit.
            cfg.save();
            return cfg;
        }
        try {
            cfg.parse(Files.readAllLines(path));
        } catch (IOException ignored) {
            // Malformed file — defaults stay, log nothing (mod stays quiet).
        }
        return cfg;
    }

    /** Write this config to the active profile's pvp.toml. */
    public void save() {
        Path path = configPath();
        if (path == null) return;
        try {
            Files.createDirectories(path.getParent());
            Files.writeString(path, format());
        } catch (IOException ignored) {
        }
    }

    private void parse(List<String> lines) {
        String section = "";
        for (String raw : lines) {
            String line = raw.trim();
            if (line.isEmpty() || line.startsWith("#")) continue;
            if (line.startsWith("[") && line.endsWith("]")) {
                section = line.substring(1, line.length() - 1).trim();
                continue;
            }
            int eq = line.indexOf('=');
            if (eq < 0) continue;
            String key = line.substring(0, eq).trim();
            String val = unquote(line.substring(eq + 1).trim());

            switch (section) {
                case "jump_reset" -> applyJumpResetKey(key, val);
                case "hit_range" -> applyHitRangeKey(key, val);
                case "colors" -> applyColorsKey(key, val);
                case "sounds.perfect" -> applySoundKey(soundPerfect, key, val);
                case "sounds.slightly_late" -> applySoundKey(soundSlightlyLate, key, val);
                case "sounds.late" -> applySoundKey(soundLate, key, val);
                case "sounds.slightly_early" -> applySoundKey(soundSlightlyEarly, key, val);
                case "sounds.early" -> applySoundKey(soundEarly, key, val);
                case "zones.zone1" -> applyZoneKey(zone1, key, val);
                case "zones.zone2" -> applyZoneKey(zone2, key, val);
                case "zones.zone3" -> applyZoneKey(zone3, key, val);
                default -> {}
            }
        }
    }

    private void applyJumpResetKey(String key, String val) {
        switch (key) {
            case "enabled" -> jumpResetEnabled = parseBool(val, jumpResetEnabled);
            case "bar_enabled" -> jumpResetBarEnabled = parseBool(val, jumpResetBarEnabled);
            case "proximity_window_ticks" -> jumpResetProximityWindowTicks = parseInt(val, jumpResetProximityWindowTicks);
            case "fade_ticks" -> jumpResetFadeTicks = parseInt(val, jumpResetFadeTicks);
            case "bar_max_range_ms" -> jumpResetBarMaxRangeMs = parseInt(val, jumpResetBarMaxRangeMs);
            case "show_all_hits" -> jumpResetShowAllHits = parseBool(val, jumpResetShowAllHits);
            default -> {}
        }
    }

    private void applyHitRangeKey(String key, String val) {
        switch (key) {
            case "enabled" -> hitRangeEnabled = parseBool(val, hitRangeEnabled);
            case "fade_ticks" -> hitRangeFadeTicks = parseInt(val, hitRangeFadeTicks);
            default -> {}
        }
    }

    private void applyColorsKey(String key, String val) {
        switch (key) {
            case "perfect" -> colorPerfect = parseColor(val, colorPerfect);
            case "slightly_late" -> colorSlightlyLate = parseColor(val, colorSlightlyLate);
            case "late" -> colorLate = parseColor(val, colorLate);
            case "slightly_early" -> colorSlightlyEarly = parseColor(val, colorSlightlyEarly);
            case "early" -> colorEarly = parseColor(val, colorEarly);
            case "no_reset" -> colorNoReset = parseColor(val, colorNoReset);
            default -> {}
        }
    }

    private static void applySoundKey(SoundSlot s, String key, String val) {
        switch (key) {
            case "enabled" -> s.enabled = parseBool(val, s.enabled);
            case "sound" -> s.sound = EwoPvpSounds.fromToken(val);
            case "pitch" -> s.pitch = parseFloat(val, s.pitch);
            case "volume" -> s.volume = parseFloat(val, s.volume);
            default -> {}
        }
    }

    private static void applyZoneKey(Zone z, String key, String val) {
        switch (key) {
            case "enabled" -> z.enabled = parseBool(val, z.enabled);
            case "min_dist" -> z.minDist = parseFloat(val, z.minDist);
            case "max_dist" -> z.maxDist = parseFloat(val, z.maxDist);
            case "sound" -> z.sound = EwoPvpSounds.fromToken(val);
            case "pitch" -> z.pitch = parseFloat(val, z.pitch);
            case "volume" -> z.volume = parseFloat(val, z.volume);
            case "color" -> z.color = parseColor(val, z.color);
            default -> {}
        }
    }

    private String format() {
        StringBuilder sb = new StringBuilder(2048);
        sb.append("# EwoClient PvP Utils config — per client profile.\n");
        sb.append("# Schema ").append(SCHEMA_VERSION).append(", written by the in-game mod.\n");

        sb.append("\n[jump_reset]\n");
        sb.append("enabled = ").append(jumpResetEnabled).append('\n');
        sb.append("bar_enabled = ").append(jumpResetBarEnabled).append('\n');
        sb.append("proximity_window_ticks = ").append(jumpResetProximityWindowTicks).append('\n');
        sb.append("fade_ticks = ").append(jumpResetFadeTicks).append('\n');
        sb.append("bar_max_range_ms = ").append(jumpResetBarMaxRangeMs).append('\n');
        sb.append("show_all_hits = ").append(jumpResetShowAllHits).append('\n');

        sb.append("\n[hit_range]\n");
        sb.append("enabled = ").append(hitRangeEnabled).append('\n');
        sb.append("fade_ticks = ").append(hitRangeFadeTicks).append('\n');

        sb.append("\n[colors]\n");
        appendHex(sb, "perfect", colorPerfect);
        appendHex(sb, "slightly_late", colorSlightlyLate);
        appendHex(sb, "late", colorLate);
        appendHex(sb, "slightly_early", colorSlightlyEarly);
        appendHex(sb, "early", colorEarly);
        appendHex(sb, "no_reset", colorNoReset);

        appendSound(sb, "perfect", soundPerfect);
        appendSound(sb, "slightly_late", soundSlightlyLate);
        appendSound(sb, "late", soundLate);
        appendSound(sb, "slightly_early", soundSlightlyEarly);
        appendSound(sb, "early", soundEarly);

        appendZone(sb, "zone1", zone1);
        appendZone(sb, "zone2", zone2);
        appendZone(sb, "zone3", zone3);

        return sb.toString();
    }

    private static void appendHex(StringBuilder sb, String key, int color) {
        sb.append(key).append(" = \"").append(String.format("%06X", color & 0xFFFFFF)).append("\"\n");
    }

    private static void appendSound(StringBuilder sb, String label, SoundSlot s) {
        sb.append("\n[sounds.").append(label).append("]\n");
        sb.append("enabled = ").append(s.enabled).append('\n');
        sb.append("sound = \"").append(s.sound.name()).append("\"\n");
        sb.append("pitch = ").append(s.pitch).append('\n');
        sb.append("volume = ").append(s.volume).append('\n');
    }

    private static void appendZone(StringBuilder sb, String label, Zone z) {
        sb.append("\n[zones.").append(label).append("]\n");
        sb.append("enabled = ").append(z.enabled).append('\n');
        sb.append("min_dist = ").append(z.minDist).append('\n');
        sb.append("max_dist = ").append(z.maxDist).append('\n');
        sb.append("sound = \"").append(z.sound.name()).append("\"\n");
        sb.append("pitch = ").append(z.pitch).append('\n');
        sb.append("volume = ").append(z.volume).append('\n');
        appendHex(sb, "color", z.color);
    }

    // ── tiny value parsers ────────────────────────────────────────────────

    private static String unquote(String s) {
        if (s.length() >= 2 && s.startsWith("\"") && s.endsWith("\"")) {
            return s.substring(1, s.length() - 1);
        }
        return s;
    }

    private static boolean parseBool(String s, boolean fallback) {
        if ("true".equalsIgnoreCase(s)) return true;
        if ("false".equalsIgnoreCase(s)) return false;
        return fallback;
    }

    private static int parseInt(String s, int fallback) {
        try { return Integer.parseInt(s); } catch (NumberFormatException e) { return fallback; }
    }

    private static float parseFloat(String s, float fallback) {
        try { return Float.parseFloat(s); } catch (NumberFormatException e) { return fallback; }
    }

    private static int parseColor(String s, int fallback) {
        String t = s;
        if (t.startsWith("#")) t = t.substring(1);
        try { return Integer.parseInt(t, 16) & 0xFFFFFF; }
        catch (NumberFormatException e) { return fallback; }
    }

    // ── path resolution ───────────────────────────────────────────────────

    /** {@code %APPDATA%/EwoClient/profiles/<active>/pvp.toml} — per profile,
     *  same convention as {@code hud.toml} in {@code hud.rs}. */
    private static Path configPath() {
        String appdata = System.getenv("APPDATA");
        if (appdata == null || appdata.isEmpty()) return null;
        String profile = readActiveProfile();
        return Paths.get(appdata, "EwoClient", "profiles", profile, "pvp.toml");
    }

    /** The active client profile name — from {@code profiles.toml}'s
     *  {@code active = "Name"} line, defaulting to {@code "Default"}. */
    private static String readActiveProfile() {
        String appdata = System.getenv("APPDATA");
        if (appdata == null || appdata.isEmpty()) return "Default";
        Path p = Paths.get(appdata, "EwoClient", "profiles.toml");
        if (!Files.isRegularFile(p)) return "Default";
        try {
            for (String line : Files.readAllLines(p)) {
                String t = line.trim();
                if (t.startsWith("active") && t.contains("=")) {
                    return unquote(t.substring(t.indexOf('=') + 1).trim());
                }
            }
        } catch (IOException ignored) {
        }
        return "Default";
    }
}
