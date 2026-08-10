//! The listening pass — the one thing in this project a machine cannot do.
//!
//! ```text
//! cargo run -p rewo-audio --example listen
//! cargo run -p rewo-audio --example listen -- minecraft/sounds/mob/chicken/step1.ogg
//! ```
//!
//! **This is the only code path in Rewo that makes a noise**, and it is an
//! example rather than a `rewo` subcommand on purpose: wiring cpal into the
//! client binary would link an audio stack into all 34 gates for a subsystem
//! none of them exercises.
//!
//! `REWO_AUDIO_PLAN.md` §4 lists what to listen for, and it is worth having open
//! while running this. In short: does the sound play at all, at the right pitch
//! and speed, without clicks at its ends, and does a repeated block give
//! different variants rather than the same one every time.
//!
//! It reads the assets straight out of the store rather than through
//! `rewo-data`'s index, because this crate deliberately does not depend on it —
//! see `decode::BytesSource`.

use std::io::Write;
use std::sync::Arc;

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
    let hash_at = text[at..]
        .find("\"hash\"")
        .ok_or("malformed index entry")?
        + at;
    let start = text[hash_at..].find(':').ok_or("malformed")? + hash_at + 1;
    let q1 = text[start..].find('"').ok_or("malformed")? + start + 1;
    let q2 = text[q1..].find('"').ok_or("malformed")? + q1;
    let hash = &text[q1..q2];
    let p = store().join("objects").join(&hash[..2]).join(hash);
    std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))
}

fn main() {
    let key = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "minecraft/sounds/mob/chicken/step1.ogg".to_string());

    let bytes = match read_asset(&key) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("could not read {key}: {e}");
            eprintln!("this needs an unpacked 26.2 asset store; see REWO_PLAN.md");
            std::process::exit(2);
        }
    };
    let pcm = match rewo_audio::decode::decode_ogg_vorbis(&bytes) {
        Ok(p) => Arc::new(p),
        Err(e) => {
            eprintln!("decode failed: {e}");
            std::process::exit(2);
        }
    };
    let seconds = pcm.samples.len() as f32 / pcm.channels as f32 / pcm.sample_rate as f32;
    println!(
        "{key}: {} ch, {} Hz, {} samples ({seconds:.2} s)",
        pcm.channels,
        pcm.sample_rate,
        pcm.samples.len()
    );

    let sink = match rewo_audio::cpal_sink::CpalSink::open() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("no audio device: {e}");
            std::process::exit(2);
        }
    };
    println!("device open at {} Hz. playing three times...", sink.out_rate());

    for i in 0..3u32 {
        if !sink.play_once(i, Arc::clone(&pcm)) {
            eprintln!("the ring refused part of the sequence -- is the device alive?");
        }
        std::io::stdout().flush().ok();
        std::thread::sleep(std::time::Duration::from_secs_f32(seconds.max(0.25) + 0.35));
    }
    std::thread::sleep(std::time::Duration::from_millis(250));

    // The two numbers worth reading together: errors with drops is a stalled
    // device, drops alone is a callback not keeping up, and neither with silence
    // means the sound reached the mixer and the mixer is wrong.
    println!(
        "done. stream errors: {}, commands dropped: {}",
        sink.errors(),
        sink.ring().dropped()
    );
    println!();
    println!("WHAT TO LISTEN FOR (REWO_AUDIO_PLAN.md section 4):");
    println!("  * it plays at all, three times, at a sensible pitch and speed");
    println!("  * no click or thump at either end of the clip");
    println!("  * silence between the repeats, not a hum or a buzz");
    println!("  * nothing crackles -- crackle with 0 drops is the mixer, with");
    println!("    drops it is the device");
}
