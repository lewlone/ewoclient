package dev.lewlone.ewohud;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;

/**
 * The Rust&rarr;JVM module-state block.
 *
 * <p>A direct {@link ByteBuffer} the mod allocates once and hands to Rust via
 * {@code nativeInitModules}. Rust writes it every frame with each EwoClient
 * module's enabled flag and settings; the mod reads it to drive the module
 * effect mixins.
 *
 * <p>This is the mirror image of {@link EwoHudData}: that block is JVM&rarr;Rust
 * (game state in), this one is Rust&rarr;JVM (module state out). The layout is
 * mirrored byte-for-byte in {@code crates/ewo-jni/src/modules.rs};
 * {@link #SCHEMA_VERSION} guards the two sides against drift.
 *
 * <p><b>Slot layout (schema 3 / post-ban refactor).</b> Legit slots live here
 * and are always written (slots 0..11). Assist slots (12..25) are only
 * written when both the Rust side and Java side are built with the pvp
 * feature — their identifiers and behaviour live in
 * {@code dev.lewlone.ewohud.assist.AssistSlots} so the legit jar doesn't
 * carry them in its constant pool. Schema 3 deleted {@code auto_crit}
 * (deprecated no-op), {@code mace_combo} (tick-perfect kill chain), and
 * {@code wind_charge_mlg} (snap-pitch was literal aim assist); renamed
 * {@code triggerbot} to {@code swing_cadence} with humanized cadence.
 */
public final class EwoModuleData {
    private EwoModuleData() {}

    /** Layout version — must match {@code modules.rs} SCHEMA_VERSION.
     *  <p>Schema 3 (post-ban refactor, 2026-05-26): rebalanced slot ordering
     *  (legit slots 0..11 first, assist 12..25 second), deleted 3 modules
     *  (auto_crit/mace_combo/wind_charge_mlg), renamed triggerbot to
     *  swing_cadence. {@code modules.toml} is id-keyed so per-module settings
     *  survive the bump; the in-memory buffer layout is what changes. */
    public static final int SCHEMA_VERSION = 3;
    /** Fixed buffer size — fits ~100 modules at the schema-3 stride; gives
     *  the catalog plenty of headroom before another bump is needed. */
    public static final int CAPACITY = 4096;
    /** Hard upper bound on module count; the real count is what Rust writes
     *  at offset 4 and {@link #moduleCount()} returns. Sized for the full
     *  pvp registry plus headroom. */
    public static final int MAX_MODULES = 32;

    // Legit module slot constants — always written by the Rust side. Pvp
    // slot constants live in dev.lewlone.ewohud.assist.AssistSlots and only
    // exist in pvp builds.
    public static final int FULLBRIGHT          = 0;
    public static final int FOV                 = 1;
    public static final int TOGGLE_SPRINT       = 2;
    public static final int TOGGLE_SNEAK        = 3;
    public static final int NO_DAMAGE_TILT      = 4;
    public static final int NO_VIEW_BOB         = 5;
    public static final int FREELOOK            = 6;
    public static final int NO_FIRE_OVERLAY     = 7;
    public static final int CROSSHAIR_ON_REACH  = 8;
    public static final int NO_PUMPKIN_OVERLAY  = 9;
    public static final int HIT_COLOR           = 10;
    public static final int HIT_INDICATOR       = 11;

    private static final int OFF_RECORDS = 8;  // past i32 schema + i32 count
    /** Schema 3: 4 + 8*4 + 4 = 40 bytes per module record. Must mirror
     *  {@code modules.rs}: {@code 8 + catalog::MAX_SETTINGS * 4}. */
    private static final int RECORD = 40;
    /** Max settings per module — mirrors {@code ewo_core::modules::MAX_SETTINGS}. */
    private static final int MAX_SETTINGS_PER_MODULE = 8;

    private static ByteBuffer buffer;

    /** Allocate the shared block. Called once, before {@code nativeInitModules}. */
    public static ByteBuffer allocate() {
        buffer = ByteBuffer.allocateDirect(CAPACITY).order(ByteOrder.nativeOrder());
        return buffer;
    }

    /** True once Rust has written a block carrying the schema the mod expects. */
    public static boolean ready() {
        return buffer != null && buffer.getInt(0) == SCHEMA_VERSION;
    }

    /** Module count Rust last reported. In a legit-build pairing this is 12;
     *  in a matched pvp pairing it's 26. Indices at or past this value have
     *  not been written and read as disabled / zero. */
    public static int moduleCount() {
        return buffer == null ? 0 : buffer.getInt(4);
    }

    /** Whether module {@code index} is currently enabled. Out-of-range
     *  (including assist slots in a legit-build pairing) returns false. */
    public static boolean enabled(int index) {
        if (buffer == null || index < 0 || index >= moduleCount()) {
            return false;
        }
        return buffer.getInt(OFF_RECORDS + index * RECORD) != 0;
    }

    /** Setting {@code slot} (0..{@link #MAX_SETTINGS_PER_MODULE}-1) of module
     *  {@code index}. */
    public static float setting(int index, int slot) {
        if (buffer == null || index < 0 || index >= moduleCount()
                || slot < 0 || slot >= MAX_SETTINGS_PER_MODULE) {
            return 0f;
        }
        return buffer.getFloat(OFF_RECORDS + index * RECORD + 4 + slot * 4);
    }
}
