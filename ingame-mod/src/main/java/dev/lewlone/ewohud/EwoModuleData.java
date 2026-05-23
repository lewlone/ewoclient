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

    /** Layout version — must match {@code modules.rs} SCHEMA_VERSION. */
    public static final int SCHEMA_VERSION = 1;
    /** Fixed buffer size — generous headroom past the current module set. */
    public static final int CAPACITY = 256;
    /** Module count — must equal {@code ewo_core::modules::REGISTRY.len()}. */
    public static final int MODULE_COUNT = 12;

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

    private static final int OFF_RECORDS = 8;  // past i32 schema + i32 count
    private static final int RECORD = 16;      // i32 enabled, f32 s0, f32 s1, i32 reserved

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

    /** Setting {@code slot} (0 or 1) of module {@code index}. */
    public static float setting(int index, int slot) {
        if (buffer == null || index < 0 || index >= MODULE_COUNT
                || slot < 0 || slot > 1) {
            return 0f;
        }
        return buffer.getFloat(OFF_RECORDS + index * RECORD + 4 + slot * 4);
    }
}
