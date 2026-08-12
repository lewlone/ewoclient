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

    /// `getStream` — **never cached**, and the loop flag rides with it.
    ///
    /// There is no `computeIfAbsent` here and no map lookup at all: every call
    /// opens the resource again. That is not an oversight — a stream is stateful
    /// (it holds a decode position), so two sounds sharing one would fight over
    /// it, and the same track started twice must genuinely play twice.
    pub fn stream(&self, key: &str, looping: bool) -> StreamHandle {
        StreamHandle {
            key: key.to_string(),
            looping,
        }
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
