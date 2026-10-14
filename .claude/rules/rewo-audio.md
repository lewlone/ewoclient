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
  render thread. `device::ring()` / `command_ring()` return exactly one
  `Producer` and one `Consumer` (neither `Clone` nor `Sync`, both `Send`; the
  storage is private), so SPSC is enforced by the types, not a comment
  (`compile_fail` doc tests pin it). `try_push` hands a refused value back.
- **The cpal data callback neither allocates nor frees**
  (`callback::RenderCallback`; `tests/rt_alloc.rs` counts both with a counting
  global allocator over new voices, queue joins, underruns, replacing
  attaches, `Stop` with a queue, finished one-shots and a NaN pitch). The
  mixer preallocates `MAX_VOICES` (256) slots, `MAX_QUEUED` (16) queued
  buffers per slot and one shared empty `Pcm`; overflow is **refused and
  counted, never grown**. Anything the mixer lets go of (the last `Arc<Pcm>`
  included) goes out as `Retired` over a second ring and is dropped by
  `CpalSink::service` on the engine thread. Don't add an allocation, a `Vec`
  growth, a `drop` of an `Arc` or a log call to the callback.
- **Device recovery.** The error callback only sets flags; `service` logs and
  rebuilds the stream on the current default device on `DeviceNotAvailable`, a
  heartbeat stalled for 2 s, or a changed default device (checked once a
  second), with 0.5 s to 30 s backoff (`Recovery`). The backoff is forgiven
  only after the heartbeat has flowed for 10 s (`STABLE`), not on a successful
  build, and recovery logs are rate-limited while rebuilds flap. The render state survives a
  rebuild (a `Drop` impl hands it back). Examples must call `service()` to get
  this; `examples/listen.rs` does not yet.
- Sample formats: f32 renders directly; i16 and u16 render into a preallocated
  scratch and convert per sample (`f32_to_i16`, `f32_to_u16`). Unit-tested, not
  yet exercised on a real device. Pitch is clamped to `calculatePitch`'s
  0.5..2.0 (NaN becomes 1.0) at the mixer boundary, or a voice never finishes.
- **Every decode job runs under `catch_unwind`** (`buffers::contained`): a
  panicking decoder is an `Err`, never a dead worker with keys stuck "in
  flight" (that parked channels for the whole session). A worker that dies
  anyway answers its in-flight keys with failures. Stream epochs are sink-wide.
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
