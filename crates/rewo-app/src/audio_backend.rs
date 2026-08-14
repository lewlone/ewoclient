//! Opening an audio device — the only file in `rewo-app` with a `#[cfg]` in it.
//!
//! **The whole feature gate is contained here on purpose.** `rewo` is one
//! binary and all 34 gates are subcommands of it, so a dependency of this crate
//! is linked into every one of them; `rewo-audio` is therefore optional and a
//! default build pulls in neither cpal nor symphonia. The price of that is a
//! real hazard — code behind a `#[cfg]` that is off by default is **not
//! type-checked** by the build everyone runs, and this project has been bitten
//! by exactly that shape before (M86's `self.baked.take()`, where nine shipped
//! features had never once rendered because the path was unreachable and no
//! gate could see it).
//!
//! Two things keep that hazard small. The `#[cfg]` appears **only** in this
//! file, so every call site and everything downstream of it compiles in both
//! configurations. And the two arms have the **same signature**, so a caller
//! cannot tell them apart and there is no second code path to keep in step —
//! the disabled arm returns an `Err` saying how to get the enabled one.
//!
//! ## What a successful `open` does and does not mean
//!
//! It means a device accepted a stereo stream and the backend is attached.
//! It does **not** mean anything is audible: absent, muted, exclusive-mode,
//! routed elsewhere and simply not plugged into speakers all look identical
//! from inside this process, and no check in this project can tell them apart.
//! `REWO_AUDIO_PLAN.md` §4 sets that out at length. The counters on
//! [`rewo_net::sound_engine::SinkDiagnostics`] are what a human reads when
//! there is no sound and no error.

use rewo_net::sound_engine::ChannelSink;

/// Open the default output device and build the engine's backend.
///
/// `version` selects the asset index — the store holds several at once, and
/// "the newest file" silently reads another version's sounds.
///
/// `Err` is not fatal to the client: a session with no audio is the session
/// every gate runs. The caller logs it and carries on.
#[cfg(feature = "audio")]
pub fn open(version: &str) -> Result<Box<dyn ChannelSink>, String> {
    use rewo_audio::buffers::PcmSource;
    use rewo_audio::cpal_sink::CpalBackend;
    use rewo_audio::decode::BytesSource;
    use rewo_data::asset_index::AssetIndex;

    // Resolved once, up front. A per-sound re-read of a multi-megabyte index
    // would put a file parse on the tick that plays a footstep — and failing
    // here, before a device is opened, is the difference between "no assets"
    // and "no sound" as a diagnosis.
    let (root, index) = AssetIndex::load_for_version(version)?;
    log::info!(
        "audio: asset index for {version} has {} object(s)",
        index.len()
    );

    // The store lookup lives on this side of `PcmSource` because it needs
    // `rewo-data`, which `rewo-audio` deliberately does not depend on.
    // TWO sources (M156). One stays here for streams and the synchronous
    // fallback; the other is moved onto the decode worker, which owns it. They
    // are independent readers of the same immutable store, so nothing is
    // shared and nothing needs a lock.
    let (root2, index2) = (root.clone(), index.clone());
    let source: Box<dyn PcmSource> = Box::new(BytesSource(move |key: &str| index.read(&root, key)));
    let worker_source: Box<dyn PcmSource + Send> =
        Box::new(BytesSource(move |key: &str| index2.read(&root2, key)));

    let backend = CpalBackend::open_with_worker(source, Some(worker_source))?;
    log::info!(
        "audio: device open at {} Hz. This says a stream was accepted; it does \
         not say anything is audible.",
        backend.out_rate()
    );
    Ok(Box::new(backend))
}

/// The default build has no audio stack linked at all.
///
/// Deliberately an `Err` with the command in it rather than a silent no-op:
/// someone who passed `--audio` and heard nothing needs to know that this
/// binary could never have made a sound, which is a different problem from a
/// device that will not open.
#[cfg(not(feature = "audio"))]
pub fn open(_version: &str) -> Result<Box<dyn ChannelSink>, String> {
    Err("this build has no audio stack linked (cargo build -p rewo-app --features audio)".into())
}

#[cfg(test)]
mod tests {
    /// **The default build must be unable to open a device, and must say so.**
    ///
    /// This is the build the 34 gates run, and the claim it pins is the one the
    /// feature exists for: a default `rewo` links no audio stack, so `--audio`
    /// cannot quietly half-work. Asserting the *message* rather than just the
    /// `Err` is what makes it useful — someone who passed `--audio` and heard
    /// nothing needs to be told the binary could never have made a sound, which
    /// is a different problem from a device that will not open.
    ///
    /// **There is deliberately no test for the enabled arm.** It opens a real
    /// device, and no test in this project does that: `cargo test` stays silent
    /// by construction, which is the invariant `REWO_AUDIO_PLAN.md` §4 rests on.
    #[test]
    #[cfg(not(feature = "audio"))]
    fn a_default_build_refuses_and_says_how_to_get_one() {
        // Matched rather than `unwrap_err`: the `Ok` type is a trait object and
        // has no `Debug`, which is itself the shape of the thing being pinned.
        match super::open("26.2") {
            Ok(_) => panic!("a build with no audio stack opened a device"),
            Err(e) => assert!(e.contains("--features audio"), "unhelpful refusal: {e}"),
        }
    }
}
