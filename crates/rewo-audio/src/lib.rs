//! Rewo's audio crate — M138b through M144.
//!
//! **Only `cpal_sink` opens a device; nothing else makes a noise.** The plan
//! ([`REWO_AUDIO_PLAN.md`](../../../REWO_AUDIO_PLAN.md)) split audio into four
//! steps so each ships and is graded on its own; all four have shipped:
//! - [`quantise`] + [`buffers`] — the parts with an **exact** vanilla answer
//!   (`32767.5/-0.5/truncate-toward-zero` PCM conversion; the buffer library's
//!   caching rules).
//! - [`mixer`] — caller-driven `Mixer::render` in the `alcRenderSamplesSOFT`
//!   shape: the attenuation curve is a transcription (OpenAL 1.1 linear), the
//!   pan law and resampler are stated approximations, HRTF is absent.
//! - [`device`] + [`live_sink`] — the SPSC command ring (a full ring drops the
//!   NEWEST command and never blocks) and the channel pools over it.
//! - [`cpal_sink`] — the device binding. Deliberately the only noise-making
//!   code, reached from `rewo-app` under its off-by-default `audio` feature,
//!   and the one thing no gate can grade (see below).
//!
//! Dependencies are directional on purpose: this crate may depend on
//! `rewo-net` (for `ListenerTransform` and the gain curve), but `rewo-net`
//! must never depend back — a default build of every other crate links no
//! audio stack, which is what keeps the gates free of cpal/symphonia.
//!
//! ## What this is NOT evidence of
//!
//! Green tests here say the arithmetic and the caching match the decompile. They
//! say nothing about whether Rewo will make a correct noise, or any noise — no
//! gate in this project opens an audio device, and the plan's §4 records at
//! length what that means and why the milestone ends with a human listening
//! step. Do not let "the audio tests pass" come to mean "audio works"; for this
//! subsystem, that inference is the one place in Rewo where it does not hold.

pub mod buffers;
pub mod cpal_sink;
pub mod decode;
pub mod decode_worker;
pub mod device;
pub mod live_sink;
pub mod mixer;
pub mod quantise;
pub mod stream_worker;
