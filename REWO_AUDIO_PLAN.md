# REWO_AUDIO_PLAN.md — the audio milestone

**Status: MOSTLY SHIPPED (2026-08-10).** `crates/rewo-audio` exists — the
quantisation, `SoundBufferLibrary`, the symphonia ogg decode, the `Mixer` and
`NullSink`, the SPSC command ring and a cpal sink — and `rewo-net` carries the
listener transform and the music fade. **M138a–d are done; M140's `level_event`
sounds and music fade shipped, and its ramps shipped as M141 (there are ten of
them, not the "~8" §M140 says); M139 has not started.** Each section below
carries its own status block; trust those over any forward-looking sentence in
the body, which was written before the code.

**Two things are true and easy to miss.** *(1)* **Nobody has listened to it.** No
gate in this project opens an audio device, so everything a machine can check
passes and that is **not** the same claim —
`cargo run -p rewo-audio --example listen` is the outstanding work and it is a
human's. *(2)* **`rewo live` is still silent**: `rewo-app` deliberately does not
depend on `rewo-audio`, so the 34 gates do not link an audio stack for a
subsystem none of them exercises.

*(This line read "a PLAN, not shipped code — nothing in `crates/` implements any
of it yet" until the header was updated to match its own body. It is the exact
failure this project keeps recording: a status line at the top of a file that its
own sections had already contradicted.)*

**Provenance.** Produced 2026-08-10 by a 14-agent survey → design → judge →
refute → synthesise pass over the 26.2 decompile and the Rewo tree. Read
`REWO_PLAN.md` §0.0 first; this file is the detail behind its "AUDIO" item.

**It carries three corrections to its own winning design** (§0), which is the
part to read first if you are short of time — each one silently breaks
something, and each was caught by re-reading the decompile rather than by
reasoning.

**The losing designs were rejected for stated, checkable reasons**, recorded so
nobody re-proposes them: *kira* interpolates attenuation in **decibels** where
Minecraft's curve is amplitude-linear (a 24 dB error at half the radius, with no
`Easing` variant able to correct it) and its distance gain **freezes**, because
only `tickingSounds` re-push volume (`SoundEngine.java:233-253`); *rodio*'s
specified voice **cannot play a stereo source** and all the music is stereo,
and its pan law is 0/0 for every UI sound.

**Numbering was rewritten on import.** The synthesis proposed M135–M137; those
numbers were taken the same day by three unrelated fixes, so its steps are
M138a–d, M139 and M140 here. Verify against `git log --oneline` before
branching — concurrent sessions assign numbers independently (§0.0).

**Sourcing convention, kept from the synthesis:** **[read]** means an agent
opened that file:line this session; **[concurring]** means two independent
surveys cite it identically without a direct read — one grade weaker. Six of its
sharpest claims were re-verified by hand before this file was committed: the
wire seed (`ClientboundSoundPacket:53`), the complete `Channel.java` surface
(no gain curve, no pan anywhere in Java), the three-factor volume formula
(`SoundEngine.java:467-469`), the **unclamped** volume used for range against
the clamped one used for gain (`:375-378`), `DEFAULT_CHANNEL_COUNT = 30`
(`Library.java:27`), and `Item`-style literals in `BeaconScreen`. All six held.

---

Every contested point is now settled against the decompile and the tree. Three of the winning design's claims about this repo were wrong, and the corrections change the plan — so the plan below carries them.

---

## M138–M140 — Audio: the mixer, the sink, and the listener seam M131 left out

**Numbering is provisional.** `main` is at M134; check `git log --oneline` before branching, because concurrent sessions assign numbers independently (`REWO_PLAN.md` §0.0).

**Citation convention.** `<D>` = `%APPDATA%/EwoClient/rewo/26.2/decompiled`. Facts marked **[read]** I opened this session. Facts marked **[concurring]** I did not open, but two independent surveys cite them identically — treat as one grade weaker.

---

## 0. Three corrections that change the design

I verified these before writing, because each one silently breaks something.

**0.1 — The channel count is 30, not 256.** A draft of this plan asserted "a software mixer has no hardware voice limit, so there is no honest derivation" and picked 256. Vanilla ships a documented fallback for exactly the case where the device advertises nothing: `DEFAULT_CHANNEL_COUNT = 30` (`<D>/com/mojang/blaze3d/audio/Library.java:27` **[read]**), returned by `getChannelCount()` at `Library.java:182` **[read]** when attribute `4112` (`ALC_MONO_SOURCES`) is absent from the list scanned at `:161` **[read]**. Rewo already ships it as the current default — `pub const DEFAULT_CHANNEL_COUNT: i32 = 30` at `crates/rewo-net/src/sound_engine.rs:45` **[read]**, whose doc comment already cites `Library.DEFAULT_CHANNEL_COUNT`. So 256 was not filling a vacuum; it would have **silently changed a decompile-cited, test-pinned default** from 25 static / 5 streaming to 248 / 248, and chosen the value at which the budget never binds — which the same draft then listed as a risk. Keep 30.

**0.2 — The streaming loop flag had nowhere to live, and music would have played once and stopped.** `SoundEngine.play` sets `channel.setLooping(isLooping && !isStreaming)` (`SoundEngine.java:426` **[read]**) — a streamed loop is told *not* to loop on the source. Looping for streams lives one layer down: `getStream(sound.getPath(), isLooping)` at `SoundEngine.java:436` **[read]**, and `SoundBufferLibrary.getStream(location, looping)` returns `looping ? new LoopingAudioStream(JOrbisAudioStream::new, is) : new JOrbisAudioStream(is)` (`SoundBufferLibrary.java:40-49` **[read]**). `LoopingAudioStream.read` restarts on an **empty** result — `if (!result.hasRemaining())`, then `bufferedInputStream.reset()` and re-create the decoder (`LoopingAudioStream.java:28-38` **[read]**, with the `mark(Integer.MAX_VALUE)` at `:18`). The seam already carries the flag: `AttachBufferStream(String, bool)` at `sound_engine.rs:147-149` **[read]**, doc-commented "the path and whether the stream itself loops". An architecture naming only `AttachStaticBuffer` drops it, and the symptom is that every music track and every `ambient.*.loop` bed plays once.

**0.3 — The "false green" is real but was mis-diagnosed, and the mis-diagnosis matters.** A draft claimed the asset-store reader "could be deleted entirely with all 149 sound tests green". False: `live_cmd.rs:22193-22209` **[read]** decides its SKIP from the **store on disk** and then asserts `!live.system.sounds.is_empty()`, and its comment records that this exact trap was already found by a mutation battery — *"replacing the whole loader with an empty index left this test green (it merely SKIPped)"*. Likewise the claim "every engine test passes `SoundFileSet::All`" is false: `sound_engine.rs:1777` **[read]** passes `Only(Default::default())` to drive `validateSoundResource`'s drop and asserts `NotStarted::EmptySound` (9 `All` sites against 1 `Only`). **The real hazards are two, and they are recorded two lines above the refutation** — `live_cmd.rs:22169-22173` **[read]**: *"`run_headless` and `LiveApp::frame` actually call `LiveSounds::drive`: those are composition roots in a binary crate with no seam… Deleting either call site survives the whole suite — measured with the mutation battery, not assumed."* Plus every store-dependent test self-skips on a bare machine. So the fix is a `--render-check` witness and a fail-closed `build_sounds`, not the M45/M92 "gate supplies an input production derives" sweep the draft invoked.

---

## 1. The decision, in three sentences

**cpal** for the device and **symphonia** (`ogg` + `vorbis` only) for decode, in a new `rewo-audio` crate, with a **pure `Mixer::render(&mut self, out: &mut [f32])`** that never names cpal and is driven identically by a `NullSink` (gate) and a `CpalSink` (production). rodio and kira are *engines* that own mixing, spatialisation and volume semantics — all of which are already transcribed across the 6,543 lines of `sound_engine.rs` / `sound_instance.rs` / `sounds.rs` / `sounds_json.rs` / `sound_events.rs` / `level_event_sounds.rs` **[read: `wc -l` = 6543 exactly]** — and kira in particular **cannot express vanilla's curve at all**, because it interpolates attenuation in decibels where Minecraft's is amplitude-linear, a 24 dB error at half the radius with no `Easing` variant able to correct it. cpal is the layer *under* rodio, which is precisely and only the layer Rewo lacks; choosing it means the seam's thirteen `ChannelCall` data variants (`sound_engine.rs:133-154` **[read]**) drive a mixer we own rather than being reinterpreted into a foreign vocabulary, and every reinterpretation would be an ungradeable step between the decompile and the sound.

---

## 2. The exact vanilla facts to transcribe

### 2.1 The call sequence — exactly eight calls, always

`SoundEngine.java:417-428` **[read]**, in order: `setPitch` (:418), `setVolume` (:419), `linearAttenuation` **or** `disableAttenuation` (:420-424 — the branch always takes one arm), `setLooping` (:426), `setSelfPosition` (:427), `setRelative` (:428). Then the attach, then play: `:430-434` for static, `:435-440` for streaming. Six properties + attach + play = **8**, not "7-8". Order is observable on a real device — `alSourcePlay` before a buffer is attached is a no-op — so a recorder storing a *set* could not distinguish a working client from a broken one.

### 2.2 The attach is deferred, and `play` blocks

`handleFuture.join()` at **`SoundEngine.java:405`** **[read]** — a hard cross-thread sync point per sound on the client thread. The attach is a continuation: `getCompleteBuffer(path).thenAccept(buf -> handle.execute(ch -> { ch.attachStaticBuffer(buf); ch.play(); }))` at `:431-434` **[read]**. Rewo transcribes the *asynchrony* and deliberately declines the *block* — a recorded divergence, not an accident.

### 2.3 The complete OpenAL source surface — and the honest gap

`Channel.java` **[read]**, every property vanilla ever writes:

| Call | Line | AL |
|---|---|---|
| `setSelfPosition` | :88-90 | `alSourcefv(4100)`, three floats |
| `setPitch` | :92-94 | `alSourcef(4099)` |
| `setLooping` | :96-98 | `alSourcei(4103)` |
| `setVolume` | :100-102 | `alSourcef(4106)` |
| `disableAttenuation` | :104-106 | `alSourcei(53248, 0)` |
| `linearAttenuation` | :108-113 | `53248 := 53251`, `4131 := maxDistance`, `4129 := 1.0`, `4128 := 0.0` |
| `setRelative` | :115-117 | `alSourcei(514)` |
| `attachStaticBuffer` | :119-121 | `alSourcei(4105)` |
| `stopped` | :84-86 | state `== 4116` |

**There is no gain curve and no pan anywhere in Java.** That is the design's central claim and it survives an exhaustive read. Attenuation is applied **per source**, which is why each sound can carry its own radius: `alEnable(512)` = `AL_SOURCE_DISTANCE_MODEL` at `Library.java:112` **[read]**, bracketed by hard throws if `AL_EXT_source_distance_model` (:108-110) or `AL_EXT_LINEAR_DISTANCE` (:113-115) is missing **[read]**. Reading `AL_DISTANCE_MODEL` as global state — its OpenAL 1.1 default — forces every concurrent sound onto one radius.

### 2.4 The listener is six floats, per frame

`Listener.java:14` **[read]** `alListener3f(4100, …)`; `:15` **[read]** `alListenerfv(4111, new float[]{fx,fy,fz,ux,uy,uz})`. Writing three is the classic bug and is correct until the player looks up. Built at `SoundEngine.java:493` **[read]** from `camera.position()`, `forwardVector()`, `upVector()`, and dispatched at `:494`. Called from **`Minecraft.java:1195`** **[read]**, inside `window.setErrorSection("Render")` (:1191) and after the tick loop — **per frame, not per tick.** Driving it from the 20 Hz tick quantises head rotation to 50 ms, audible as stepping in the stereo image while turning.

Basis: `Camera.java:339` **[read]** `rotation.rotationYXZ((float)Math.PI - yRot*(float)(Math.PI/180.0), -xRot*(float)(Math.PI/180.0), 0.0F)`, applied to `FORWARDS = (0,0,-1)` and `UP = (0,1,0)` (`Camera.java:42-43` **[read]**) at `:340-341`. The `PI -` and the negated pitch are in the quaternion, not the constants.

### 2.5 The quantisation — the one exact answer in the decode path

`ChunkedSampleByteBuf.java:28` **[read]**, verbatim:

```java
int intVal = Mth.clamp((int)(sample * 32767.5F - 0.5F), -32768, 32767);
```

Multiplier `32767.5`, bias applied **before** a C-style truncating cast. Reproduce it even though f32-end-to-end is more accurate — matching vanilla's lossy step *is* the fidelity claim. Note the latent asymmetry at `:17-18` **[read]**: the stored `bufferSize` rounds up to even (`bufferSize + 1 & -2`) but the first buffer allocates from the raw argument. Unreachable today (both callers pass even sizes **[concurring]**); a Rust port that rounds both is a deliberate divergence and should say so.

### 2.6 Volume, range, and the redirect's three different rules

- `calculateVolume` = `clamp(volume,0,1) * clamp(getFinalSoundSourceVolume(source),0,1) * gainBySource` (`SoundEngine.java:467-469` **[read]**).
- Master applied once, never squared: `source == MASTER ? get(source) : get(source) * get(MASTER)` (`Options.java:1303-1307` **[read]**).
- Range uses the **unclamped** volume while gain uses the clamped one: `attenuationDistance = Math.max(instanceVolume, 1.0F) * sound.getAttenuationDistance()` at `SoundEngine.java:376` **[read]**, against `:378`. Clamping once and reusing collapses every volume>1 sound (jukeboxes at 4.0) from 64 blocks to 16.
- A `type:"event"` redirect resolves **three fields three ways** (`SoundManager.java:272-281` **[read]**): volume and pitch **multiply** (`MultipliedFloats`, :274-275), weight comes from the **outer** (`sound.getWeight()`, :276), streaming is an **OR** (:278), attenuation comes from the **inner** (`wrappedSound.getAttenuationDistance()`, :280).

### 2.7 Looping is three mutually exclusive mechanisms

`SoundEngine.java:318-328` **[read]**: `requiresManualLooping = getDelay() > 0`; `shouldLoopManually = isLooping && requires`; `shouldLoopAutomatically = isLooping && !requires`. Combined with `:426`: AL looping for a static non-delayed loop, `LoopingAudioStream` for a streaming loop, the `queuedSounds` re-queue for a delayed loop. Setting a source-level loop on a streaming voice loops the four queued buffers — a fraction of a second stuttering.

### 2.8 Streaming geometry, the budget, and the limiter

- Ring is **4 buffers × 1 second**: `QUEUED_BUFFER_COUNT = 4`, `BUFFER_DURATION_SECONDS = 1` (`Channel.java:16-17` **[read]**), `calculateBufferSize(format, 1)` then `pumpBuffers(4)` (`:126-127` **[read]**). The `streamingBufferSize = 16384` field initialiser at `:20` **[read]** is a placeholder immediately overwritten — copying it as the real size underruns at 20 Hz refill.
- Pools: `streaming = clamp((int)Mth.sqrt(total), 2, 8)`, `static = clamp(total - streaming, 8, 255)` (`Library.java:102-103` **[read]**). The cast truncates and the two need not sum to `total`.
- Budget exhaustion **drops the newest** — `acquire` returns null, `play` returns `NOT_STARTED` (`SoundEngine.java:406-411` **[read]**). No eviction, no priority, no LRU.
- `MIN_SOURCE_LIFETIME`: `soundDeleteTime.put(instance, tickCount + 20)` (`SoundEngine.java:414` **[read]**).
- The **output limiter is unconditional**: `attr.put(6554).put(1)` at `Library.java:131` **[read]**, outside the HRTF guard. HRTF is gated twice — the attribute pair is written only when `alcGetInteger(device, 6548) > 0` (`:125-129` **[read]**) and its *value* is the user option.

---

## 3. Milestone breakdown — four steps, each independently shippable and gated

### M138a — the listener seam, and the two live false greens
**No new dependency.** Ships alone, merges green, and closes hazards that exist today.

> **SHIPPED IN FULL 2026-08-10** — all four items and r45; see `REWO_PLAN.md`
> §15. It landed in two commits (3 and 4 first, as live bugs that stand alone;
> then 1 and 2, the structural gap the device needs), which is why the log
> carries two entries.
>
> **Two things the plan did not anticipate.** `listener_basis`'s forward vector
> is *exactly* `Entity.calculateViewVector`, reached by a completely different
> route — which turns the transcription from self-consistent into checkable, and
> is the witness this step leans on. And **`ListenerTransform::INITIAL` is not a
> camera at yaw 0**: it is `Camera`'s raw `FORWARDS` constant, while
> `setRotation` opens with `rotationYXZ(pi - yaw, ...)`, so yaw 0 faces +Z and
> the record's default faces -Z. A test asserting they agree looks obviously
> right and puts the ears backwards.
>
> **r45 asserts `pushes == frames`, not `pushes > 0`**, because the push
> belonging on the render path rather than the tick loop is the whole claim; a
> per-tick client would still be non-zero. Verified by deleting the call site:
> 0 of 5783, 44/45, with the unit suite entirely green.
>
> One correction from doing them: item 4 is smaller than "one index" suggests in
> only one respect and larger in another. The decode really is one match arm, but
> **the witness that matters is the one asserting the sound world READS the
> flag** — every other test passes against a decode stored where nothing looks,
> which was the pre-existing state. Reverting the consumer is a mutation in the
> battery for exactly that reason.

1. **A fifth trait method**, not a `ChannelCall` variant: `fn set_listener(&mut self, t: ListenerTransform)`. The listener is not a channel, and a fake channel id would be both ugly and unassertable. `RecordingDevice` gains `listener_history`. Today `AudioDevice` is four methods with **no listener call** (`sound_engine.rs:165-180` **[read]**), so every `SetSelfPosition` is an absolute world coordinate panned against the origin facing +Z.
2. **`listener_basis(yaw, pitch) -> (forward, up)`** in `rewo-net`, transcribing `Camera.java:339-341`. The handedness lives where the tests are (M97's lesson). `rewo-app` passes camera angles, **per frame**.
3. **`build_sounds` stops swallowing** — today a missing sounds.json becomes an empty index behind a `log::info!` (`live_cmd.rs:3820-3828` **[concurring]**), behaviourally identical to totally broken resolution. Fail closed under `--render-check`.
4. **`DATA_SILENT`** (SynchedEntityData index 4) so `entity_silent` stops answering a hardcoded `false` (`sound_engine.rs:1168-1175` **[read]**). One index; without it a `/data`-silenced mob is audible and the symptom gets blamed on the device.

**Gate:** unit tests + `--render-check` **r45** — inject a `sound` packet through the real router, assert non-zero `SoundStats.started` on the **windowed** path. This is the only check that can see the composition roots `live_cmd.rs:22169-22173` records as unwitnessed.

### M138b — decode and the buffer library

> **SHIPPED IN FULL 2026-08-10** — the quantisation, the buffer library's
> caching rules, and the symphonia decode. It landed in two commits (the two
> pieces with an exact vanilla answer first, on a crate with no dependencies at
> all; then the decoder), which is why the log carries two entries.
>
> **Two claims in §2.8 stopped being citations and became measurements.**
> `item/goat_horn/call3.ogg` is 2-channel where `call0` is mono, and the store is
> mixed-rate (chicken 44100, horn 48000) — both now asserted against real assets,
> with the measured vectors committed and the audio left in the user's install.
>
> **The peak cannot see the quantisation.** Dropping the `-0.5` bias moves a
> real sound's peak by one (19988 -> 19987) and its SUM by 812 across 1728
> samples; the witness is on the sum. Found by a mutation surviving, not by
> reading. And the `channels == 0` guard is unreachable — symphonia's probe
> rejects every truncated ogg before a track exists — so that mutation is
> recorded as an expected survivor with the proof rather than left looking
> untested.
>
> Two things worth carrying forward. The **failure is cached too** —
> `computeIfAbsent` stores the future and an exceptionally-completed future stays
> in the map, so a missing file is not retried; retrying is the obvious design
> and is not vanilla's. And `buffer_size` records the constructor's latent
> asymmetry (the field rounds up to even, the *first* buffer allocates the raw
> argument) and **diverges from it on purpose**, because a one-byte-short first
> chunk cannot hold a 16-bit sample and vanilla never reaches the path.

New crate `rewo-audio` (deps: `rewo-net`, `rewo-data`, `symphonia`). **cpal not yet.** `rewo-net` must not gain a device dependency or 34 gates link an audio stack to decode a packet.

`decode.rs` (ogg → f32 → the `32767.5` quantisation → f32), `buffers.rs` transcribing `SoundBufferLibrary`: statics cached permanently by path, **streams never cached**, and **the loop flag honoured** per §0.2. Paths are asset-index **keys** (`<ns>/sounds/<path>.ogg`), resolved via the index to `<assets>/objects/<hash[0..2]>/<hash>` — a device treating the string as a filesystem path finds nothing.

**Gate:** `soundshot --check` layers (w)(s)(d).

### M138c — the mixer

> **SHIPPED 2026-08-10.** `mixer.rs` with `Mixer`, `Voice` and `NullSink`;
> layer (m)'s claims are unit witnesses reading the rendered output, and the
> `soundshot` command that would gather (w)(s)(a)(d)(m) under one
> `EXPECTED_WITNESSES` lock is still to build.
>
> **What the listener's up vector is for, measured.** `right = forward x up` is
> `(-cos yaw, 0, -sin yaw)` for EVERY pitch — pitching turns about the right
> axis and cannot move it — so almost every way of breaking the basis is
> invisible. The one that is not: pin `up` to `(0,1,0)` and at pitch 90 the
> forward vector is `(0,-1,0)`, making `forward x up` the ZERO vector and
> collapsing the stereo image to centre.
>
> **Two weak fixtures the battery caught**, both worth remembering: a
> rate-conversion witness whose sources were longer than its render window
> measured the window rather than the rate; and witnesses that render into a
> freshly-allocated (therefore zeroed) buffer cannot see a missing clear, which
> on a real device is unbounded accumulation.

`mixer.rs` + `sink/null.rs`. Pure; no device, no thread, no clock. Same shape as `alcRenderSamplesSOFT`: caller-driven.

**Gate:** `soundshot` layer (m).

### M138d — the device

> **SHIPPED 2026-08-10 — M138 is complete.** The SPSC ring, the mixer's
> `ChannelCall` interpreter, `cpal_sink.rs`, and `examples/listen.rs`. **The
> listening pass itself is still owed**, and it is a human's: run
> `cargo run -p rewo-audio --example listen`.
>
> **Two containments worth keeping.** cpal is NOT wired into `rewo-app`, so the
> client binary and all 34 gates do not link an audio stack for a subsystem none
> of them exercises; and no test opens a device, so `cargo test` stays silent.
>
> **One stated deviation.** `retire_finished` runs in the callback, so dropping a
> finished voice's last `Arc<Pcm>` deallocates there — the plan's design returns
> retired buffers to a worker over a second ring and this cut has none. The
> consequence is a possible dropout as a sound ends.
>
> **A voice is created SILENT and waits for its `Play`**, because
> `alSourcePlay` before an attach is a no-op and vanilla's order is properties,
> attach, play. And `AttachStaticBuffer` is counted-and-ignored at the callback
> rather than handled: it carries an asset key, and resolving one is a syscall
> plus a large allocation. The producer sends `Attach` with the PCM in hand,
> which is what vanilla's `thenAccept` continuation does too.
>
> **Memory-ordering mutations are deliberately not in the battery** — relaxing a
> `Release` is a real bug a test can only catch by luck, and a flaky battery
> entry is worse than none. The orderings are argued in the type's doc instead.
>
> **Two harness lessons, both of which cost a run.** A mutant that hangs takes
> the whole battery down, its `finally` never runs, and the mutation is left on
> disk — so a per-run timeout now makes a hang a KILL rather than an outage. And
> the hung test binary keeps holding the link output, so the *next* build fails
> with linker error 1104 and looks like a broken tree; the harness reaps strays.

`sink/cpal.rs` + `device.rs`. Four threads: render thread (one SPSC ring write per `ChannelCall` — a `play` emits exactly 8, so a busy tick starting 30 sounds is ~240 writes, bounded and countable); the cpal callback (no allocation, no lock, no syscall, **no drops** — retired buffers return to a worker on a channel); decode workers (`std::thread` + mpsc, the established pattern; CLAUDE.md forbids tokio/smol); a device watchdog.

**Ring full → drop and count, never block.** A full ring means the callback is not running, i.e. the device is dead, and stalling the renderer for a dead device is the wrong trade. **Buffer not ready → do not wait**, because vanilla already defers it (`SoundEngine.java:431-434`).

**Channel count: 30**, per §0.1 — which also makes the budget *bind*, so vanilla's drop-newest rule is reachable in ordinary play rather than needing a synthetic small count.

**Gate:** `--render-check` **r46** (non-zero mixed samples on the windowed path) + **the human listening pass** (§4).

### M139 — the loopback oracle
**Feasible, verified:** 26.2 pins `org.lwjgl:lwjgl-openal:3.4.1` **[read from `26.2.json`]** — and note both `3.3.3/` and `3.4.1/` are on disk **[read]**, which is how one survey graded the wrong jar. The 3.4.1 jar carries `SOFTLoopback`/`SOFTHRTF`/`SOFTOutputLimiter` and the shipped DLL is OpenAL Soft 1.25.1 exporting `alcLoopbackOpenDeviceSOFT` / `alcRenderSamplesSOFT` / `alcIsRenderFormatSupportedSOFT` **[concurring, two independent reads]**. A Java harness drives **vanilla's own** `Library`/`Listener`/`Channel` against a loopback device and dumps PCM — the M37 and M125 `tools/java_tostring_oracle/` precedent, checking in the **vectors** so no JVM is needed at gate time. It must pin `ALC_FORMAT_CHANNELS_SOFT`, `ALC_FORMAT_TYPE_SOFT`, frequency, `ALC_HRTF_SOFT`, `ALC_OUTPUT_LIMITER_SOFT` **and read `ALC_MONO_SOURCES` back** (the artefact that settles §0.1), or the vectors rot silently when the DLL's default output mode changes — and a checked-in vector file that stops matching looks like a Rewo regression.

### M140 — breadth

> **PARTLY SHIPPED 2026-08-10.** `level_event`'s sounds are wired — the block
> CENTRE (`Level.java:475`), the `data` gates, the per-row volume, and the
> global-flag match. **`MusicManager`'s gain ramp shipped too, as M140b**, and
> its finding corrects this section's own framing: `gainBySource` has exactly
> ONE writer in the client — the music crossfade — and not the options sliders,
> which arrive one factor earlier through `getFinalSoundSourceVolume`. **The ~8
> tickable per-tick ramps are still OPEN**, as is the rest of music: selection,
> the timers, and `getSituationalMusic`.
>
> **M141 took the ramps** (2026-08-11) — and there are **TEN** tick bodies, not
> the "~8" this section claims: `EntityBoundSoundInstance`, `BeeSoundInstance`,
> `BiomeAmbientSoundsHandler.LoopSoundInstance`, `DirectionalSoundInstance`,
> `ElytraOnPlayerSoundInstance`, `GuardianAttackSoundInstance`,
> `MinecartSoundInstance`, `RidingEntitySoundInstance`, `SnifferSoundInstance`,
> and the two nested in `UnderwaterAmbientSoundInstances`. They live in
> `crates/rewo-net/src/tickable.rs` and the engine drives them. **M141e built
> the first trigger** — the elytra — so one of the ten is constructed; the rest
> are `REWO_PLAN.md` §0.0 item 4a, one construction site at a time.
>
> M141e's finding belongs in §5's trap list: **`canPlaySound()` is a per-class
> override that six of the ten declare and four decline**, so a ramp's silence
> gate is not the entity it follows. Conflating them gives an elytra sound that
> a `/data`-silenced player silences, which vanilla does not do.
>
> **M141d then fed them their velocity**, which was the input gating four of the
> ten, and its finding belongs in this file's §5 trap list too: a remote
> entity's `getDeltaMovement()` is **a decaying echo of the last
> `set_entity_motion` packet**, not a velocity, because a client never
> integrates it into a position. A bee gliding steadily past with no motion
> packets has its buzz fade to silence. The decay is `0.98` a tick, is the
> **else** of the interpolation branch, is skipped for a vehicle the local
> player rides, ends in a deadband with two different forms, and **does not run
> for a minecart at all** (`aiStep` is `LivingEntity`'s). Four separate ways for
> a reasonable implementation to be wrong.
>
> Its headline belongs in this file too, because it is the kind of thing §5's
> trap list is for: **`MinecartSoundInstance` shadows
> `AbstractSoundInstance.pitch`**, and Java field access is statically bound, so
> vanilla's own pitch ramp writes a field `getPitch()` never reads. The
> plausible transcription gives every minecart ride a twenty-second pitch
> glissando that vanilla does not have. See `REWO_PLAN.md` §15.
>
> Camera- and listener-placed ids (four of seventy) are declined, because both
> need the camera and the decode layer does not have it; `distance_delay` is not
> modelled, so those sounds arrive early rather than late. And a structural fact
> found while writing the witness: **all three global rows are camera-placed**,
> so the sound path never fires for a global packet at all — asserted over the
> whole table, so adding a block-placed global row fails loudly.

`level_event_sounds` has **zero production callers** today; most block interactions (dispenser, anvil, composter) arrive as `level_event` ids, so a large fraction of world sound stays silent even with a perfect device. Plus the ~8 tickable per-tick ramps and `MusicManager`/`update_category_volume` (which is why `gainBySource` is pinned at 1.0).

---

## 4. The gate

**`rewo soundshot --check`** — serverless, CPU-only, fail-closed on an `EXPECTED_WITNESSES` lock, five layers mirroring `particleshot`. Every layer drives the production path; none rebuilds a slice of it.

- **(w) wire, 6.** The three packets + `level_event` as hand-assembled bodies through the real `route_sound`, ids resolved **by name** from the report. One witness drives a **numeric** registry id and asserts the resolved **name** — the only shape that can see M64's alphabetisation trap, where a positional table gives a different wrong name for each of 1,968 events with full round-trip success.
- **(s) resolution, 8.** The seeded variant pick (the wire carries the seed → `LegacyRandomSource` → the 48-bit LCG Rewo already ports bit-exactly), the redirect's **second** RNG draw, the missing-file weight shift. Built from an index produced by the **production loader**, failing closed on a missing store rather than degrading to an empty index.
- **(a) arithmetic and sequence, 10.** Through `RecordingDevice`: the exact eight-call order; master applied once; `max(volume,1.0)` using the **unclamped** volume for range while gain uses the clamped one; the zero-volume drop with its two escapes; `STARTED_SILENTLY` distinct; `MIN_SOURCE_LIFETIME` driven from both sides; budget exhaustion dropping the **newest**.
- **(d) decode, 6.** The quantisation against **literal vectors** (`32767.5` vs `32767`, truncation vs `round`, the `-0.5` bias) — the one part of the decode path with an exact vanilla answer, inaudible when wrong. Plus the granulepos end-trim as an exact sample count, channel count, rate.
- **(m) mixer, 12.** `NullSink` renders the **production** `Mixer`. Assertions read **out of the output**, never from `openal::linear_gain` recomputed in the test — M88's `r20` lesson, that a witness reading a value which merely *implies* the render is a proxy that looks more rigorous than it is. Linear ramp at several distances and **exactly zero at `max`** (the property inverse-square cannot have); L/R balance hard-left, hard-right, front; `AL_SOURCE_RELATIVE` keeping a UI sound centred as the listener walks away; pitch 0.5/2.0 changing output *length*; a **pitched** listener (a 3-float orientation bug is invisible until you look up); voice exhaustion; no clipping; underrun producing silence.

Plus `--render-check` r45/r46, and a mutation battery **with a no-op control that must SURVIVE** — M109's lesson, that a battery run against an already-failing command reads KILLED for every entry.

### What the gate does NOT assert

**A green `soundshot` is not evidence that this client makes any sound.** No gate opens a device; `NullSink` renders to memory, and the whole path from `CpalSink` through cpal's format negotiation, WASAPI and the speakers is ungraded — a client that mixes perfectly into a stream nobody opened passes every witness. **It does not assert that the mix matches vanilla**, and M139 does not close that: vanilla computes no pan and no gain curve at all (the complete surface is §2.3), so the panning law and the resampler belong to OpenAL Soft, and Rewo's equal-power pan and Catmull-Rom resampling are **stated approximations graded against Rewo's own declaration**; M139 turns "unknown" into "a measured divergence in dB", which is a number and not a zero. **Distance attenuation is graded against the OpenAL 1.1 specification, not against Minecraft** — `openal::linear_gain` (`sound_engine.rs:97`) has said so in its own doc since M131, and a witness asserting "gain at 8 blocks is 0.5" is grading a spec transcription. **The output limiter's curve is not matched** (vanilla's is OpenAL Soft's defaults, set nowhere in Java, existing only in the DLL), so Rewo diverges exactly on the dense scenes where it matters and no CPU-side gate can see it. **HRTF is not implemented**, so divergence is total when `directionalAudio` is on. **Vorbis decode is not bit-exact against jorbis and cannot be** — Vorbis I does not mandate identical float output between implementations, so it is graded to a stated tolerance with the bound in the witness's own detail string (M12's precedent, which graded `nextGaussian` to a ULP bound for the same reason). **Latency, glitching, underrun under real load and device hot-swap are unassertable**; underrun *counts* are, whether a human hears the glitch is not, and the callback's real-time discipline is enforced by construction and code review, **not by any test** — `NullSink` has no deadline, so it can never witness a missed one. **Timbre, stereo correctness as perceived, and whether the sound that plays is the sound the event meant are addressed by no machine check in this design.**

**Therefore the milestone requires an owned human listening step with a written outcome** — a named scene, a stated list of what to listen for (variant variety on repeated blocks, gain falling off with distance and cutting out at the radius, the stereo image tracking while turning, no click at clip ends, no glitching in a mob crowd, music not once-and-stopping), and a line in `REWO_PLAN.md` §15 recording that it was done and by whom. This paragraph belongs verbatim in the gate's own module doc, in the form `particleshot_cmd.rs:38-42` uses — because this project's own record is that prose next to a number goes stale while the number stays true, and a gate is what a future session reads. Without it, "verified" silently comes to mean "the gate was green", which for this subsystem is the one place in Rewo where that inference does not hold.

---

## 5. The traps

**A weak fixture where two readings agree.** Every fixture built from `Sound::file` has volume and pitch 1.0 — exactly where `instance.volume * sound.volume` and a *dropped multiplication* agree, which is the canonical device bug. Four of M131's witnesses were wrong and none of its code was. **Every new mixer witness must use a volume that is neither 0 nor 1 and a pitch that is neither 1 nor a power of two.** The channel count has the same shape and is **already solved in-tree**: `sound_engine.rs:1412-1423` **[read]** records that `sqrt(30) = 5.477` truncates *and* rounds to 5, so the default cannot witness the cast, and pins `pool_sizes(8).1 == 2` instead. Follow that file rather than re-deriving the hazard.

**A gate supplying an input production derives.** Live, but not where the draft said (§0.3). The two real ones: store-dependent tests **self-skip** on a bare machine, so a green run there proves nothing; and the two `drive` call sites are composition roots that **survive deletion against the whole suite** (`live_cmd.rs:22169-22173`). Both are closed by r45, not by a fixture change. The general sweep — a `*shot` gate reimplementing a slice of the app's setup (M45's `install_shapes`, M92's registry, M45's own `itemshot` glint) — still applies to the new gate: call the production `build_sounds`, not a hand-assembled index.

**An alphabetised registry.** `serde_json`'s default `Map` is a sorted `BTreeMap`, but ids 0..6 are the seven `entity.allay.*` events and `ambient.cave` is 7. Deriving an id from iteration position gives **a different wrong name for every one of 1,968 sounds**, with every round-trip still succeeding and no decode error anywhere — invisible to every gate, audible on the first sound played. `sound_events.rs` reads `protocol_id`; the (w) layer must drive a numeric id and assert the *name*.

**A comment describing what the code does not do.** Six documented instances (M93t's `setCanLoseFocus`, M96's fill note, `any_enchantments`' doc, M102, …). Two live in this subsystem right now: `entity_silent`'s doc is honest but its `false` is wrong-by-omission, and `sound_engine.rs:1204` **[concurring]** says three items "had zero production callers between them" when M131 wired two — half-stale, and it should be split rather than left to mislead.

**Two more, specific to audio.** Someone will "fix" the `32767.5` quantisation as a pointless precision loss in an f32 mixer; it needs a comment saying it is deliberate and a witness with literal vectors, or it will not survive the first cleanup pass. And **stereo is a per-variant decision, not per-event** — `item/goat_horn/call3.ogg` is 2ch while `call0-2` and `call4-7` are 1ch **[concurring]**, so a horn spatializes on seven rolls and not the eighth; OpenAL does not spatialize multi-channel buffers, so vanilla plays those non-positionally. A hand-written mixer naturally downmixes and spatializes uniformly, which is arguably *better* and is a divergence. **Choose explicitly and write it down**, rather than discovering it later as a mismatch. The store is also mixed-rate (44100 and 48000 inside one event family **[concurring]**), so the resampler is on the hot path for essentially every sound, not an edge case.

---

## 6. Deliberately out of scope for M138

- **HRTF.** `directionalAudio` will be unsupported and say so. A convolution with head-related impulse responses is its own project; vanilla only requests it when the device advertises specifiers (`Library.java:125-129`).
- **Doppler, reverb, cone angles, air absorption, `AL_MIN_GAIN`/`AL_MAX_GAIN`.** Vanilla sets none of these — §2.3 is the complete surface — so omitting them is **parity, not a shortfall**.
- **Matching OpenAL Soft's limiter curve.** M138 ships *a* look-ahead limiter so dense scenes do not hard-clip; the curve is a stated divergence.
- **Bit-exactness against jorbis.** Not achievable; graded to a stated tolerance.
- **`level_event` sounds, tickable per-tick ramps, and music** → M140. Each is independently gradeable and independent of the device; folding them in would make the device milestone judged on sounds it was never wired to play.
- **The loopback oracle** → M139. It needs something to grade.
- **Moving `sound_engine.rs` out of `rewo-net`.** It is client logic living in the protocol crate; relocating it is an M126-style down-crate migration with a conservation proof and zero functional gain. Recorded, not fixed.
- **`preload`.** Zero vanilla entries carry it **[concurring]**, so it is unreachable by any gate driven from vanilla data.

**Scope honesty:** M138a–d is a new crate, a decoder, a buffer/stream library, a mixer, two sinks, a device with four threads, a fifth trait method, a camera-basis transcription, a five-layer gate with ~42 witnesses, a mutation battery, and two false-green fixes. Splitting it into four independently-gated steps is what keeps its gate from being written last and its witnesses shaped to pass.