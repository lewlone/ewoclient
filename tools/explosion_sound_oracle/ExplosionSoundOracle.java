// Independent oracle for M161's explosion-sound draws — run on a REAL JVM.
//
// `ClientLevel.playLocalSound(.., 4.0F,
//    (1.0F + (level.getRandom().nextFloat() - level.getRandom().nextFloat()) * 0.2F) * 0.7F,
//    false)` and then, inside `playLocalSound`, `SimpleSoundInstance`'s seed
// from `this.random.nextLong()`. Three consecutive draws off ONE generator,
// in that order.
//
// `java.util.Random` is an exact stand-in for `LegacyRandomSource` for these
// three calls, which is what makes this an oracle rather than a second copy:
//   * `LegacyRandomSource.next(bits)` is the same 48-bit LCG — multiplier
//     25214903917, increment 11, mask 281474976710655 — and the same
//     `seed ^ MULTIPLIER` scramble in `setSeed`.
//   * `BitRandomSource.nextFloat()` is `next(24) * 5.9604645E-8F`, and
//     `1.0f / (1 << 24)` IS that float, so it is `java.util.Random.nextFloat()`
//     bit for bit.
//   * `BitRandomSource.nextLong()` is `((long)next(32) << 32) + next(32)` —
//     the same SIGNED `+` java.util.Random uses, which is the half a
//     `(hi << 32) | lo` reading gets wrong on about every second draw.
// So this file grades Rewo's transcription against the platform rather than
// against a second transcription of the same paragraph.
//
// Re-run after any change to the pitch expression or the draw order:
//
//   java tools/explosion_sound_oracle/ExplosionSoundOracle.java
//
// and paste the printed values into
// `rewo_net::motion::tests::the_explosion_sound_matches_a_real_jvm`.
import java.util.HashSet;
import java.util.Random;

public final class ExplosionSoundOracle {
    public static void main(String[] args) {
        long[] seeds = {0L, 1L, 0x5EEDA11B1E17L, -1L, 1234567890123L};
        for (long s : seeds) {
            Random r = new Random(s);
            float a = r.nextFloat();
            float b = r.nextFloat();
            float pitch = (1.0F + (a - b) * 0.2F) * 0.7F;
            long soundSeed = r.nextLong();
            System.out.printf(
                    "seed=%d a=%s b=%s pitchBits=0x%08X pitch=%s soundSeed=%d%n",
                    s,
                    Float.toString(a),
                    Float.toString(b),
                    Float.floatToRawIntBits(pitch),
                    Float.toString(pitch),
                    soundSeed);
        }

        // The distinctness claim, measured rather than assumed: how many
        // DISTINCT seeds 2000 consecutive explosions off one generator produce.
        // A constant seed collapses this to 1 and makes every explosion in the
        // game pick the same one of `entity.generic.explode`'s four variants.
        Random r = new Random(0x5EEDA11B1E17L);
        HashSet<Long> seen = new HashSet<>();
        for (int i = 0; i < 2000; i++) {
            r.nextFloat();
            r.nextFloat();
            seen.add(r.nextLong());
        }
        System.out.println("distinct sound seeds over 2000 explosions = " + seen.size());

        // And the ORDER, which is separately guessable: drawing the seed
        // FIRST gives a different pitch and a different seed from the same
        // start, so a witness pinning both cannot be satisfied by a reordering.
        Random q = new Random(0x5EEDA11B1E17L);
        long seedFirst = q.nextLong();
        float qa = q.nextFloat();
        float qb = q.nextFloat();
        System.out.printf(
                "seed-first ordering from 0x5EEDA11B1E17: soundSeed=%d pitchBits=0x%08X%n",
                seedFirst,
                Float.floatToRawIntBits((1.0F + (qa - qb) * 0.2F) * 0.7F));
    }
}
