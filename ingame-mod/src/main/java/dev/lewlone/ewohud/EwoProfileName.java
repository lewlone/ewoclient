package dev.lewlone.ewohud;

import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.List;

/**
 * The active client profile name, read from {@code profiles.toml}, and the
 * rule for whether it may become a path component. A Java mirror of the Rust
 * side: {@code ewo_core::pvp::{active_profile_in, profile_file_in}} and
 * {@code ewo_core::util::validate_name}.
 *
 * <p>The name comes from a user-editable file, so it is untrusted: a
 * hand-edited {@code active = "../.."} must not steer a read or write outside
 * {@code %APPDATA%/EwoClient/profiles/}. Rust refuses such a name (every
 * per-profile file there goes through {@code profile_file_in}); the mod used
 * to join the raw string, so the two sides disagreed about which directory a
 * profile meant. Both now apply the same rule.
 *
 * <p>The constants below are pinned to their Rust twins by tests in
 * {@code ewo-jni} ({@code java_profile_rule_matches_rust}); change them
 * together.
 */
public final class EwoProfileName {

    /** {@code ewo_core::pvp::MAX_PROFILE_NAME_LEN}, in code points. */
    static final int MAX_PROFILE_NAME_LEN = 48;

    /** The characters {@code validate_name} forbids outright (besides
     *  control characters). */
    static final String FORBIDDEN_CHARS = "/\\:*?\"<>|";

    /** The profile used when {@code profiles.toml} is missing or silent. */
    public static final String DEFAULT_PROFILE = "Default";

    private EwoProfileName() {}

    /**
     * Whether {@code profile} may be joined into a path. Exactly the names
     * Rust's {@code profile_file_in} accepts: non-empty after trimming and
     * already trimmed, at most {@link #MAX_PROFILE_NAME_LEN} code points, no
     * control or {@link #FORBIDDEN_CHARS} characters, no {@code ..}, not
     * ending in a dot, and not a reserved Windows device name.
     */
    public static boolean isSafe(String profile) {
        if (profile == null || profile.isEmpty()) return false;
        // `validate_name(profile)? == profile`: nothing for trim() to remove.
        if (isRustWhitespace(profile.codePointAt(0))
                || isRustWhitespace(profile.codePointBefore(profile.length()))) {
            return false;
        }
        if (profile.codePointCount(0, profile.length()) > MAX_PROFILE_NAME_LEN) return false;
        for (int i = 0; i < profile.length(); ) {
            int cp = profile.codePointAt(i);
            i += Character.charCount(cp);
            if (Character.getType(cp) == Character.CONTROL
                    || (cp < 0x80 && FORBIDDEN_CHARS.indexOf(cp) >= 0)) {
                return false;
            }
        }
        if (profile.equals(".") || profile.endsWith(".") || profile.contains("..")) return false;
        // Windows device names, with or without an extension.
        int dot = profile.indexOf('.');
        String stem = trimEnd(dot < 0 ? profile : profile.substring(0, dot));
        stem = asciiUpper(stem);
        if (stem.equals("CON") || stem.equals("PRN") || stem.equals("AUX") || stem.equals("NUL")) {
            return false;
        }
        if ((stem.startsWith("COM") || stem.startsWith("LPT")) && stem.length() == 4) {
            char d = stem.charAt(3);
            if (d >= '1' && d <= '9') return false;
        }
        return true;
    }

    /**
     * {@code <appdata>/EwoClient/profiles/<profile>/<file>}, or {@code null}
     * when {@code appdata} is unset or {@code profile} is not {@linkplain
     * #isSafe safe}. Same contract as Rust's {@code profile_file_in}.
     */
    public static Path profileFile(String appdata, String profile, String file) {
        if (appdata == null || appdata.isEmpty() || !isSafe(profile)) return null;
        return Paths.get(appdata, "EwoClient", "profiles", profile, file);
    }

    /**
     * The {@code active = "Name"} line of {@code profiles.toml}, parsed like
     * Rust's {@code active_profile_in}: the first line that, once trimmed,
     * starts with {@code active} followed (after optional whitespace) by
     * {@code =}; the value is trimmed and stripped of every leading and
     * trailing quote. {@link #DEFAULT_PROFILE} when there is no such line.
     * The result is <em>not</em> validated: pass it to {@link #isSafe}.
     */
    public static String activeFromToml(List<String> lines) {
        for (String line : lines) {
            String t = trim(line);
            if (!t.startsWith("active")) continue;
            String rest = trimStart(t.substring("active".length()));
            if (!rest.startsWith("=")) continue;
            String v = trim(rest.substring(1));
            int lo = 0;
            int hi = v.length();
            while (lo < hi && v.charAt(lo) == '"') lo++;
            while (hi > lo && v.charAt(hi - 1) == '"') hi--;
            return v.substring(lo, hi);
        }
        return DEFAULT_PROFILE;
    }

    // -- Rust's str::trim family (Unicode White_Space), not String.trim() ----

    /** {@code char::is_whitespace}: the Unicode {@code White_Space} property. */
    static boolean isRustWhitespace(int cp) {
        return (cp >= 0x09 && cp <= 0x0D) || cp == 0x20 || cp == 0x85 || cp == 0xA0
                || cp == 0x1680 || (cp >= 0x2000 && cp <= 0x200A) || cp == 0x2028
                || cp == 0x2029 || cp == 0x202F || cp == 0x205F || cp == 0x3000;
    }

    private static String trim(String s) {
        return trimEnd(trimStart(s));
    }

    private static String trimStart(String s) {
        int i = 0;
        while (i < s.length()) {
            int cp = s.codePointAt(i);
            if (!isRustWhitespace(cp)) break;
            i += Character.charCount(cp);
        }
        return s.substring(i);
    }

    private static String trimEnd(String s) {
        int end = s.length();
        while (end > 0) {
            int cp = s.codePointBefore(end);
            if (!isRustWhitespace(cp)) break;
            end -= Character.charCount(cp);
        }
        return s.substring(0, end);
    }

    /** {@code str::to_ascii_uppercase}: only {@code a-z} change. */
    private static String asciiUpper(String s) {
        StringBuilder sb = new StringBuilder(s.length());
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            sb.append(c >= 'a' && c <= 'z' ? (char) (c - 32) : c);
        }
        return sb.toString();
    }
}
