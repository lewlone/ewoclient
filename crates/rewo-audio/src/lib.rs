//! Rewo's audio crate — M138b.
//!
//! **Nothing here opens a device or makes a noise.** The plan
//! ([`REWO_AUDIO_PLAN.md`](../../../REWO_AUDIO_PLAN.md)) splits audio into four
//! steps so each ships and is graded on its own; this crate is where the second
//! and third live, and the device is the fourth. What is here now is the part of
//! the decode path with an **exact** vanilla answer, plus the buffer library's
//! caching rules — both of which are pure logic and need no decoder, which is
//! why they are in before one.
//!
//! **It deliberately has no dependencies yet.** The plan lists `rewo-net`,
//! `rewo-data` and `symphonia`; none of them is needed by what is here, and
//! adding them early would put a build cost on 34 gates that decode packets and
//! render frames without ever wanting audio. `rewo-net` in particular must never
//! gain a device dependency.
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
pub mod device;
pub mod mixer;
pub mod quantise;
