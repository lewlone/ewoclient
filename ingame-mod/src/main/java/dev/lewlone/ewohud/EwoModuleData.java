package dev.lewlone.ewohud;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;

/**
 * The Rust&rarr;JVM module-state block (Phase G).
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
 */
public final class EwoModuleData {
    private EwoModuleData() {}

    /** Layout version — must match {@code modules.rs} SCHEMA_VERSION.
     *  <p>Schema 2 (2026-05): record stride grew 16 → 40 bytes (MAX_SETTINGS
     *  2 → 8) and {@link #CAPACITY} grew 256 → 4096. Schema 1 was over-
     *  capacity at 17 modules — the 17th record overflowed the 256-byte
     *  buffer, causing IndexOutOfBoundsException reads and undefined
     *  behavior on the Rust unsafe write side. */
    public static final int SCHEMA_VERSION = 2;
    /** Fixed buffer size — fits ~100 modules at the schema-2 stride; gives
     *  the catalog plenty of headroom before another bump is needed. */
    public static final int CAPACITY = 4096;
    /** Module count — must equal {@code ewo_core::modules::REGISTRY.len()}. */
    public static final int MODULE_COUNT = 24;

    // Module indices — mirror of modules::REGISTRY order. The effect mixins
    // reference these by name; reordering needs a SCHEMA_VERSION bump.
    public static final int FULLBRIGHT = 0;
    public static final int FOV = 1;
    public static final int TOGGLE_SPRINT = 2;
    public static final int TOGGLE_SNEAK = 3;
    public static final int NO_DAMAGE_TILT = 4;
    public static final int NO_VIEW_BOB = 5;
    public static final int FREELOOK = 6;
    public static final int NO_FIRE_OVERLAY = 7;
    public static final int CROSSHAIR_ON_REACH = 8;
    public static final int AUTO_TOOL = 9;
    public static final int AUTO_TOTEM = 10;
    public static final int LEGIT_ELYTRA_SWAP = 11;
    public static final int HAND_RESTOCK = 12;
    public static final int NO_PUMPKIN_OVERLAY = 13;
    public static final int HIT_COLOR = 14;
    public static final int SPRINT_TAP = 15;
    public static final int AUTO_EAT = 16;
    public static final int AUTO_MACE_SWAP = 17;
    public static final int AUTO_JUMP_RESET = 18;
    public static final int AUTO_CRIT = 19;
    public static final int AUTO_PEARL = 20;
    public static final int RIPTIDE_BOOST = 21;
    public static final int MACE_COMBO = 22;
    public static final int WIND_CHARGE_MLG = 23;

    private static final int OFF_RECORDS = 8;  // past i32 schema + i32 count
    /** Schema 2: 4 + 8*4 + 4 = 40 bytes per module record. Must mirror
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

    /** Module count Rust last reported — for the drift check and logging. */
    public static int moduleCount() {
        return buffer == null ? 0 : buffer.getInt(4);
    }

    /** Whether module {@code index} is currently enabled. */
    public static boolean enabled(int index) {
        if (buffer == null || index < 0 || index >= MODULE_COUNT) {
            return false;
        }
        return buffer.getInt(OFF_RECORDS + index * RECORD) != 0;
    }

    /** Setting {@code slot} (0..{@link #MAX_SETTINGS_PER_MODULE}-1) of module
     *  {@code index}. */
    public static float setting(int index, int slot) {
        if (buffer == null || index < 0 || index >= MODULE_COUNT
                || slot < 0 || slot >= MAX_SETTINGS_PER_MODULE) {
            return 0f;
        }
        return buffer.getFloat(OFF_RECORDS + index * RECORD + 4 + slot * 4);
    }
}
