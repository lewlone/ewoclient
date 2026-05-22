package dev.lewlone.ewohud;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.Map;

import net.minecraft.client.Minecraft;
import org.lwjgl.glfw.GLFW;

/**
 * Reads the launcher-written {@code ewo-keybinds.txt} from the game directory
 * and exposes the bound GLFW key code for every EwoClient action — Phase F5c
 * (the overlay key) extended in Phase G with the per-module keys.
 *
 * <p>The launcher resolves the active client profile's keybinds and drops this
 * file into the instance directory before launch. Each line is
 * {@code action=code} (or {@code action=code:mods}); the file is parsed once,
 * lazily, on first use. If it is missing or unparseable every action is left
 * unbound (code {@code 0}) except the overlay key, which falls back to its
 * built-in default — so input can never break.
 */
public final class EwoKeybinds {
    private EwoKeybinds() {}

    /** Built-in default for the overlay-open key — matches the registry. */
    private static final int DEFAULT_OVERLAY_KEY = GLFW.GLFW_KEY_RIGHT_SHIFT;

    private static volatile boolean loaded = false;
    private static int overlayKey = DEFAULT_OVERLAY_KEY;
    /** action id &rarr; GLFW key code, for every line in the file. */
    private static final Map<String, Integer> codes = new HashMap<>();

    /** GLFW code for the overlay-open key; Right Shift until the file loads. */
    public static int overlayKey() {
        if (!loaded) {
            load();
        }
        return overlayKey;
    }

    /**
     * GLFW code bound to {@code actionId}, or {@code 0} (unbound) if the action
     * has no binding. {@code 0} is never a real GLFW key, so an unbound action
     * simply never matches a key press.
     */
    public static int code(String actionId) {
        if (!loaded) {
            load();
        }
        Integer c = codes.get(actionId);
        return c == null ? 0 : c;
    }

    private static synchronized void load() {
        if (loaded) {
            return;
        }
        loaded = true; // one shot — the file is written before the JVM starts
        try {
            Minecraft mc = Minecraft.getInstance();
            if (mc == null) {
                return;
            }
            Path file = mc.gameDirectory.toPath().resolve("ewo-keybinds.txt");
            if (!Files.exists(file)) {
                return;
            }
            for (String raw : Files.readAllLines(file)) {
                String line = raw.trim();
                if (line.isEmpty() || line.startsWith("#")) {
                    continue;
                }
                int eq = line.indexOf('=');
                if (eq < 0) {
                    continue;
                }
                String id = line.substring(0, eq).trim();
                String value = line.substring(eq + 1).trim();
                // `code` or `code:mods` — Phase G only needs the key code.
                int colon = value.indexOf(':');
                if (colon >= 0) {
                    value = value.substring(0, colon);
                }
                int c = Integer.parseInt(value.trim());
                codes.put(id, c);
                if (id.equals("overlay.open")) {
                    overlayKey = c;
                }
            }
        } catch (Throwable e) {
            // Keybinds are not worth crashing input over — fall back silently.
            System.err.println("[ewo-hud] keybinds load failed: " + e);
        }
    }
}
