//! The listening pass — the one thing in this project a machine cannot do.
//!
//! ```text
//! cargo run -p rewo-audio --example listen                  # the whole pass
//! cargo run -p rewo-audio --example listen -- --list        # what the stages are
//! cargo run -p rewo-audio --example listen -- --stage 4     # just one of them
//! cargo run -p rewo-audio --example listen -- --key minecraft/sounds/note/harp.ogg
//! ```
//!
//! **This is the only code path in Rewo that makes a noise**, and it is an
//! example rather than a `rewo` subcommand on purpose: wiring cpal into the
//! client binary would link an audio stack into all 34 gates for a subsystem
//! none of them exercises.
//!
//! # Why this is staged, and what the first cut could not reach
//!
//! The first version of this example played one clip three times through
//! [`CpalSink::play_once`], and printed a checklist asking after clicks at the
//! clip ends, silence between repeats and crackle. Two things were wrong with
//! that, and the second is the one that matters.
//!
//! The small one: its default clip was `mob/chicken/step1.ogg`, which is **0.04
//! seconds**. At forty milliseconds a clip and a click are the same duration, so
//! three of its four questions could not be answered on the sound it chose.
//!
//! The large one: `play_once` pushes exactly one configuration —
//! `SetPitch(1.0)`, `SetVolume(1.0)`, `DisableAttenuation`, `SetLooping(false)`,
//! position `(0,0,0)`, `SetRelative(true)` (`cpal_sink.rs:152-164`). That is a
//! centred, unattenuated, unpitched UI sound: **the one configuration in which
//! the pan law, the distance curve, the pitch resampler and the listener basis
//! are all inert.** `REWO_AUDIO_PLAN.md` §4 lists what only a human can grade —
//! "variant variety on repeated blocks, gain falling off with distance and
//! cutting out at the radius, the stereo image tracking while turning, no click
//! at clip ends, no glitching in a mob crowd, music not once-and-stopping" —
//! and the only tool in Rewo that can make a sound could reach two of the six.
//!
//! So the stages below drive `ring().push(Command::…)` directly, one property
//! each, and every one of them states **what a failure sounds like**. A stage
//! that only said "listen to this" would be the same gap in a longer form.
//!
//! # What this cannot tell you either
//!
//! Nothing here compares Rewo against vanilla. The pan law and the resampler are
//! Rewo's own (vanilla computes neither in Java — the complete `Channel` surface
//! is §2.3, and both live inside the OpenAL Soft DLL), so a stage that sounds
//! *good* is not evidence it sounds *the same*. Turning that into a number is
//! M139's job. What these stages can catch is the class of bug where a property
//! is wired to nothing — a pan that never moves, a distance curve that never
//! reaches zero, a listener basis that collapses when you look up — and that
//! class is invisible to every gate in this project.
//!
//! It reads the assets straight out of the store rather than through
//! `rewo-data`'s index, because this crate deliberately does not depend on it —
//! see `decode::BytesSource`.

use rewo_audio::buffers::Pcm;
use rewo_audio::cpal_sink::CpalSink;
use rewo_audio::device::Command;
use rewo_net::sound_engine::{listener_basis, ChannelCall as C, ChannelId, ListenerTransform};
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// The asset store
// ---------------------------------------------------------------------------

fn store() -> std::path::PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_default();
    std::path::Path::new(&base).join("EwoClient/shared/assets")
}

/// Resolve `<namespace>/sounds/<path>.ogg` through the asset index.
///
/// The store is content-addressed, so the key is a *lookup*, not a filename —
/// a decoder handed the string as a path finds nothing, every time.
fn read_asset(key: &str) -> Result<Vec<u8>, String> {
    let idx = store().join("indexes/32.json");
    let text = std::fs::read_to_string(&idx)
        .map_err(|e| format!("no asset index at {}: {e}", idx.display()))?;
    // A deliberate two-line parse rather than a serde dependency: this is an
    // example, and the crate's dependency list is part of what it is claiming.
    let needle = format!("\"{key}\"");
    let at = text
        .find(&needle)
        .ok_or_else(|| format!("{key} is not in the index"))?;
    let hash_at = text[at..].find("\"hash\"").ok_or("malformed index entry")? + at;
    let start = text[hash_at..].find(':').ok_or("malformed")? + hash_at + 1;
    let q1 = text[start..].find('"').ok_or("malformed")? + start + 1;
    let q2 = text[q1..].find('"').ok_or("malformed")? + q1;
    let hash = &text[q1..q2];
    let p = store().join("objects").join(&hash[..2]).join(hash);
    std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Decode one asset key, or explain why not and stop.
///
/// **A missing clip aborts the stage rather than being skipped.** A listening
/// pass that quietly played five of its six sounds would read as a pass, which
/// is the same false green `REWO_AUDIO_PLAN.md` §0.3 records for the store-
/// dependent unit tests.
fn load(key: &str) -> Result<Arc<Pcm>, String> {
    let bytes = read_asset(key)?;
    let pcm = rewo_audio::decode::decode_ogg_vorbis(&bytes).map_err(|e| format!("{key}: {e}"))?;
    Ok(Arc::new(pcm))
}

fn seconds_of(pcm: &Pcm) -> f32 {
    pcm.samples.len() as f32 / pcm.channels as f32 / pcm.sample_rate as f32
}

/// Decode every clip the pass will use, before playing any of it.
///
/// **This exists because a first draft named a clip that does not exist.** The
/// chicken has two step variants and the draft listed three, so the pass died at
/// stage 11 — correctly, but after two minutes of listening, which is the worst
/// moment to discover a typo. Loading everything up front turns "the pass is
/// broken" into a message in the first second.
///
/// It also prints the table, which is worth having on screen: the rates are what
/// say whether the stage-2 A/B is valid on *this* device, and the durations are
/// what say whether a stage can answer its own question. A clip under a quarter
/// of a second cannot witness a click at its ends.
fn preflight(out_rate: u32) -> Result<(), String> {
    let mut keys: Vec<&str> = vec![clip::LEVELUP, clip::HARP, clip::HORN, clip::CAVE];
    keys.extend_from_slice(&clip::VARIANTS);
    println!("preflight — decoding {} clips:", keys.len());
    for k in keys {
        let pcm = load(k)?;
        let d = seconds_of(&pcm);
        println!(
            "  {:>6.2} s  {} ch  {:>5} Hz{}  {}",
            d,
            pcm.channels,
            pcm.sample_rate,
            if pcm.sample_rate == out_rate {
                " ="
            } else {
                " ~"
            },
            k
        );
    }
    println!("  (= native to the device, ~ resampled)");
    println!();
    Ok(())
}

// ---------------------------------------------------------------------------
// Driving the ring
// ---------------------------------------------------------------------------

/// Every clip these stages use, with why each was chosen.
///
/// The rates are not incidental. The device here opens at whatever WASAPI
/// offers — 48000 on the machine this was written on — and most of the store is
/// 44100, so the resampler is on the hot path for nearly every sound rather
/// than being an edge case (`REWO_AUDIO_PLAN.md` §5 says so; it is measurable
/// with `--key`, which prints the rate).
mod clip {
    /// 1.75 s, 1 ch, 44100 Hz. Long enough to hear ends and a tail; familiar
    /// enough that a wrong pitch or speed is obvious without a reference.
    pub const LEVELUP: &str = "minecraft/sounds/random/levelup.ogg";
    /// 0.58 s, 1 ch, 44100 Hz. A near-pure pitched tone, which is the signal
    /// resampler artefacts are easiest to hear on — noise hides them.
    pub const HARP: &str = "minecraft/sounds/note/harp.ogg";
    /// 4.50 s, **2 ch, 48000 Hz**. The one clip that is both natively stereo
    /// and natively at the usual device rate, so it is the A-side for the
    /// resampler and the only exercise of the multi-channel path.
    pub const HORN: &str = "minecraft/sounds/item/goat_horn/call3.ogg";
    /// The four `dig/stone` variants, ~0.4 s each.
    ///
    /// Chosen over `mob/chicken/step*` for two reasons, one of which was a bug.
    /// `REWO_AUDIO_PLAN.md` §4 names "variant variety on **repeated blocks**" as
    /// a thing only a human can grade, and stone is that block. And the chicken
    /// has **two** step variants, not three — a first draft listed `step3.ogg`,
    /// which is not in the index. The pass aborted rather than quietly playing
    /// two of three, which is the fail-closed behaviour working; `preflight`
    /// below now moves that failure to the first second instead of the third
    /// minute.
    pub const VARIANTS: [&str; 4] = [
        "minecraft/sounds/dig/stone1.ogg",
        "minecraft/sounds/dig/stone2.ogg",
        "minecraft/sounds/dig/stone3.ogg",
        "minecraft/sounds/dig/stone4.ogg",
    ];
    /// 11.5 s of ambience, 1 ch, 48000 Hz — long enough to hold a note while the
    /// listener sweeps a full circle, which is what stages 6 and 7 need.
    ///
    /// (A first draft's comment said 35.5 s, inferred from the 35.5 KB the index
    /// reports. That is the *compressed* size and says nothing about duration;
    /// `preflight` prints the real one, which is how the error was caught.)
    pub const CAVE: &str = "minecraft/sounds/ambient/cave/cave14.ogg";
}

/// `SoundEngine`'s ordinary attenuation distance for a normal-range sound
/// (`sound_engine.rs:2881`, and the same literal in vanilla's own tests).
const ATTEN: f32 = 16.0;

/// A monotonically increasing channel id.
///
/// Fresh per voice rather than reused: the mixer retires a finished voice from
/// the callback, so reusing an id races that retirement, and a stage that
/// occasionally lost its sound would be indistinguishable from a real defect.
struct Ids(ChannelId);
impl Ids {
    fn next(&mut self) -> ChannelId {
        self.0 += 1;
        self.0
    }
}

/// A voice under this harness's control, built in vanilla's call order.
///
/// The order is `SoundEngine.java:417-434` — properties, then attach, then
/// play — and it is not cosmetic: `alSourcePlay` before a buffer is attached is
/// a no-op on a real device, so a builder that played first would produce
/// silence with every individual call correct.
struct Voice<'a> {
    sink: &'a CpalSink,
    id: ChannelId,
    ok: bool,
}

impl<'a> Voice<'a> {
    fn new(sink: &'a CpalSink, id: ChannelId) -> Voice<'a> {
        Voice {
            sink,
            id,
            ok: true,
        }
    }
    fn call(mut self, c: C) -> Self {
        self.ok &= self.sink.ring().push(Command::Channel(self.id, c));
        self
    }
    fn pitch(self, v: f32) -> Self {
        self.call(C::SetPitch(v))
    }
    fn volume(self, v: f32) -> Self {
        self.call(C::SetVolume(v))
    }
    fn attenuated(self, max: f32) -> Self {
        self.call(C::LinearAttenuation(max))
    }
    fn unattenuated(self) -> Self {
        self.call(C::DisableAttenuation)
    }
    fn looping(self, v: bool) -> Self {
        self.call(C::SetLooping(v))
    }
    fn at(self, x: f64, y: f64, z: f64) -> Self {
        self.call(C::SetSelfPosition(x, y, z))
    }
    fn relative(self, v: bool) -> Self {
        self.call(C::SetRelative(v))
    }
    /// Attach and play. Returns the id so a stage can move or stop it later.
    fn play(mut self, pcm: Arc<Pcm>) -> ChannelId {
        self.ok &= self.sink.ring().push(Command::Attach(self.id, pcm));
        self.ok &= self.sink.ring().push(Command::Channel(self.id, C::Play));
        if !self.ok {
            eprintln!(
                "    !! the ring refused part of channel {}'s sequence — the voice is \
                 half-configured, so whatever you hear next is not the stage's claim",
                self.id
            );
        }
        self.id
    }
}

/// Point the ears somewhere. `listener_basis` is production
/// (`sound_engine.rs:298`), deliberately not re-derived here: a harness with its
/// own copy of the basis would agree with itself while disagreeing with the
/// client, which is the failure `REWO_PLAN.md` records as M45's `install_shapes`.
///
/// # Which way is forward — the trap that put every source in this file backwards
///
/// **Yaw 0 faces +Z, not -Z**, and the first draft of these stages placed its
/// sources against -Z because that is what `ListenerTransform::INITIAL` says.
/// Both are right. `Camera.setRotation` opens `rotationYXZ(PI - yRot, …)`
/// (`Camera.java:337-342`), a half turn, so the `FORWARDS` constant `(0,0,-1)`
/// comes out of `listener_basis(0, 0)` as `(0,0,+1)` — while `INITIAL` is the
/// transform a listener has *before any camera exists* and keeps the raw
/// constant. So the two disagree by 180°, and a stage written against the wrong
/// one is audible but backwards, which is exactly the kind of wrong that sounds
/// fine on its own.
///
/// Working from that: at yaw 0, `right = forward x up` is `(0,0,1) x (0,1,0)`
/// = **-X**. So **+Z is ahead, -X is your right, +X is your left**, and every
/// position below is placed on that basis.
///
/// Pitch signs follow Minecraft's: **-90 is straight up**, +90 straight down.
fn look(sink: &CpalSink, pos: [f64; 3], yaw_deg: f32, pitch_deg: f32) {
    let (forward, up) = listener_basis(yaw_deg, pitch_deg);
    sink.ring().push(Command::Listener(ListenerTransform {
        position: pos,
        forward,
        up,
    }));
}

fn moved(sink: &CpalSink, id: ChannelId, x: f64, y: f64, z: f64) {
    sink.ring().push(Command::Channel(id, C::SetSelfPosition(x, y, z)));
}

fn stop(sink: &CpalSink, id: ChannelId) {
    sink.ring().push(Command::Channel(id, C::Stop));
}

fn nap(secs: f32) {
    std::io::stdout().flush().ok();
    std::thread::sleep(Duration::from_secs_f32(secs));
}

/// Announce a step within a stage, so the ear knows what it is hearing *while*
/// it hears it rather than afterwards.
fn beat(label: &str) {
    println!("      · {label}");
    std::io::stdout().flush().ok();
}

// ---------------------------------------------------------------------------
// The stages
// ---------------------------------------------------------------------------

struct Stage {
    n: u32,
    name: &'static str,
    /// What §4 property this reaches. One line.
    grades: &'static str,
    /// What a failure sounds like. This is the part that makes a stage useful.
    fails: &'static str,
    run: fn(&CpalSink, &mut Ids) -> Result<(), String>,
}

const STAGES: &[Stage] = &[
    Stage {
        n: 1,
        name: "it plays at all",
        grades: "the decode, the device and the clip's own ends",
        fails: "silence; a click or thump at either end; a tail cut short",
        run: stage_plays,
    },
    Stage {
        n: 2,
        name: "the resampler (A/B)",
        grades: "a 44100 source stretched to the device rate, against a native one",
        fails: "grain, a whistle, or a metallic edge on the harp that is absent on the horn",
        run: stage_resampler,
    },
    Stage {
        n: 3,
        name: "a stereo source",
        grades: "the multi-channel path, and a MEASURED divergence in level (M139)",
        fails: "one channel missing; the image collapsed to centre; a phasey swirl.
              Note the 8-block horn being quieter is EXPECTED and is the divergence",
        run: stage_stereo,
    },
    Stage {
        n: 4,
        name: "distance, to exactly zero",
        grades: "the linear attenuation curve and its cutoff at max",
        fails: "audible at or past 16 blocks; a step rather than a ramp; no change at all",
        run: stage_distance,
    },
    Stage {
        n: 5,
        name: "a flyby",
        grades: "one LIVE voice moved continuously — gain and pan interpolation",
        fails: "zipper noise; a stepped or granular sweep; the sound jumping rather than passing",
        run: stage_flyby,
    },
    Stage {
        n: 6,
        name: "pan: left, centre, right",
        grades: "the pan law with the listener held still",
        fails: "both channels equal throughout; left and right swapped",
        run: stage_pan,
    },
    Stage {
        n: 7,
        name: "the listener turning",
        grades: "the basis under yaw — the image must counter-rotate",
        fails: "the image follows your turn instead of opposing it; it does not move",
        run: stage_turning,
    },
    Stage {
        n: 8,
        name: "looking straight up",
        grades: "the up vector at pitch -90, where a basis pinned to (0,1,0) degenerates",
        fails: "the image snaps to dead centre as the tilt reaches -90",
        run: stage_pitched_listener,
    },
    Stage {
        n: 9,
        name: "pitch changes length",
        grades: "the resampler driven as a pitch control",
        fails: "the pitch moves but the duration does not (or the reverse)",
        run: stage_pitch,
    },
    Stage {
        n: 10,
        name: "a UI sound while walking away",
        grades: "AL_SOURCE_RELATIVE — the contrast with stage 4",
        fails: "it fades or pans as the listener moves; it should do neither",
        run: stage_relative,
    },
    Stage {
        n: 11,
        name: "variants differ",
        grades: "that the four dig/stone variants are audibly distinct",
        fails: "four identical sounds",
        run: stage_variants,
    },
    Stage {
        n: 12,
        name: "a crowd",
        grades: "mixing density, the limiter, and the counters underneath",
        fails: "crackle, dropouts, or a mix that pumps as voices come and go",
        run: stage_crowd,
    },
    Stage {
        n: 13,
        name: "a static loop",
        grades: "SetLooping — the seam at the wrap",
        fails: "it plays once and stops; a click or a gap at each wrap",
        run: stage_loop,
    },
];

fn stage_plays(sink: &CpalSink, ids: &mut Ids) -> Result<(), String> {
    let pcm = load(clip::LEVELUP)?;
    println!(
        "    {} — {:.2} s, {} ch, {} Hz. Three times, with silence between.",
        clip::LEVELUP,
        seconds_of(&pcm),
        pcm.channels,
        pcm.sample_rate
    );
    let d = seconds_of(&pcm);
    for _ in 0..3 {
        Voice::new(sink, ids.next())
            .pitch(1.0)
            .volume(0.8)
            .unattenuated()
            .looping(false)
            .at(0.0, 0.0, 0.0)
            .relative(true)
            .play(Arc::clone(&pcm));
        nap(d + 0.5);
    }
    Ok(())
}

fn stage_resampler(sink: &CpalSink, ids: &mut Ids) -> Result<(), String> {
    let harp = load(clip::HARP)?;
    let horn = load(clip::HORN)?;
    println!(
        "    A: harp   {} Hz  (device is {} Hz — resampled)",
        harp.sample_rate,
        sink.out_rate()
    );
    println!(
        "    B: horn   {} Hz  ({})",
        horn.sample_rate,
        if horn.sample_rate == sink.out_rate() {
            "matches the device — NOT resampled"
        } else {
            "also resampled on this device; the A/B is void, say so"
        }
    );
    if horn.sample_rate == sink.out_rate() {
        println!("    Any artefact on A that is absent on B is the resampler, not the decode.");
    }
    for _ in 0..2 {
        beat("A — harp, resampled");
        Voice::new(sink, ids.next())
            .pitch(1.0)
            .volume(0.8)
            .unattenuated()
            .looping(false)
            .at(0.0, 0.0, 0.0)
            .relative(true)
            .play(Arc::clone(&harp));
        nap(seconds_of(&harp) + 0.4);
    }
    beat("B — horn, native rate");
    Voice::new(sink, ids.next())
        .pitch(1.0)
        .volume(0.8)
        .unattenuated()
        .looping(false)
        .at(0.0, 0.0, 0.0)
        .relative(true)
        .play(Arc::clone(&horn));
    nap(seconds_of(&horn) + 0.4);
    Ok(())
}

fn stage_stereo(sink: &CpalSink, ids: &mut Ids) -> Result<(), String> {
    let horn = load(clip::HORN)?;
    println!(
        "    {} is {} ch — its siblings call0-2 and call4-7 are 1 ch, so a goat horn",
        clip::HORN,
        horn.channels
    );
    println!("    is stereo on one roll in eight.");
    println!();
    println!("    WHAT REWO ACTUALLY DOES, read rather than assumed. `pan_gains`");
    println!("    (mixer.rs:400-406) returns (1.0, 1.0) whenever channels >= 2, so a stereo");
    println!("    source is NOT panned — which MATCHES OpenAL, since it does not spatialise a");
    println!("    multi-channel buffer. The level is a separate factor: `gain = v.gain *");
    println!("    attenuation` (mixer.rs:298) applies to a stereo source like any other, so");
    println!("    Rewo spatialises it in LEVEL but not in DIRECTION.");
    println!();
    println!("    AND THE LEVEL IS A MEASURED DIVERGENCE. M139's loopback oracle settled what");
    println!("    the plan had only marked [concurring]: OpenAL does not ATTENUATE a");
    println!("    multi-channel buffer either, not merely decline to pan it — its stereo.d1p0");
    println!("    and stereo.d8p0 captures are BYTE-IDENTICAL across an eightfold distance");
    println!("    change, while halving AL_GAIN halves the output. Rewo applies linear_gain");
    println!("    regardless of channel count (mixer.rs:294-298 has no channel gate, unlike");
    println!("    pan_gains), so Rewo fades a stereo source that vanilla holds at full level:");
    println!("    -6.02 dB at 8 of 16 blocks, to silence at the radius.");
    println!();
    println!("    So this stage plays it BOTH ways. First relative and unattenuated, where");
    println!("    neither factor is in play and you are grading only that both channels arrive");
    println!("    coherently. Then positional at 0 and 8 blocks, where vanilla would give you");
    println!("    two identical horns and Rewo gives you a quiet second one. That difference");
    println!("    is real, it is recorded rather than fixed, and hearing it is the point.");
    let d = seconds_of(&horn);
    beat("relative + unattenuated — both channels, coherent image");
    Voice::new(sink, ids.next())
        .pitch(1.0)
        .volume(0.8)
        .unattenuated()
        .looping(false)
        .at(0.0, 0.0, 0.0)
        .relative(true)
        .play(Arc::clone(&horn));
    nap(d + 0.5);

    look(sink, [0.0, 0.0, 0.0], 0.0, 0.0);
    for step in [0.0f64, 8.0] {
        beat(&format!(
            "positional at {step:>4.1} blocks{}",
            if step > 0.0 {
                "   <- vanilla: unchanged. Rewo: -6.02 dB"
            } else {
                ""
            }
        ));
        Voice::new(sink, ids.next())
            .pitch(1.0)
            .volume(1.0)
            .attenuated(ATTEN)
            .looping(false)
            .at(0.0, 0.0, step)
            .relative(false)
            .play(Arc::clone(&horn));
        nap(d + 0.4);
    }
    Ok(())
}

fn stage_distance(sink: &CpalSink, ids: &mut Ids) -> Result<(), String> {
    let pcm = load(clip::LEVELUP)?;
    let d = seconds_of(&pcm);
    look(sink, [0.0, 0.0, 0.0], 0.0, 0.0);
    println!("    Attenuation distance is {ATTEN} blocks. The curve is LINEAR, so the last");
    println!("    step must be SILENT — not merely quiet. That is the property an");
    println!("    inverse-square curve cannot have, and it is the one to listen for.");
    for step in [0.0f64, 4.0, 8.0, 12.0, 15.5, 16.0] {
        beat(&format!(
            "{step:>5.1} blocks ahead{}",
            if step >= ATTEN as f64 {
                "   <- must be silent"
            } else {
                ""
            }
        ));
        Voice::new(sink, ids.next())
            .pitch(1.0)
            .volume(1.0)
            .attenuated(ATTEN)
            .looping(false)
            // +Z is ahead at yaw 0 — see `look`. The first draft used -Z, which
            // is directly behind, and would have graded the same curve while
            // describing the opposite scene.
            .at(0.0, 0.0, step)
            .relative(false)
            .play(Arc::clone(&pcm));
        nap(d + 0.35);
    }
    Ok(())
}

/// One live voice walked past the listener, rather than respawned at each mark.
///
/// **Stage 4 cannot see this and neither can stage 6.** Both fire a *fresh*
/// voice at a fixed position, so they grade the gain and the pan at the moment a
/// sound starts. What a player actually hears is a source that moves while
/// sounding — an arrow, a minecart, a mob walking by — and the defect class
/// there is different: a gain or pan recomputed per block rather than smoothed
/// gives zipper noise, which is inaudible on any sound that never moves.
fn stage_flyby(sink: &CpalSink, ids: &mut Ids) -> Result<(), String> {
    let pcm = load(clip::CAVE)?;
    look(sink, [0.0, 0.0, 0.0], 0.0, 0.0);
    println!("    One voice crossing from your right (-X) to your left (+X), 6 blocks");
    println!("    ahead, over ~8 s, updated every 80 ms.");
    println!();
    println!("    Stages 4 and 6 fire a NEW voice at each mark, so they grade gain and pan");
    println!("    at the instant a sound starts. This one moves a LIVE voice, which is the");
    println!("    only stage that can hear a per-update recomputation instead of a smoothed");
    println!("    one. Zipper noise sounds like a faint buzz or grain riding the sweep, and");
    println!("    it tracks the update rate rather than the sound.");
    let id = Voice::new(sink, ids.next())
        .pitch(1.0)
        .volume(1.0)
        .attenuated(ATTEN)
        .looping(true)
        .at(-10.0, 0.0, 6.0)
        .relative(false)
        .play(Arc::clone(&pcm));
    let steps = 100;
    for i in 0..=steps {
        let x = -10.0 + 20.0 * (i as f64 / steps as f64);
        if i % 25 == 0 {
            beat(&format!("x = {x:>5.1}"));
        }
        moved(sink, id, x, 0.0, 6.0);
        nap(0.08);
    }
    stop(sink, id);
    nap(0.3);
    Ok(())
}

fn stage_pan(sink: &CpalSink, ids: &mut Ids) -> Result<(), String> {
    let pcm = load(clip::LEVELUP)?;
    let d = seconds_of(&pcm);
    look(sink, [0.0, 0.0, 0.0], 0.0, 0.0);
    println!("    Listener at the origin, still, facing +Z — yaw 0 is a HALF TURN from the");
    println!("    -Z `FORWARDS` constant, because Camera.setRotation opens with PI - yRot.");
    println!("    Right = forward x up = (0,0,1) x (0,1,0) = -X, so a source at -X must be on");
    println!("    your RIGHT. If left and right are swapped here the cross product is the");
    println!("    wrong way round (up x forward), and every sound in the game is mirrored.");
    for (label, x) in [("hard right (-X)", -6.0f64), ("centre", 0.0), ("hard left (+X)", 6.0)] {
        beat(label);
        Voice::new(sink, ids.next())
            .pitch(1.0)
            .volume(1.0)
            .attenuated(ATTEN)
            .looping(false)
            .at(x, 0.0, 0.0)
            .relative(false)
            .play(Arc::clone(&pcm));
        nap(d + 0.35);
    }
    Ok(())
}

fn stage_turning(sink: &CpalSink, ids: &mut Ids) -> Result<(), String> {
    let pcm = load(clip::CAVE)?;
    println!("    One long source, FIXED at +X (your left). You turn a full circle over ~12 s.");
    println!("    The image must sweep the opposite way to your turn, smoothly, and pass");
    println!("    through centre twice. An image that does not move means the listener");
    println!("    transform is not reaching the mixer at all.");
    look(sink, [0.0, 0.0, 0.0], 0.0, 0.0);
    let id = Voice::new(sink, ids.next())
        .pitch(1.0)
        .volume(1.0)
        .attenuated(ATTEN)
        .looping(true)
        .at(6.0, 0.0, 0.0)
        .relative(false)
        .play(Arc::clone(&pcm));
    for i in 0..=48 {
        let yaw = i as f32 * 360.0 / 48.0;
        if i % 12 == 0 {
            beat(&format!("yaw {yaw:>5.0}°"));
        }
        look(sink, [0.0, 0.0, 0.0], yaw, 0.0);
        nap(0.25);
    }
    stop(sink, id);
    nap(0.3);
    Ok(())
}

fn stage_pitched_listener(sink: &CpalSink, ids: &mut Ids) -> Result<(), String> {
    let pcm = load(clip::CAVE)?;
    println!("    Source FIXED at +X (your left). You tilt from level to straight up over ~6 s.");
    println!("    Minecraft's pitch sign: -90 is up, +90 is down. This uses -90.");
    println!();
    println!("    This is the one stage that can see a degenerate basis. `right = forward x up`");
    println!("    is (-cos yaw, 0, -sin yaw) at EVERY pitch, so almost every way of breaking");
    println!("    the basis is inaudible. The exception: pin `up` to the constant (0,1,0) and");
    println!("    at pitch -90 forward becomes (0,+1,0) too, making forward x up the ZERO");
    println!("    vector — and the stereo image collapses to dead centre. Correctly, `up`");
    println!("    tilts to -Z there and the image stays left. If it snaps to the middle as the");
    println!("    tilt completes, that is the bug.");
    look(sink, [0.0, 0.0, 0.0], 0.0, 0.0);
    let id = Voice::new(sink, ids.next())
        .pitch(1.0)
        .volume(1.0)
        .attenuated(ATTEN)
        .looping(true)
        .at(6.0, 0.0, 0.0)
        .relative(false)
        .play(Arc::clone(&pcm));
    for i in 0..=24 {
        let pitch = -(i as f32) * 90.0 / 24.0;
        if i % 8 == 0 {
            beat(&format!("pitch {pitch:>4.0}°"));
        }
        look(sink, [0.0, 0.0, 0.0], 0.0, pitch);
        nap(0.25);
    }
    beat("held at -90° (straight up) — the image must still be left, not centred");
    nap(2.0);
    stop(sink, id);
    nap(0.3);
    Ok(())
}

fn stage_pitch(sink: &CpalSink, ids: &mut Ids) -> Result<(), String> {
    let pcm = load(clip::HARP)?;
    let d = seconds_of(&pcm);
    println!("    The same {d:.2} s harp at three pitches. Pitch is a resampling rate, so it");
    println!("    must change the DURATION too: 0.5 is an octave down and twice as long,");
    println!("    2.0 an octave up and half as long. A pitch that moves without the length");
    println!("    moving is a pitch shifter, which is not what vanilla has.");
    for p in [0.5f32, 1.0, 2.0] {
        beat(&format!("pitch {p}  -> expect ~{:.2} s", d / p));
        Voice::new(sink, ids.next())
            .pitch(p)
            .volume(0.8)
            .unattenuated()
            .looping(false)
            .at(0.0, 0.0, 0.0)
            .relative(true)
            .play(Arc::clone(&pcm));
        nap(d / p + 0.45);
    }
    Ok(())
}

fn stage_relative(sink: &CpalSink, ids: &mut Ids) -> Result<(), String> {
    let pcm = load(clip::LEVELUP)?;
    let d = seconds_of(&pcm);
    println!("    Same clip, SetRelative(true) — a UI sound, positioned in listener space.");
    println!("    The listener walks from 0 to 40 blocks away, well past the {ATTEN}-block");
    println!("    cutoff that silenced stage 4. This one must NOT fade and must NOT pan.");
    for step in [0.0f64, 10.0, 25.0, 40.0] {
        beat(&format!("listener {step:>4.0} blocks away — must be unchanged"));
        look(sink, [step, 0.0, 0.0], 0.0, 0.0);
        Voice::new(sink, ids.next())
            .pitch(1.0)
            .volume(0.8)
            .unattenuated()
            .looping(false)
            .at(0.0, 0.0, 0.0)
            .relative(true)
            .play(Arc::clone(&pcm));
        nap(d + 0.35);
    }
    look(sink, [0.0, 0.0, 0.0], 0.0, 0.0);
    Ok(())
}

fn stage_variants(sink: &CpalSink, ids: &mut Ids) -> Result<(), String> {
    println!("    The four dig/stone variants, in order, twice - the \"repeated block\" case.");
    println!();
    println!("    SCOPE, precisely: this grades that the four FILES are audibly distinct.");
    println!("    Whether the weighted seeded PICK chooses between them is a unit-tested");
    println!("    property of `sounds.json` resolution, not something an ear can confirm —");
    println!("    so do not read four different sounds here as evidence the selector works.");
    let pcms: Vec<Arc<Pcm>> = clip::VARIANTS
        .iter()
        .map(|k| load(k))
        .collect::<Result<_, _>>()?;
    for _ in 0..2 {
        for (i, pcm) in pcms.iter().enumerate() {
            beat(&format!("stone{}", i + 1));
            Voice::new(sink, ids.next())
                .pitch(1.0)
                .volume(1.0)
                .unattenuated()
                .looping(false)
                .at(0.0, 0.0, 0.0)
                .relative(true)
                .play(Arc::clone(pcm));
            nap(0.45);
        }
        nap(0.5);
    }
    Ok(())
}

fn stage_crowd(sink: &CpalSink, ids: &mut Ids) -> Result<(), String> {
    let pcms: Vec<Arc<Pcm>> = clip::VARIANTS
        .iter()
        .map(|k| load(k))
        .collect::<Result<_, _>>()?;
    println!("    24 positional voices scattered around you, fired in bursts.");
    println!();
    println!("    SCOPE: the mixer has no voice cap — vanilla's budget (25 static / 5");
    println!("    streaming, from Library.DEFAULT_CHANNEL_COUNT = 30) is enforced upstream in");
    println!("    `sound_engine`, which this harness bypasses. So this grades mixing density");
    println!("    and the limiter, NOT the budget.");
    let before_drop = sink.ring().dropped();
    let before_err = sink.errors();
    for burst in 0..3 {
        beat(&format!("burst {}", burst + 1));
        for i in 0..24 {
            let a = i as f64 * std::f64::consts::TAU / 24.0;
            Voice::new(sink, ids.next())
                .pitch(0.85 + (i as f32 % 5.0) * 0.07)
                .volume(0.9)
                .attenuated(ATTEN)
                .looping(false)
                .at(a.cos() * 5.0, 0.0, a.sin() * 5.0)
                .relative(false)
                .play(Arc::clone(&pcms[i % pcms.len()]));
        }
        nap(1.2);
    }
    nap(0.5);
    println!(
        "    during this stage: {} commands dropped, {} stream errors",
        sink.ring().dropped() - before_drop,
        sink.errors() - before_err
    );
    println!("    Crackle with 0 drops is the mixer; crackle with drops is the device.");
    Ok(())
}

fn stage_loop(sink: &CpalSink, ids: &mut Ids) -> Result<(), String> {
    let pcm = load(clip::HARP)?;
    let d = seconds_of(&pcm);
    println!("    A {d:.2} s clip with SetLooping(true), for ~8 s — about {} wraps.", (8.0 / d) as u32);
    println!("    Listen at the seam: a click, a gap or a stutter each time round is the");
    println!("    wrap, and it will recur at a fixed interval rather than randomly.");
    println!();
    println!("    SCOPE: this is a STATIC loop. Music and the ambient beds are STREAMED, and");
    println!("    a stream's loop lives one layer down in LoopingAudioStream (which restarts");
    println!("    one read LATE, so it emits a short buffer at the loop point). This harness");
    println!("    has no chunk producer, so 'music not once-and-stopping' is NOT graded here —");
    println!("    that needs `rewo live --audio`.");
    let id = Voice::new(sink, ids.next())
        .pitch(1.0)
        .volume(0.7)
        .unattenuated()
        .looping(true)
        .at(0.0, 0.0, 0.0)
        .relative(true)
        .play(Arc::clone(&pcm));
    nap(8.0);
    stop(sink, id);
    nap(0.4);
    Ok(())
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

/// The ad-hoc mode: play one arbitrary key three times, as the first cut did.
fn play_key(sink: &CpalSink, ids: &mut Ids, key: &str) -> Result<(), String> {
    let pcm = load(key)?;
    let d = seconds_of(&pcm);
    println!(
        "{key}: {} ch, {} Hz, {} samples ({d:.2} s){}",
        pcm.channels,
        pcm.sample_rate,
        pcm.samples.len(),
        if d < 0.25 {
            "   <- under a quarter second; too short to judge ends or crackle on"
        } else {
            ""
        }
    );
    for _ in 0..3 {
        Voice::new(sink, ids.next())
            .pitch(1.0)
            .volume(0.9)
            .unattenuated()
            .looping(false)
            .at(0.0, 0.0, 0.0)
            .relative(true)
            .play(Arc::clone(&pcm));
        nap(d.max(0.25) + 0.35);
    }
    Ok(())
}

fn list() {
    println!("The listening pass — {} stages.\n", STAGES.len());
    for s in STAGES {
        println!("  {:>2}. {}", s.n, s.name);
        println!("      grades: {}", s.grades);
        println!("      fails:  {}", s.fails);
    }
    println!();
    println!("  cargo run -p rewo-audio --example listen -- --stage <N>");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut only: Option<u32> = None;
    let mut key: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--list" => {
                list();
                return;
            }
            "--stage" => {
                i += 1;
                match args.get(i).and_then(|s| s.parse::<u32>().ok()) {
                    Some(n) if STAGES.iter().any(|s| s.n == n) => only = Some(n),
                    _ => {
                        eprintln!("--stage wants one of 1..={}; see --list", STAGES.len());
                        std::process::exit(2);
                    }
                }
            }
            "--key" => {
                i += 1;
                match args.get(i) {
                    Some(k) => key = Some(k.clone()),
                    None => {
                        eprintln!("--key wants an asset key");
                        std::process::exit(2);
                    }
                }
            }
            // Bare argument: the first cut's interface, kept so muscle memory
            // and any note that recorded a key still work.
            other => key = Some(other.to_string()),
        }
        i += 1;
    }

    let sink = match CpalSink::open() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("no audio device: {e}");
            std::process::exit(2);
        }
    };
    let mut ids = Ids(0);
    println!("device open at {} Hz.", sink.out_rate());
    println!();

    if let Some(k) = key {
        if let Err(e) = play_key(&sink, &mut ids, &k) {
            eprintln!("{e}");
            eprintln!("this needs an unpacked 26.2 asset store; see REWO_PLAN.md");
            std::process::exit(2);
        }
        report(&sink);
        return;
    }

    if let Err(e) = preflight(sink.out_rate()) {
        eprintln!("preflight failed: {e}");
        eprintln!("this needs an unpacked 26.2 asset store; see REWO_PLAN.md");
        std::process::exit(2);
    }

    let chosen: Vec<&Stage> = STAGES.iter().filter(|s| only.is_none_or(|n| s.n == n)).collect();
    if only.is_none() {
        println!("Running all {} stages. Roughly three minutes.", chosen.len());
        println!("Each one says what it grades and what a failure sounds like.");
        println!("`--list` shows them; `--stage N` runs one.");
        println!();
    }

    for s in &chosen {
        println!("── {}. {} {}", s.n, s.name, "─".repeat(46usize.saturating_sub(s.name.len())));
        println!("    GRADES: {}", s.grades);
        println!("    FAILS:  {}", s.fails);
        if let Err(e) = (s.run)(&sink, &mut ids) {
            eprintln!("    !! stage {} could not run: {e}", s.n);
            eprintln!("       a listening pass that skipped a stage would read as a pass; stopping.");
            eprintln!("       this needs an unpacked 26.2 asset store; see REWO_PLAN.md");
            std::process::exit(2);
        }
        println!();
        nap(0.4);
    }

    report(&sink);
    println!();
    println!("Record the outcome in REWO_PLAN.md §15 — which stages passed, which did not,");
    println!("and by whom. REWO_AUDIO_PLAN.md §4 requires that: without a written result,");
    println!("\"verified\" quietly comes to mean \"the gate was green\", which for this");
    println!("subsystem is the one place in Rewo where that inference does not hold.");
}

/// The two numbers worth reading together: errors with drops is a stalled
/// device, drops alone is a callback not keeping up, and neither with silence
/// means the sound reached the mixer and the mixer is wrong.
fn report(sink: &CpalSink) {
    println!(
        "done. stream errors: {}, commands dropped: {}",
        sink.errors(),
        sink.ring().dropped()
    );
}
