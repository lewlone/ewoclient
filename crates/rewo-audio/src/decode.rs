//! Ogg Vorbis to the 16-bit PCM a static buffer holds.
//!
//! This is `JOrbisAudioStream.readAll()`'s job, and it is the one part of the
//! audio path Rewo **cannot** grade bit-for-bit against vanilla. Vorbis I does
//! not mandate identical float output between implementations — the
//! specification defines the bitstream, not the exact floating-point result of
//! decoding it — so jorbis and symphonia are both correct and may disagree in
//! the last bits. Anyone expecting a byte-identical match against Minecraft's
//! own decode is expecting something the format does not promise.
//!
//! **What IS exact is what happens after**: [`crate::quantise::quantise`] is
//! transcribed to the bit and pinned with literal vectors, so the lossy step
//! that gives Minecraft audio its character is reproduced even though the float
//! samples feeding it are only equal to within the format's tolerance. That
//! split — an exact tail on an approximate head — is the honest description, and
//! it is why the quantisation lives in its own module with its own tests rather
//! than inline here where it would look like an implementation detail.
//!
//! **The end trim is symphonia's, not ours.** A Vorbis stream's final page
//! carries a granule position that can be *less* than the samples the last
//! packet decodes to, and the surplus must be discarded — otherwise every sound
//! ends with a few milliseconds of whatever the encoder's lapping produced.
//! Symphonia applies it; reimplementing it here would be a second, worse copy.
//! It is named rather than left implicit because a decoder that ignored it
//! sounds *almost* right, which is the hardest kind of wrong to notice.

use crate::buffers::Pcm;
use crate::quantise::quantise;

/// Decode a complete Ogg Vorbis file to interleaved 16-bit PCM.
///
/// The samples come out already through [`quantise`], because that is where
/// vanilla's precision loss happens and a caller holding f32 would be holding
/// something Minecraft never has.
///
/// **Mixed sample rates are normal, not an edge case.** The 26.2 store carries
/// both 44100 and 48000 inside a single event family, so the rate is returned
/// per file and a mixer that assumed one rate would be resampling most sounds
/// wrongly rather than a few.
///
/// **Channel count is per VARIANT, not per event.** `item/goat_horn/call3.ogg`
/// is stereo while its siblings are mono, so one event resolves to a 2-channel
/// buffer on one roll of the dice and a 1-channel buffer on the next. OpenAL
/// does not spatialise a multi-channel buffer at all, so vanilla plays those
/// non-positionally; Rewo's own handling of that is a decision for the mixer
/// and is recorded in `REWO_AUDIO_PLAN.md` rather than decided here.
pub fn decode_ogg_vorbis(bytes: &[u8]) -> Result<Pcm, String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let cursor = std::io::Cursor::new(bytes.to_vec());
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());
    let mut hint = Hint::new();
    // A hint, not a requirement: the probe reads the OggS magic either way. It
    // is supplied because every path Rewo decodes really is an `.ogg`, and a
    // wrong hint costs nothing while a right one skips a format guess.
    hint.with_extension("ogg");

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("not an ogg stream: {e}"))?;
    let mut format = probed.format;

    // `default_track` rather than "the first track": an Ogg container may
    // multiplex, and the first stream is not necessarily the audio one.
    let track = format
        .default_track()
        .ok_or_else(|| "ogg stream has no default track".to_string())?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("no vorbis decoder for this track: {e}"))?;

    let mut samples: Vec<i16> = Vec::new();
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut buf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            // End of stream. Symphonia reports it as an `UnexpectedEof` io
            // error rather than a distinct variant, so this is the normal exit
            // and not an error path — treating it as one would fail every
            // successful decode at the last packet.
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => {
                return Err("ogg stream requires a decoder reset (chained stream)".into())
            }
            Err(e) => return Err(format!("ogg read: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(audio) => {
                let spec = *audio.spec();
                if channels == 0 {
                    channels = spec.channels.count() as u16;
                    sample_rate = spec.rate;
                }
                let b = buf.get_or_insert_with(|| {
                    SampleBuffer::<f32>::new(audio.capacity() as u64, spec)
                });
                // Interleaved, which is what OpenAL wants and what the mixer
                // will read: L R L R for stereo rather than two planes.
                b.copy_interleaved_ref(audio);
                samples.extend(b.samples().iter().copied().map(quantise));
            }
            // A corrupt packet is skippable in Vorbis — vanilla's decoder does
            // the same rather than abandoning the file, and a sound with one
            // bad page is better than no sound.
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(format!("vorbis decode: {e}")),
        }
    }

    // **Defensive, and unreachable with symphonia's probe** — recorded because a
    // mutation replacing it with `Ok(empty)` survives, and the reason matters.
    // Every truncation of a real `.ogg` (tried at nine lengths from 58 bytes up)
    // fails in `get_probe().format(...)` with "end of stream" long before a
    // track exists to decode nothing, so no fixture can reach here. The guard
    // stays anyway: it costs one comparison, and a probe that one day accepts a
    // container more leniently would otherwise hand the engine a zero-channel
    // buffer at zero hertz and call it a successful decode.
    if channels == 0 {
        return Err("ogg stream decoded no audio".into());
    }
    Ok(Pcm {
        samples,
        channels,
        sample_rate,
    })
}

/// One live Vorbis decode position — the state `JOrbisAudioStream` holds.
///
/// Separate from [`OggStream`] because `LoopingAudioStream` **throws its inner
/// stream away and builds a new one** at the loop point rather than seeking
/// (`LoopingAudioStream.java:31-33`), so "the decoder" has to be a thing that
/// can be replaced without disturbing the bytes it reads or the format it
/// reports.
struct VorbisReader {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    buf: Option<symphonia::core::audio::SampleBuffer<f32>>,
}

impl VorbisReader {
    /// Open the container and the codec, and report the format the *header*
    /// declares.
    ///
    /// **The format has to be known before any audio is decoded**, because
    /// `Channel.attachBufferStream` sizes its buffers from `stream.getFormat()`
    /// before it pumps anything (`Channel.java:125-127`). Vorbis carries both in
    /// its identification header, so this reads them off `codec_params` rather
    /// than inferring them from a decoded packet — a stream that could only
    /// answer after decoding could not be attached at all.
    fn open(bytes: std::sync::Arc<[u8]>) -> Result<(VorbisReader, u16, u32), String> {
        use symphonia::core::codecs::DecoderOptions;
        use symphonia::core::formats::FormatOptions;
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;
        use symphonia::core::probe::Hint;

        let cursor = std::io::Cursor::new(bytes);
        let mss = MediaSourceStream::new(Box::new(cursor), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("ogg");
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|e| format!("not an ogg stream: {e}"))?;
        let format = probed.format;
        let track = format
            .default_track()
            .ok_or_else(|| "ogg stream has no default track".to_string())?;
        let track_id = track.id;
        let channels = track
            .codec_params
            .channels
            .map(|c| c.count() as u16)
            .ok_or_else(|| "ogg stream declares no channel layout".to_string())?;
        let sample_rate = track
            .codec_params
            .sample_rate
            .ok_or_else(|| "ogg stream declares no sample rate".to_string())?;
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| format!("no vorbis decoder for this track: {e}"))?;
        Ok((
            VorbisReader {
                format,
                decoder,
                track_id,
                buf: None,
            },
            channels,
            sample_rate,
        ))
    }

    /// One packet's samples, or `None` at end of stream.
    ///
    /// `Some(empty)` is a real and ordinary answer — Vorbis' first audio packets
    /// decode to nothing — and is **not** the same as `None`. A caller that
    /// treated them alike would report every stream exhausted before it started.
    fn next(&mut self) -> Result<Option<Vec<i16>>, String> {
        use symphonia::core::audio::SampleBuffer;
        use symphonia::core::errors::Error;
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(None)
                }
                Err(Error::ResetRequired) => {
                    return Err("ogg stream requires a decoder reset (chained stream)".into())
                }
                Err(e) => return Err(format!("ogg read: {e}")),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(audio) => {
                    let spec = *audio.spec();
                    let b = self
                        .buf
                        .get_or_insert_with(|| SampleBuffer::<f32>::new(audio.capacity() as u64, spec));
                    b.copy_interleaved_ref(audio);
                    return Ok(Some(b.samples().iter().copied().map(quantise).collect()));
                }
                Err(Error::DecodeError(_)) => continue,
                Err(e) => return Err(format!("vorbis decode: {e}")),
            }
        }
    }
}

/// `JOrbisAudioStream`, optionally wrapped in `LoopingAudioStream` (M144).
///
/// **The whole compressed file is held in memory, and that is vanilla's shape
/// too** for the looping case: `LoopingAudioStream` marks its
/// `BufferedInputStream` with `Integer.MAX_VALUE` and `reset()`s it at the loop
/// point (`:18`, `:32`), which buffers the entire resource. What is *not* held
/// is the decoded PCM — the largest streamed track is 142 MB of samples against
/// 11.3 MB of ogg, and that ratio is the reason this type exists.
pub struct OggStream {
    /// Kept for the restart. This is `bufferedInputStream`'s mark.
    bytes: std::sync::Arc<[u8]>,
    reader: VorbisReader,
    looping: bool,
    channels: u16,
    sample_rate: u32,
    /// Decoded but not yet handed out. A packet does not divide evenly into a
    /// requested size, so the surplus has to live somewhere.
    pending: Vec<i16>,
}

impl OggStream {
    pub fn open(bytes: std::sync::Arc<[u8]>, looping: bool) -> Result<OggStream, String> {
        let (reader, channels, sample_rate) = VorbisReader::open(std::sync::Arc::clone(&bytes))?;
        Ok(OggStream {
            bytes,
            reader,
            looping,
            channels,
            sample_rate,
            pending: Vec::new(),
        })
    }

    /// `bufferedInputStream.reset()` + `provider.create(...)` — a fresh decoder
    /// over the same bytes.
    fn restart(&mut self) -> Result<(), String> {
        let (reader, channels, sample_rate) =
            VorbisReader::open(std::sync::Arc::clone(&self.bytes))?;
        // The same bytes cannot describe a different format; asserting it would
        // be asserting that `VorbisReader::open` is deterministic. What is worth
        // keeping is that a restart does not silently change what the caller
        // already sized its buffers against.
        debug_assert_eq!((channels, sample_rate), (self.channels, self.sample_rate));
        self.reader = reader;
        self.pending.clear();
        Ok(())
    }

    /// `JOrbisAudioStream.read` — up to `samples`, empty once exhausted.
    fn inner_read(&mut self, samples: usize) -> Result<Vec<i16>, String> {
        while self.pending.len() < samples {
            match self.reader.next()? {
                Some(s) => self.pending.extend(s),
                None => break,
            }
        }
        let take = samples.min(self.pending.len());
        Ok(self.pending.drain(..take).collect())
    }
}

impl crate::buffers::PcmStream for OggStream {
    fn format(&self) -> (u16, u32) {
        (self.channels, self.sample_rate)
    }

    /// `LoopingAudioStream.read`, transcribed including its shape.
    ///
    /// ```java
    /// ByteBuffer result = this.stream.read(expectedSize);
    /// if (!result.hasRemaining()) {
    ///    this.stream.close();
    ///    this.bufferedInputStream.reset();
    ///    this.stream = this.provider.create(new NoCloseBuffer(this.bufferedInputStream));
    ///    result = this.stream.read(expectedSize);
    /// }
    /// return result;
    /// ```
    ///
    /// **The restart happens on the read AFTER the one that ran out**, not by
    /// splicing across the boundary — the guard is on the inner read coming back
    /// *empty*, and a short non-empty read is the ordinary end of a file. So a
    /// looping stream hands out one **short** buffer at the loop point and a full
    /// one after it, while the samples themselves are continuous. Splicing to
    /// keep every buffer the same size would sound identical and would not be
    /// this, and the difference is observable in the buffer lengths.
    fn read(&mut self, samples: usize) -> Result<Vec<i16>, String> {
        // A zero-sized read is empty by definition, and must not be mistaken for
        // exhaustion — otherwise it would restart a looping stream from the top.
        if samples == 0 {
            return Ok(Vec::new());
        }
        let out = self.inner_read(samples)?;
        if out.is_empty() && self.looping {
            self.restart()?;
            return self.inner_read(samples);
        }
        Ok(out)
    }
}

/// A [`crate::buffers::PcmSource`] over raw bytes a caller has already fetched.
///
/// The **store lookup is deliberately not here**. An asset key is
/// `<namespace>/sounds/<path>.ogg` and resolves through the asset index to a
/// content-addressed `<assets>/objects/<hash[0..2]>/<hash>`, which needs
/// `rewo-data`; taking that dependency would put the whole asset stack behind
/// this crate. The caller reads the bytes, this decodes them.
pub struct BytesSource<F>(pub F)
where
    F: FnMut(&str) -> Result<Vec<u8>, String>;

impl<F> crate::buffers::PcmSource for BytesSource<F>
where
    F: FnMut(&str) -> Result<Vec<u8>, String>,
{
    fn open(&mut self, key: &str) -> Result<Pcm, String> {
        let bytes = (self.0)(key)?;
        decode_ogg_vorbis(&bytes)
    }

    fn open_stream(
        &mut self,
        key: &str,
        looping: bool,
    ) -> Result<Box<dyn crate::buffers::PcmStream>, String> {
        let bytes = (self.0)(key)?;
        Ok(Box::new(OggStream::open(bytes.into(), looping)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffers::{PcmSource, SoundBufferLibrary};

    #[test]
    fn garbage_is_an_error_rather_than_silence() {
        // The failure mode worth excluding: a decoder that returned an empty
        // buffer for unreadable input would make every missing or corrupt sound
        // indistinguishable from a legitimately silent one, and the engine would
        // dutifully play nothing with no error anywhere.
        let e = decode_ogg_vorbis(b"this is not an ogg file at all").unwrap_err();
        assert!(!e.is_empty());
        assert!(decode_ogg_vorbis(&[]).is_err(), "and empty input too");
    }

    #[test]
    fn an_ogg_header_with_no_audio_does_not_report_success() {
        // "OggS" magic and nothing usable after it. A probe that accepted the
        // container and then found no track must not return an empty `Pcm`,
        // because zero channels at zero hertz is not a decode.
        let mut b = b"OggS".to_vec();
        b.extend_from_slice(&[0u8; 64]);
        assert!(decode_ogg_vorbis(&b).is_err());
    }

    #[test]
    fn the_source_adapter_reports_the_fetch_error_rather_than_swallowing_it() {
        let mut src = BytesSource(|k: &str| Err(format!("no such asset: {k}")));
        let e = src.open("minecraft/sounds/nope.ogg").unwrap_err();
        assert!(e.contains("nope.ogg"), "got {e}");
    }

    #[test]
    fn a_failed_decode_is_cached_by_the_library_like_any_other() {
        // Ties the two halves together: the library's caching rules were graded
        // against a fake source, and this shows the real adapter obeys them.
        let mut opens = 0;
        {
            let src = BytesSource(|_: &str| {
                opens += 1;
                Ok(b"not an ogg".to_vec())
            });
            let mut lib = SoundBufferLibrary::new(src);
            assert!(lib.complete_buffer("a.ogg").is_err());
            assert!(lib.complete_buffer("a.ogg").is_err());
        }
        assert_eq!(opens, 1, "computeIfAbsent caches the failure too");
    }
}


#[cfg(test)]
mod real_assets {
    //! Decoding real Ogg Vorbis from the 26.2 asset store.
    //!
    //! **The vectors are checked in; the audio is not.** The store is Mojang's
    //! and belongs in the user's own install, exactly as the decompile and the
    //! datagen reports do — this repo has never carried game assets and does not
    //! start here. What is recorded is what was *measured* from three of them,
    //! which is the same shape as `tools/java_tostring_oracle` and the loopback
    //! oracle M139 plans: run the artefact once, commit the numbers.
    //!
    //! **A missing store SKIPS, and says so out loud.** That is a real weakness
    //! and it is named rather than hidden: on a machine with no unpacked assets
    //! these witnesses prove nothing, and a green run there is not evidence.
    //! The `soundshot` gate M138b's plan describes is where this becomes
    //! fail-closed, for the same reason `build_sounds` now panics under
    //! `--render-check` instead of degrading to an empty index.

    fn asset(sub: &str, hash: &str) -> Option<Vec<u8>> {
        let base = std::env::var("APPDATA").ok()?;
        let p = std::path::Path::new(&base)
            .join("EwoClient/shared/assets/objects")
            .join(sub)
            .join(hash);
        std::fs::read(p).ok()
    }

    /// `mob/chicken/step1.ogg` — mono, 44.1 kHz, 1728 samples.
    const CHICKEN: (&str, &str) = ("e1", "e16352150262ab49686f6c0aeaffa7532d3157ea");
    /// `item/goat_horn/call0.ogg` — mono, 48 kHz.
    const HORN_MONO: (&str, &str) = ("ce", "ce8a2675cc2c9ac986851d2c5139d5c9ad3eeee1");
    /// `item/goat_horn/call3.ogg` — **stereo**, 48 kHz. The odd one out.
    const HORN_STEREO: (&str, &str) = ("16", "16c3be71d3e789ee539cd70819e526343ace5e84");

    #[test]
    fn a_real_ogg_decodes_to_its_exact_sample_count() {
        let Some(bytes) = asset(CHICKEN.0, CHICKEN.1) else {
            println!("SKIPPED: no unpacked asset store -- this witness proved nothing");
            return;
        };
        let pcm = super::decode_ogg_vorbis(&bytes).expect("chicken step1 must decode");
        assert_eq!(pcm.channels, 1);
        assert_eq!(pcm.sample_rate, 44100);
        // **Exact, and it is the end trim that makes it so.** A Vorbis stream's
        // final page carries a granule position below what the last packet
        // decodes to, and the surplus is discarded; a decoder that ignored it
        // returns MORE samples here and sounds almost right.
        assert_eq!(pcm.samples.len(), 1728);

        // Content, loosely: exact sample values would pin symphonia's version
        // rather than anything about Rewo, since Vorbis I does not mandate
        // identical float output. These bounds still catch the failures that
        // matter — a quantisation that lost its multiplier decodes to near
        // silence, and one with a doubled scale clips flat.
        let peak = pcm.samples.iter().map(|s| (*s as i32).abs()).max().unwrap();
        assert!(peak > 8000, "decoded near-silence: peak {peak}");
        assert!(peak < 32767, "decoded clipped: peak {peak}");

        // **The SUM is what sees the quantisation, and the peak is not.**
        // Measured: -136 through `quantise`, and +676 with the `-0.5` bias
        // dropped for a naive `(s * 32767.0) as i16` — a gap of 812 across 1728
        // samples, which is the half-step showing up once per sample. The peak
        // moves by ONE over the same change (19988 against 19987), so a
        // peak-only witness cannot see it; a mutation battery is how that was
        // established rather than assumed.
        //
        // The tolerance is wide on purpose. An aggregate over 1728 samples
        // shifts by a few when a symphonia bump changes a few samples by an LSB,
        // and by hundreds when the bias goes missing, so +/-200 is robust
        // against the first and catches the second with room to spare. Pinning
        // the exact value would pin symphonia's version instead of Rewo's
        // arithmetic, which Vorbis I explicitly does not promise.
        let sum: i64 = pcm.samples.iter().map(|s| *s as i64).sum();
        assert!(
            (sum - -136).abs() < 200,
            "sum {sum}: expected about -136 (a naive scale without the -0.5 bias gives +676)"
        );
    }

    #[test]
    fn channels_are_per_variant_and_rates_are_mixed() {
        // Both facts are load-bearing for the mixer and both are easy to assume
        // away. `call3` is the ONLY stereo goat-horn variant, so one event
        // resolves to a 2-channel buffer on one roll and a 1-channel buffer on
        // the next; OpenAL does not spatialise multi-channel buffers at all, so
        // vanilla plays that one non-positionally. And the store is not one
        // rate: the chicken is 44100 while the horn is 48000, which puts the
        // resampler on the hot path for essentially every sound rather than on
        // an edge case.
        let (Some(mono), Some(stereo), Some(chicken)) = (
            asset(HORN_MONO.0, HORN_MONO.1),
            asset(HORN_STEREO.0, HORN_STEREO.1),
            asset(CHICKEN.0, CHICKEN.1),
        ) else {
            println!("SKIPPED: no unpacked asset store -- this witness proved nothing");
            return;
        };
        let mono = super::decode_ogg_vorbis(&mono).unwrap();
        let stereo = super::decode_ogg_vorbis(&stereo).unwrap();
        let chicken = super::decode_ogg_vorbis(&chicken).unwrap();

        assert_eq!(mono.channels, 1, "call0");
        assert_eq!(stereo.channels, 2, "call3 -- the one stereo variant");
        assert_eq!(mono.sample_rate, 48000);
        assert_eq!(stereo.sample_rate, 48000);
        assert_ne!(
            chicken.sample_rate, mono.sample_rate,
            "44100 against 48000: the store is mixed-rate"
        );
        // Interleaved, so a stereo buffer holds an even number of samples and
        // its frame count is half its length. A planar decode would still have
        // the right total and the wrong order, which this cannot see — the
        // mixer's own witnesses are where that lands.
        assert_eq!(stereo.samples.len() % 2, 0);
        assert_eq!(stereo.samples.len(), 432000, "4.5 s at 48 kHz, two channels");
    }
    // ── the incremental stream (M144) ─────────────────────────────────────

    use crate::buffers::PcmStream;

    fn stream(a: (&str, &str), looping: bool) -> Option<super::OggStream> {
        let bytes = asset(a.0, a.1)?;
        Some(super::OggStream::open(bytes.into(), looping).expect("must open"))
    }

    /// **The format is known before a single sample is decoded.**
    ///
    /// `Channel.attachBufferStream` reads `stream.getFormat()` to size its
    /// buffers and only then pumps (`Channel.java:125-127`), so a stream that
    /// learned its format by decoding could not be attached at all. This comes
    /// off the Vorbis identification header via `codec_params`.
    #[test]
    fn a_stream_reports_its_format_before_any_read() {
        let Some(s) = stream(CHICKEN, false) else {
            println!("SKIPPED: no unpacked asset store -- this witness proved nothing");
            return;
        };
        assert_eq!(s.format(), (1, 44100));
        let Some(h) = stream(HORN_STEREO, false) else { return };
        assert_eq!(h.format(), (2, 48000), "and it is per-variant, not per-event");
    }

    /// **Reading a file in chunks gives exactly what reading it whole gives.**
    ///
    /// The claim that makes streaming safe to introduce at all: the two decode
    /// paths must be the same audio, sample for sample, or a streamed sound is a
    /// subtly different sound. Chunked at a size that does not divide the file,
    /// so packet boundaries and read boundaries disagree throughout — which is
    /// the case that catches a `pending` buffer that drops or duplicates a
    /// remainder, and the reason not to pick a round number here.
    #[test]
    fn reading_a_stream_in_chunks_yields_exactly_the_whole_file() {
        let Some(bytes) = asset(CHICKEN.0, CHICKEN.1) else {
            println!("SKIPPED: no unpacked asset store -- this witness proved nothing");
            return;
        };
        let whole = super::decode_ogg_vorbis(&bytes).expect("decode");
        let mut s = super::OggStream::open(bytes.into(), false).expect("open");

        let mut chunked: Vec<i16> = Vec::new();
        let mut reads = 0;
        loop {
            let part = s.read(577).expect("read");
            if part.is_empty() {
                break;
            }
            assert!(part.len() <= 577, "a read must not overshoot what was asked");
            chunked.extend(part);
            reads += 1;
            assert!(reads < 1000, "runaway: the stream never reported exhaustion");
        }
        assert!(reads > 1, "the fixture must actually take several reads");
        assert_eq!(chunked.len(), whole.samples.len());
        assert_eq!(chunked, whole.samples, "chunked and whole must be the same audio");
    }

    /// A non-looping stream stays exhausted rather than restarting by itself.
    #[test]
    fn a_non_looping_stream_stays_exhausted() {
        let Some(mut s) = stream(CHICKEN, false) else {
            println!("SKIPPED: no unpacked asset store -- this witness proved nothing");
            return;
        };
        let mut total = 0;
        while !s.read(1000).unwrap().is_empty() {
            total += 1;
            assert!(total < 1000, "runaway");
        }
        // Repeatedly, not once: the exhausted state has to be stable, because
        // the producer polls it every tick for the rest of the sound's life.
        for _ in 0..5 {
            assert!(s.read(1000).unwrap().is_empty());
        }
    }

    /// **The loop restarts on the read AFTER the one that ran out**, which is
    /// visible in the buffer LENGTHS and not in the samples.
    ///
    /// `LoopingAudioStream.read` guards on the inner read coming back *empty*, so
    /// the last partial buffer of a pass is handed out as-is and the restart
    /// happens on the next call. The observable signature is therefore a short
    /// buffer at the loop point followed by a full one — the audio is continuous
    /// either way, so a version that spliced across the boundary to keep every
    /// buffer full would sound identical and would not be this.
    ///
    /// The chicken is 1728 samples, so at 1000 the pattern is 1000, 728, 1000,
    /// 728 … and the numbers are the fixture's rather than round by luck.
    #[test]
    fn a_looping_stream_restarts_with_a_short_buffer_at_the_boundary() {
        let Some(mut s) = stream(CHICKEN, true) else {
            println!("SKIPPED: no unpacked asset store -- this witness proved nothing");
            return;
        };
        let lens: Vec<usize> = (0..5).map(|_| s.read(1000).unwrap().len()).collect();
        assert_eq!(lens, vec![1000, 728, 1000, 728, 1000], "got {lens:?}");

        // And the audio really is continuous: the samples after the restart are
        // the start of the file again, not a gap and not a repeat of the tail.
        let Some(bytes) = asset(CHICKEN.0, CHICKEN.1) else { return };
        let whole = super::decode_ogg_vorbis(&bytes).unwrap();
        let mut fresh = super::OggStream::open(bytes.into(), true).unwrap();
        let mut seen: Vec<i16> = Vec::new();
        for _ in 0..4 {
            seen.extend(fresh.read(1000).unwrap());
        }
        assert_eq!(seen.len(), 1728 * 2, "two full passes");
        assert_eq!(&seen[..1728], &whole.samples[..]);
        assert_eq!(&seen[1728..], &whole.samples[..], "the second pass is the first");
    }

    /// **A zero-sized read must not restart a looping stream.**
    ///
    /// It is empty by definition, and the restart guard is "the inner read came
    /// back empty" — so without the explicit guard a `read(0)` would rewind a
    /// playing track to its beginning. Reachable in production if a format ever
    /// reported zero channels, which `calculate_buffer_size` would turn into a
    /// zero-sized buffer.
    #[test]
    fn a_zero_sized_read_does_not_restart_a_looping_stream() {
        let Some(mut s) = stream(CHICKEN, true) else {
            println!("SKIPPED: no unpacked asset store -- this witness proved nothing");
            return;
        };
        let first = s.read(600).unwrap();
        assert_eq!(first.len(), 600);
        assert!(s.read(0).unwrap().is_empty());
        // Still where it was: the next 600 are the file's SECOND 600, not its
        // first. A restart here would make them the first.
        let second = s.read(600).unwrap();
        assert_eq!(second.len(), 600);
        assert_ne!(first, second, "a rewind would make these equal");
    }

    /// **The adapter really opens a stream**, not just a static buffer.
    ///
    /// `BytesSource::open_stream` is one line, and it is the join between the
    /// asset lookup the app owns and the decoder this crate owns — so it is
    /// exactly the kind of line that is never exercised. Everything on either
    /// side of it is tested; nothing was testing it. Driven through
    /// `SoundBufferLibrary` rather than the adapter directly, because that is
    /// the path production takes and it is where a library that forgot to
    /// delegate would show up.
    #[test]
    fn the_bytes_adapter_opens_a_real_stream_through_the_library() {
        use crate::buffers::SoundBufferLibrary;
        let Some(bytes) = asset(CHICKEN.0, CHICKEN.1) else {
            println!("SKIPPED: no unpacked asset store -- this witness proved nothing");
            return;
        };
        let whole = super::decode_ogg_vorbis(&bytes).expect("decode");
        let mut lib = SoundBufferLibrary::new(super::BytesSource(move |_: &str| Ok(bytes.clone())));

        let mut s = lib
            .open_stream("minecraft/sounds/mob/chicken/step1.ogg", false)
            .expect("the adapter must open a stream");
        assert_eq!(s.format(), (1, 44_100));
        assert_eq!(s.read(1_000).unwrap().len(), 1_000);
        assert_eq!(lib.cached(), 0, "and a stream never enters the buffer cache");

        // A looping one from the same library is a separate position, and loops.
        let mut l = lib
            .open_stream("minecraft/sounds/mob/chicken/step1.ogg", true)
            .expect("looping");
        let mut all: Vec<i16> = Vec::new();
        for _ in 0..3 {
            all.extend(l.read(1_000).unwrap());
        }
        assert_eq!(all.len(), 2_728, "1000 + 728 + 1000 across the loop point");
        assert_eq!(&all[..1_728], &whole.samples[..], "the first pass is the file");
    }

    /// Two streams of one asset are independent decode positions — `getStream`
    /// has no cache, and a shared one would make two plays fight over a cursor.
    #[test]
    fn two_streams_of_one_asset_do_not_share_a_position() {
        let Some(bytes) = asset(CHICKEN.0, CHICKEN.1) else {
            println!("SKIPPED: no unpacked asset store -- this witness proved nothing");
            return;
        };
        let b: std::sync::Arc<[u8]> = bytes.into();
        let mut a = super::OggStream::open(std::sync::Arc::clone(&b), false).unwrap();
        let mut c = super::OggStream::open(b, false).unwrap();
        let from_a = a.read(400).unwrap();
        let _ = a.read(400).unwrap();
        let from_c = c.read(400).unwrap();
        assert_eq!(from_a, from_c, "each starts at the beginning of the file");
    }

}
