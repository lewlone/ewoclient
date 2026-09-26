---
paths:
  - "crates/rewo-audio/**"
  - "crates/rewo-net/src/sound*.rs"
  - "crates/rewo-net/src/tickable.rs"
  - "crates/rewo-net/src/music.rs"
  - "crates/rewo-net/src/ambient_handlers.rs"
  - "crates/rewo-world/src/music.rs"
  - "crates/rewo-world/src/ambient.rs"
  - "crates/rewo-world/src/end_flash.rs"
  - "crates/rewo-data/src/sound*.rs"
  - "crates/rewo-app/src/audio_backend.rs"
  - "crates/rewo-app/src/soundshot_cmd.rs"
  - "REWO_AUDIO_PLAN.md"
---

# Rewo audio

`REWO_AUDIO_PLAN.md` is the plan and decision record (cpal + symphonia, a
caller-driven `Mixer::render`). Milestone reasoning: `docs/rewo/` and
`docs/history/claude-md-session-log.md` (M138–M151 footers).

## Containment (don't break it)

- The `audio` feature on `rewo-app` is **off by default**. A default build links
  no cpal/symphonia (verify with `cargo tree`, don't assert it), and the other
  gates don't link an audio stack.
- **No test opens a device.** `cargo test` stays silent. The only path to a
  sound is `rewo live --audio` on a `--features audio` build or
  `cargo run -p rewo-audio --example listen`.
- **A green `soundshot` isn't evidence the client makes a sound.** An absent,
  muted, exclusive-mode or unplugged device all look identical from inside the
  process. **The listening pass is the user's.**
- `soundshot` has two locks: the default build grades wire, resolution and
  engine arithmetic; `--features audio` adds decode and the mixer. It fails
  closed on whichever applies.

## Traps

- **`stopped()` for a real device can't answer `true` unconditionally** the way
  `SilentDevice` does: `schedule_tick` releases on `true`, which turns every
  sound into a 50 ms click. It's modelled from the buffer's own length on the
  producer side.
- Streaming refills on a **buffer count**, not a duration. An underrun isn't a
  death; the producer decides when a stream ends. Streams need an **epoch** (a
  late chunk is a position), where static buffers don't.
- The command ring drops the **newest** command when full and never blocks the
  render thread. Last-`Arc<Pcm>` deallocation in the callback is a stated
  deviation.
- **Stereo attenuation:** OpenAL doesn't attenuate multi-channel buffers, while
  Rewo's `render()` does (M139/M153). It's an owned, recorded decision; don't
  "fix" it silently.
- `sounds.json` and every `.ogg` come from the **asset index**, not the client
  jar. Missing variants drop and their weight moves with them. The
  `sound_event` registry is read by `protocol_id` (sorted-map trap).
- Tickable ramps: `Mth.lerp` takes its factor **first**, so named ceilings
  often aren't the real ceilings. `MinecartSoundInstance`'s pitch field is
  **shadowed** and never read by `getPitch()`. `canPlaySound()` is a per-class
  override: "follow this entity" isn't "silence-gate on this entity".
- A tickable ambient instance must be **constructed at volume 1.0**, or
  `SoundEngine.play` refuses it.
- Music: the Overworld's music comes from its **dimension type**
  (`background_music`), not a biome. Seed the frequency option through the
  constructor's plain read; `setMinutesBetweenSongs` re-rolls the delay (M161).
  `pale_garden` declares music volume **0.0**.
- The End flash's first 600 ticks never flash, and tick 0's intensity is NaN.
  Both are vanilla behaviour.
