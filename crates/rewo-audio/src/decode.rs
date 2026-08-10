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
}
