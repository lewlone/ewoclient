// Prints `Double.toString` / `Float.toString` / `Integer.toString` for the
// vector `rewo_net::chat_translate`'s tests pin.
//
// WHY THIS EXISTS. A translatable component's `with` list can hold a number,
// and `TranslatableContents.getArgument` renders it with `arg.toString()`.
// Which Java type that is was settled by reading the decompiled
// `com.mojang.serialization.JavaOps` out of datafixerupper-10.0.21.jar
// (it preserves the NBT tag's numeric width), but the *text* those types
// produce is `java.lang.Double.toString`, which is in the JDK and therefore in
// no decompile this project holds. So it is graded against a real JVM, the
// same way M114 graded brigadier's suggestion primitives against the real jar.
//
//   javac ToStringOracle.java && java ToStringOracle
//
// Re-run after a JDK bump. Java 19+ uses the Raffaello Giulietti shortest
// representation; an older JVM will disagree on some rows and that
// disagreement is the point of re-running.
public final class ToStringOracle {
    public static void main(String[] args) {
        // Longs: the width boundaries and the sign.
        long[] longs = {0L, 1L, -1L, 127L, -128L, 32767L, 2147483647L, -2147483648L, 9223372036854775807L};
        for (long v : longs) {
            System.out.println("long\t" + v + "\t" + Long.toString(v));
        }

        // Doubles: the two scientific-notation thresholds from both sides,
        // the always-a-decimal-point rule, and the non-finite trio.
        double[] doubles = {
            0.0, -0.0, 1.0, -1.0, 3.0, 0.5, 100.0,
            1.0E-3, 9.999E-4, 1.0E7, 9999999.0, 1.0E-4, 1.0E8,
            1.23E-5, 1.0E20, 0.1, 1.0 / 3.0,
            Double.NaN, Double.POSITIVE_INFINITY, Double.NEGATIVE_INFINITY,
        };
        for (double v : doubles) {
            System.out.println("double\t" + Double.doubleToRawLongBits(v) + "\t" + Double.toString(v));
        }

        // Floats: the same thresholds, which are the same numbers for float.
        float[] floats = {
            0.0f, 1.0f, 1.5f, 0.1f, 1.0E7f, 9999999.0f, 1.0E-3f, 9.999E-4f, 3.4028235E38f,
        };
        for (float v : floats) {
            System.out.println("float\t" + Float.floatToRawIntBits(v) + "\t" + Float.toString(v));
        }
    }
}
