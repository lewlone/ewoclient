package dev.lewlone.ewohud;

import java.nio.ByteBuffer;

/**
 * JNI surface of the {@code ewo-jni} native library (Rust + Skia).
 *
 * <p>Phase E. The native methods are implemented in
 * {@code crates/ewo-jni/src/lib.rs}; the symbol names are derived from this
 * class's fully-qualified name, so neither side may be renamed independently.
 *
 * <p>All methods must be invoked on Minecraft's render thread with the GL
 * context current.
 */
public final class EwoHudNative {
    private EwoHudNative() {}

    /** Liveness check: proves the cdylib loaded and JNI linkage works. */
    public static native void nativeHello();

    /**
     * Register the shared JVM&rarr;Rust data block. Called once at mod init.
     * Rust resolves the direct buffer's address and reads it every frame
     * thereafter with no further JNI marshaling.
     *
     * @param buffer a direct {@link ByteBuffer} ({@link EwoHudData#allocate()}).
     */
    public static native void nativeInit(ByteBuffer buffer);

    /** Paint + composite one HUD frame from the shared data block. */
    public static native void nativeRender();
}
