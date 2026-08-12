//! `SoundBufferLibrary` — what gets cached, what never does, and the loop flag.
//!
//! Transcribed from `SoundBufferLibrary.java`. The decoder is a trait rather
//! than a concrete Vorbis reader, because the caching rules are the part with a
//! vanilla answer and they can be graded without one: a fake source that counts
//! its calls proves "cached permanently" and "never cached" far more directly
//! than a real `.ogg` would.
//!
//! ```java
//! private final Map<Identifier, CompletableFuture<SoundBuffer>> cache = Maps.newHashMap();
//!
//! public CompletableFuture<SoundBuffer> getCompleteBuffer(final Identifier location) {
//!    return this.cache.computeIfAbsent(location, l -> CompletableFuture.supplyAsync(...));
//! }
//!
//! public CompletableFuture<AudioStream> getStream(final Identifier location, final boolean looping) {
//!    return CompletableFuture.supplyAsync(() -> {
//!       InputStream is = this.resourceManager.open(location);
//!       return looping ? new LoopingAudioStream(JOrbisAudioStream::new, is) : new JOrbisAudioStream(is);
//!    }, ...);
//! }
//! ```
//! (`:26-47`.)

use std::collections::HashMap;
use std::sync::Arc;

/// Decoded PCM, as a static buffer holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Pcm {
    /// Interleaved 16-bit samples, already through
    /// [`crate::quantise::quantise`].
    pub samples: Vec<i16>,
    pub channels: u16,
    pub sample_rate: u32,
}

/// Where PCM comes from — the seam a real Vorbis decoder fills.
///
/// `open` takes an **asset-index key** of the form `<namespace>/sounds/<path>.ogg`
/// (`ResolvedSound::asset_key`), not a filesystem path: the store is
/// content-addressed, so the key resolves through the index to
/// `<assets>/objects/<hash[0..2]>/<hash>`. A decoder that treated the string as
/// a path would find nothing, on every sound, with a perfectly good error.
pub trait PcmSource {
    fn open(&mut self, key: &str) -> Result<Pcm, String>;

    /// `getStream` — the same asset, read incrementally (M144).
    ///
    /// **A default method, so every existing fake is unchanged.** The caching
    /// rules above are graded against a source that counts its calls and never
    /// touches a decoder, which is M138b's design point; making streams a
    /// required method would have forced all of those to grow an ogg.
    ///
    /// `looping` selects `LoopingAudioStream` over a bare `JOrbisAudioStream`
    /// and belongs *here* rather than on the channel — see [`StreamHandle`].
    fn open_stream(
        &mut self,
        key: &str,
        looping: bool,
    ) -> Result<Box<dyn PcmStream>, String> {
        let _ = looping;
        Err(format!("{key}: this source cannot open streams"))
    }
}

/// `AudioStream` — an asset read a chunk at a time instead of all at once.
///
/// **Streaming is not an optimisation here, it is the only option.** Measured
/// against the real 26.2 store: `music.end` is 11.3 MB of ogg, roughly 806
/// seconds, which is **142 MB in one PCM buffer** — and 344 of 8,024 variants
/// are streamed. Decoding one fully and letting the mixer's own loop flag stand
/// in for `LoopingAudioStream` is the simplification an implementer reaches for
/// first, and the number above is why it is not available.
pub trait PcmStream {
    /// `AudioStream.getFormat()` — `(channels, sample_rate)`.
    ///
    /// **Available before the first read**, because `Channel.attachBufferStream`
    /// sizes its buffers from the format *before* it pumps any
    /// (`Channel.java:125-127`). A stream that only learned its format by
    /// decoding could not answer this.
    fn format(&self) -> (u16, u32);

    /// `AudioStream.read(expectedSize)`, in **samples** rather than bytes.
    ///
    /// Returns up to `samples` interleaved `i16`s, already quantised. **An empty
    /// result means exhausted** — that is the signal `LoopingAudioStream` reads
    /// to restart, and the signal a non-looping caller reads to stop. A short
    /// non-empty result is the ordinary end of a file and is *not* exhaustion.
    fn read(&mut self, samples: usize) -> Result<Vec<i16>, String>;
}

/// `Channel.calculateBufferSize(format, seconds)` — one buffer, in **bytes**.
///
/// ```java
/// return (int)(seconds * format.getSampleSizeInBits() / 8.0F * format.getChannels() * format.getSampleRate());
/// ```
/// (`Channel.java:130-132`.) `JOrbisAudioStream` builds its format as
/// `new AudioFormat(rate, 16, channels, true, false)` (`:63`), so the sample
/// size is **16 bits** and this is `2 * channels * rate` per second.
///
/// Kept in bytes rather than converted here, because that is what vanilla's
/// `streamingBufferSize` is and the `/ 2` belongs at the one call site that
/// needs samples. The float arithmetic is transcribed and then truncated; for
/// every realistic rate the product is exactly representable in `f32`, so the
/// truncation never actually rounds — which is worth knowing before someone
/// "fixes" it into integer maths and changes nothing.
pub fn calculate_buffer_size(channels: u16, sample_rate: u32, seconds: i32) -> usize {
    let bits = 16i32;
    let bytes = seconds as f32 * bits as f32 / 8.0 * channels as f32 * sample_rate as f32;
    bytes.max(0.0) as usize
}

/// `Channel.QUEUED_BUFFER_COUNT` — how many buffers a stream keeps queued.
pub const QUEUED_BUFFER_COUNT: usize = 4;

/// `Channel.BUFFER_DURATION_SECONDS` — how much audio each one holds.
pub const BUFFER_DURATION_SECONDS: i32 = 1;

/// So a caller whose source is a closure can still name the library's type.
///
/// [`crate::decode::BytesSource`] wraps an `FnMut`, and a closure's type cannot
/// be written down — which matters because the backend that holds one has to be
/// stored in a struct field. Boxing is the way out, and it needs this impl to
/// satisfy `SoundBufferLibrary<S: PcmSource>`.
impl PcmSource for Box<dyn PcmSource> {
    fn open(&mut self, key: &str) -> Result<Pcm, String> {
        (**self).open(key)
    }
    /// Forwarded explicitly. Inheriting the trait's default here would make a
    /// boxed source silently refuse every stream while the source inside it
    /// supported them — the failure would present as "music never plays" with a
    /// perfectly good error message naming the wrong cause.
    fn open_stream(&mut self, key: &str, looping: bool) -> Result<Box<dyn PcmStream>, String> {
        (**self).open_stream(key, looping)
    }
}

/// A stream handle. Streams carry their loop flag; static buffers do not.
///
/// **Looping for a streamed sound lives HERE and not on the channel.**
/// `SoundEngine.play` sets `channel.setLooping(isLooping && !isStreaming)`
/// (`SoundEngine.java:426`) — a streamed loop is explicitly told *not* to loop
/// on the source — because `LoopingAudioStream.read` restarts the decoder when
/// a read comes back empty (`LoopingAudioStream.java:28-38`). Model the flag
/// only as an AL property and every music track and every `ambient.*.loop` bed
/// plays once and stops.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamHandle {
    pub key: String,
    pub looping: bool,
}

/// `SoundBufferLibrary`.
///
/// **The cache holds `Arc<Pcm>`, not `Pcm`.** A buffer whose whole purpose is
/// "decoded once for the life of the session" must be handed out as a shared
/// handle: a step sound fires several times a second, and copying ~170 KB of
/// samples per play would undo exactly the saving the cache exists for. It is
/// also what `Command::Attach` wants, since the same buffer is read by the
/// audio callback while the producer still holds it.
pub struct SoundBufferLibrary<S: PcmSource> {
    source: S,
    cache: HashMap<String, Result<Arc<Pcm>, String>>,
}

impl<S: PcmSource> SoundBufferLibrary<S> {
    pub fn new(source: S) -> SoundBufferLibrary<S> {
        SoundBufferLibrary {
            source,
            cache: HashMap::new(),
        }
    }

    /// `getCompleteBuffer` — **cached permanently by path**.
    ///
    /// `computeIfAbsent` with no eviction of any kind: the only way an entry
    /// leaves is [`Self::clear`], which vanilla calls on a resource reload. A
    /// sound decoded once is decoded once for the life of the session, which is
    /// what makes a static buffer cheap enough to fire dozens of times a second.
    ///
    /// **A failure is cached too**, which is `computeIfAbsent`'s doing rather
    /// than a choice: the map stores the future, and a future that completed
    /// exceptionally is still in the map. Retrying every frame on a missing file
    /// would be the more obvious design and is not vanilla's.
    /// Returned **by value**, which is an `Arc` clone on the hit path and a
    /// `String` clone on the (cached, rare) failure path. A borrow of `self`
    /// would be cheaper and would also stop the caller touching any other field
    /// of the backend while it held the buffer — which is precisely what a
    /// caller does next, since the point of the lookup is to push the result at
    /// a ring that lives beside this library.
    pub fn complete_buffer(&mut self, key: &str) -> Result<Arc<Pcm>, String> {
        if !self.cache.contains_key(key) {
            let decoded = self.source.open(key).map(Arc::new);
            self.cache.insert(key.to_string(), decoded);
        }
        self.cache[key].clone()
    }

    /// `getStream`'s **rule** — never cached, and the loop flag rides with it.
    ///
    /// There is no `computeIfAbsent` here and no map lookup at all: every call
    /// opens the resource again. That is not an oversight — a stream is stateful
    /// (it holds a decode position), so two sounds sharing one would fight over
    /// it, and the same track started twice must genuinely play twice.
    ///
    /// This states the decision without performing it, which is what lets it be
    /// graded by a source that has no decoder in it at all.
    /// [`Self::open_stream`] is the action.
    pub fn stream(&self, key: &str, looping: bool) -> StreamHandle {
        StreamHandle {
            key: key.to_string(),
            looping,
        }
    }

    /// `getStream`'s **action** — open one, honouring [`Self::stream`]'s rule.
    ///
    /// Goes straight to the source and never near `cache`, so a stream opened
    /// twice really is two independent decode positions.
    pub fn open_stream(&mut self, key: &str, looping: bool) -> Result<Box<dyn PcmStream>, String> {
        self.source.open_stream(key, looping)
    }

    /// `clear()` — a resource reload. The only thing that empties the cache.
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    /// `preload(Collection<Sound>)` — bulk `getCompleteBuffer`, same caching.
    pub fn preload(&mut self, keys: &[String]) {
        for k in keys {
            let _ = self.complete_buffer(k);
        }
    }

    /// How many distinct paths are held. `enumerate`'s debug output, reduced to
    /// the number a test can assert.
    pub fn cached(&self) -> usize {
        self.cache.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source that counts, so "cached" and "not cached" are observable
    /// directly rather than inferred from timing.
    #[derive(Default)]
    struct Counting {
        opens: Vec<String>,
        fail: bool,
    }

    impl PcmSource for Counting {
        fn open(&mut self, key: &str) -> Result<Pcm, String> {
            self.opens.push(key.to_string());
            if self.fail {
                return Err(format!("no such asset: {key}"));
            }
            Ok(Pcm {
                samples: vec![0, 1, 2],
                channels: 1,
                sample_rate: 44100,
            })
        }
    }

    #[test]
    fn a_static_buffer_is_decoded_once_and_kept() {
        let mut lib = SoundBufferLibrary::new(Counting::default());
        for _ in 0..5 {
            assert!(lib.complete_buffer("minecraft/sounds/step/grass1.ogg").is_ok());
        }
        assert_eq!(lib.cached(), 1);
        // The claim is about the SOURCE, not about the map: a cache that stored
        // the value but re-decoded anyway would still report one entry.
        assert_eq!(
            lib.source.opens.len(),
            1,
            "computeIfAbsent decodes once for the life of the session"
        );
    }

    #[test]
    fn a_stream_is_never_cached() {
        let mut lib = SoundBufferLibrary::new(Counting::default());
        let a = lib.stream("minecraft/sounds/music/calm1.ogg", true);
        let b = lib.stream("minecraft/sounds/music/calm1.ogg", true);
        assert_eq!(a, b, "same request, same handle");
        assert_eq!(lib.cached(), 0, "and nothing landed in the buffer cache");
        // …and a static of the same path is a separate, cached thing.
        let _ = lib.complete_buffer("minecraft/sounds/music/calm1.ogg");
        assert_eq!(lib.cached(), 1);
    }

    #[test]
    fn the_loop_flag_rides_with_the_stream() {
        // The flag selects `LoopingAudioStream` at the decoder, because
        // `SoundEngine.play` tells a streamed source NOT to loop. A model that
        // only carried it as an AL property plays every music track once.
        let lib = SoundBufferLibrary::new(Counting::default());
        assert!(lib.stream("k", true).looping);
        assert!(!lib.stream("k", false).looping);
    }

    #[test]
    fn a_failure_is_cached_too() {
        // `computeIfAbsent` stores the future, and an exceptionally-completed
        // future is still in the map. Retrying every frame would be the obvious
        // design and is not vanilla's.
        let mut lib = SoundBufferLibrary::new(Counting {
            fail: true,
            ..Default::default()
        });
        assert!(lib.complete_buffer("missing.ogg").is_err());
        assert!(lib.complete_buffer("missing.ogg").is_err());
        assert_eq!(lib.source.opens.len(), 1, "not retried");
    }

    /// `Channel.calculateBufferSize` — one second, in bytes, at 16 bits.
    ///
    /// Literal values rather than the formula recomputed, because a witness that
    /// re-derives its expectation agrees with any formula (§0.0 gotcha 0a). The
    /// figure that matters is **176400**: one second of 44.1 kHz stereo, which
    /// is what four queued buffers of it being ~705 KB rests on.
    #[test]
    fn a_stream_buffer_is_one_second_of_sixteen_bit_audio() {
        assert_eq!(calculate_buffer_size(2, 44_100, 1), 176_400);
        assert_eq!(calculate_buffer_size(1, 44_100, 1), 88_200);
        assert_eq!(calculate_buffer_size(2, 48_000, 1), 192_000);
        // The 16 is `JOrbisAudioStream`'s `new AudioFormat(rate, 16, ...)`, so
        // the byte count is twice the sample count — the `/ 2` a caller needs to
        // reach samples is the only conversion, and it is exact.
        assert_eq!(calculate_buffer_size(2, 44_100, 1) / 2, 2 * 44_100);
        // Degenerate inputs are zero rather than negative or huge: a format that
        // reported no channels would otherwise size a buffer by wrapping.
        assert_eq!(calculate_buffer_size(0, 44_100, 1), 0);
        assert_eq!(calculate_buffer_size(2, 44_100, 0), 0);
        assert_eq!(calculate_buffer_size(2, 44_100, -1), 0);
        // The two constants are vanilla's, and four one-second buffers is the
        // whole streaming invariant.
        assert_eq!(QUEUED_BUFFER_COUNT, 4);
        assert_eq!(BUFFER_DURATION_SECONDS, 1);
    }

    #[test]
    fn a_source_that_cannot_stream_says_so_with_the_key() {
        // The default method. A fake built for the caching tests has no decoder
        // in it, and must refuse a stream rather than pretend to open one.
        let mut lib = SoundBufferLibrary::new(Counting::default());
        // Matched rather than `unwrap_err`: the `Ok` side is a trait object with
        // no `Debug`, which is the shape of the seam rather than an obstacle.
        match lib.open_stream("minecraft/sounds/music/calm1.ogg", true) {
            Ok(_) => panic!("a decoderless source opened a stream"),
            Err(e) => assert!(e.contains("calm1.ogg"), "got {e}"),
        }
        assert_eq!(lib.cached(), 0, "and a stream never touches the buffer cache");
    }

    #[test]
    fn clear_is_the_only_thing_that_evicts() {
        let mut lib = SoundBufferLibrary::new(Counting::default());
        lib.preload(&["a".into(), "b".into(), "a".into()]);
        assert_eq!(lib.cached(), 2, "preload de-duplicates through the cache");
        assert_eq!(lib.source.opens.len(), 2);
        lib.clear();
        assert_eq!(lib.cached(), 0);
        let _ = lib.complete_buffer("a");
        assert_eq!(lib.source.opens.len(), 3, "and a reload really re-decodes");
    }
}
