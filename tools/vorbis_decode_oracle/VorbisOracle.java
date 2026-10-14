import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import javax.sound.sampled.AudioFormat;
import net.minecraft.client.sounds.JOrbisAudioStream;

/// Runs vanilla 26.2's OWN Ogg Vorbis decoder (`JOrbisAudioStream.readAll()`,
/// the `SoundBufferLibrary.getCompleteBuffer` path) over a fixed sample of the
/// local asset store and prints, per file, the exact frame count and a hash of
/// the 16-bit PCM bytes it hands to OpenAL.
///
///     pwsh tools/vorbis_decode_oracle/run.ps1
///
/// WHY. `crates/rewo-audio/src/decode.rs` decodes with symphonia, and Vorbis I
/// does not mandate bit-identical float output, so the decoded VALUES may differ
/// in the last bits. What must not differ is the SAMPLE COUNT (start and end
/// trim): a count that drifts changes every sound's length, loop point and the
/// stream-chunk arithmetic. `JOrbisAudioStream` caps the stream at the final
/// page's granule position (`readPage`: `totalSamplesInStream = granulepos()` on
/// the EOS page; `getSamplesToWrite`), so the count is `min(decoded, granule)`,
/// and this measures it rather than assuming it.
///
/// `last_granule` is read here from the file's final Ogg page, independently of
/// jorbis, so a row shows both what vanilla produced and the figure it capped to.
///
/// Output (TSV, `#` lines are comments):
///     name  hash  channels  rate  frames  fnv1a64(pcm bytes)  last_granule
///     sum(all i16 samples)  first 64 samples  last 64 samples
///
/// The sample is deterministic: the N smallest ogg files, then an even spread
/// over the size-sorted rest (up to MAX_BYTES), plus three named anchors. Rows
/// are consumed by `decode.rs`'s `real_assets` tests; they are checked in, and no
/// gate needs a JVM. ASCII only.
public class VorbisOracle {
    /// Samples (interleaved) kept from each end of the PCM, to check alignment.
    static final int EDGE = 64;
    static final int SMALLEST = 14;
    static final int SPREAD = 40;
    static final long MAX_BYTES = 2_500_000L;
    static final String[] ANCHORS = {
        "minecraft/sounds/mob/chicken/step1.ogg",
        "minecraft/sounds/item/goat_horn/call0.ogg",
        "minecraft/sounds/item/goat_horn/call3.ogg",
    };

    record Entry(String name, String hash, long size) {}

    public static void main(String[] args) throws Exception {
        Path assets = Paths.get(args[0]); // .../shared/assets
        String indexId = args[1];
        JsonObject objects = JsonParser.parseString(
                Files.readString(assets.resolve("indexes").resolve(indexId + ".json")))
            .getAsJsonObject().getAsJsonObject("objects");
        List<Entry> all = new ArrayList<>();
        Set<String> seen = new HashSet<>();
        for (var e : objects.entrySet()) {
            String name = e.getKey();
            if (!name.startsWith("minecraft/sounds/") || !name.endsWith(".ogg")) continue;
            JsonObject o = e.getValue().getAsJsonObject();
            String hash = o.get("hash").getAsString();
            if (!seen.add(hash)) continue;
            all.add(new Entry(name, hash, o.get("size").getAsLong()));
        }
        all.sort(Comparator.comparingLong(Entry::size).thenComparing(Entry::name));

        List<Entry> pick = new ArrayList<>();
        Set<String> picked = new HashSet<>();
        for (int i = 0; i < SMALLEST && i < all.size(); i++) add(pick, picked, all.get(i));
        List<Entry> mid = new ArrayList<>();
        for (Entry e : all) if (e.size() <= MAX_BYTES) mid.add(e);
        for (int i = 0; i < SPREAD; i++) {
            add(pick, picked, mid.get((int) ((long) i * (mid.size() - 1) / (SPREAD - 1))));
        }
        for (String a : ANCHORS) {
            for (Entry e : all) if (e.name().equals(a)) add(pick, picked, e);
        }
        pick.sort(Comparator.comparing(Entry::name));

        System.out.println("# vorbis_decode_oracle: vanilla 26.2 JOrbisAudioStream.readAll()");
        System.out.println("# name\thash\tchannels\trate\tframes\tfnv1a64\tlast_granule\tsum\thead64\ttail64");
        for (Entry e : pick) {
            Path p = assets.resolve("objects").resolve(e.hash().substring(0, 2)).resolve(e.hash());
            byte[] bytes = Files.readAllBytes(p);
            try (JOrbisAudioStream s = new JOrbisAudioStream(new ByteArrayInputStream(bytes))) {
                ByteBuffer data = s.readAll();
                AudioFormat f = s.getFormat();
                int ch = f.getChannels();
                int n = data.remaining();
                long h = 0xcbf29ce484222325L;
                for (int i = 0; i < n; i++) {
                    h ^= (data.get(data.position() + i) & 0xffL);
                    h *= 0x100000001b3L;
                }
                long frames = (long) n / 2 / ch;
                data.order(ByteOrder.LITTLE_ENDIAN);
                int count = n / 2;
                long sum = 0;
                for (int i = 0; i < count; i++) sum += data.getShort(data.position() + 2 * i);
                int k = Math.min(EDGE, count);
                StringBuilder head = new StringBuilder();
                StringBuilder tail = new StringBuilder();
                for (int i = 0; i < k; i++) {
                    if (i > 0) {
                        head.append(',');
                        tail.append(',');
                    }
                    head.append(data.getShort(data.position() + 2 * i));
                    tail.append(data.getShort(data.position() + 2 * (count - k + i)));
                }
                System.out.printf("%s\t%s\t%d\t%d\t%d\t%016x\t%d\t%d\t%s\t%s%n",
                    e.name(), e.hash(), ch, (int) f.getSampleRate(), frames, h, lastGranule(bytes),
                    sum, head, tail);
            } catch (IOException ex) {
                System.out.printf("%s\t%s\tERR\t%s%n", e.name(), e.hash(), ex.getMessage());
            }
        }
    }

    static void add(List<Entry> pick, Set<String> picked, Entry e) {
        if (picked.add(e.hash())) pick.add(e);
    }

    /// The granule position of the file's last Ogg page (bytes 6..14, LE).
    static long lastGranule(byte[] b) {
        for (int i = b.length - 27; i >= 0; i--) {
            if (b[i] == 'O' && b[i + 1] == 'g' && b[i + 2] == 'g' && b[i + 3] == 'S') {
                long g = 0;
                for (int k = 7; k >= 0; k--) g = (g << 8) | (b[i + 6 + k] & 0xffL);
                return g;
            }
        }
        return -1;
    }
}
