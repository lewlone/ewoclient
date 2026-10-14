//! Static decode on a worker thread (M156).
//!
//! # What vanilla actually does, which is not one thing
//!
//! The brief for this was "vanilla's `supplyAsync`", and that describes **half**
//! of it. `SoundBufferLibrary.getCompleteBuffer` wraps the whole decode of a
//! STATIC sound in `CompletableFuture.supplyAsync(.., Util.nonCriticalIoPool())`
//! (`SoundBufferLibrary.java:26-38`), and `getStream` wraps only the *opening*
//! of a stream — a file open and three Ogg header packets
//! (`JOrbisAudioStream.java:37-63`). Every streaming audio packet after that is
//! decoded in `Channel.pumpBuffers`, reached only from `ChannelHandle.execute`
//! and `ChannelAccess.scheduleTick`, **both of which post to a single daemon
//! thread**. So vanilla has two executors of different shapes and the streaming
//! one is not a pool at all.
//!
//! **This module is the static half only, and that is deliberate.** A single
//! unified worker pool would get the static side right and spread streaming
//! across N threads where vanilla serialises it on one — see the module doc of
//! [`crate::live_sink`] for where Rewo's streaming decode still runs and why
//! moving it is its own milestone.
//!
//! `nonCriticalIoPool()` is also **not** `ioPool()`: it returns `DOWNLOAD_POOL`
//! (`Util.java:261-263`), an *unbounded* `newCachedThreadPool` of daemon threads
//! that `shutdownExecutors` deliberately does not await. One worker here is a
//! stated deviation rather than a transcription — see [`DecodeWorker::spawn`].
//!
//! # The measurement this exists for
//!
//! The largest static decode in the 26.2 asset store is
//! `mob/enderdragon/end.ogg`: 251 KB of Ogg into 1,589,120 samples, measured at
//! **20.1 ms**. A client tick is 50 ms, so that is 40% of one tick spent
//! decoding on the thread that also runs physics and the render loop.
//!
//! The "11 MB" figure in this milestone's brief is `music.end`, which is
//! **streamed** — its inline cost is an open plus four one-second chunks, not
//! 11 MB of decode.
//!
//! **The "41 of 578" this paragraph used to give was wrong** (corrected M159).
//! Re-counted against the index 26.2 actually uses: **344 streamed variant
//! entries of 7,963**, or **98 distinct names of 4,843**. M143's `344 of 8,024`
//! was right all along — 8,024 counts the 61 `type: "event"` redirects M66
//! records and 7,963 excludes them.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use crate::buffers::{Pcm, PcmSource};

/// One decoded asset on its way back from the worker.
pub type Decoded = (String, Result<Arc<Pcm>, String>);

/// A worker thread that owns a [`PcmSource`] and decodes on request (M156).
///
/// **No async runtime**: `std::thread` plus `mpsc`, which is this project's
/// standing rule (`REWO_PLAN.md` §4) and is also what the shape wants — the
/// work items are large and few, which is the opposite of what a work-stealing
/// pool is for.
pub struct DecodeWorker {
    tx: Sender<String>,
    rx: Receiver<Decoded>,
    /// Keys submitted and not yet returned.
    ///
    /// **This is the in-flight dedup, and it is a transcription rather than an
    /// optimisation.** `SoundBufferLibrary`'s field is
    /// `Map<Identifier, CompletableFuture<SoundBuffer>>` and `computeIfAbsent`
    /// inserts the **future** (`:20`, `:27`), so a second caller for the same
    /// path while the first decode is still running receives that same
    /// in-flight future — vanilla dedupes in-flight decodes, not merely
    /// completed ones. It is invisible while a decode is synchronous, because
    /// "cache the result" and "cache the future" are then the same thing, and
    /// becomes load-bearing the moment it is not: without it, a block-break
    /// spamming its first `stone.break` decodes the same asset once per play.
    inflight: HashSet<String>,
}

impl DecodeWorker {
    /// Spawn the worker, handing it the source.
    ///
    /// **One thread rather than vanilla's unbounded pool**, and that is a
    /// stated deviation. Vanilla's `DOWNLOAD_POOL` will spawn a thread per
    /// concurrent decode; this serialises them. The justification is the
    /// measurement above — 20 ms for the largest asset in the store, and most
    /// are two orders smaller — against the cost of an unbounded pool on a
    /// client whose whole design premise is frame-time consistency. If a queue
    /// ever backs up audibly, the fix is more workers here, not a different
    /// shape.
    pub fn spawn<S>(mut source: S) -> DecodeWorker
    where
        S: PcmSource + Send + 'static,
    {
        let (tx, job_rx) = std::sync::mpsc::channel::<String>();
        let (res_tx, rx) = std::sync::mpsc::channel::<Decoded>();
        std::thread::Builder::new()
            .name("rewo-audio-decode".into())
            .spawn(move || {
                // Ends when the sender is dropped, which is the app shutting
                // down. A detached thread is fine: it holds no device and the
                // OS reclaims it.
                //
                // **A panicking decode is that job's failure, not the
                // thread's.** A dead worker would strand every channel parked
                // on a key it had accepted (see `drain` for the backstop).
                let mut warned = false;
                while let Ok(key) = job_rx.recv() {
                    let decoded = crate::buffers::contained(|| source.open(&key)).map(Arc::new);
                    if let Err(e) = &decoded {
                        if !warned && e.starts_with("the decoder panicked") {
                            warned = true;
                            log::warn!(
                                "audio: a static decode panicked ({key}); it counts as a failed \
                                 decode and the worker carries on"
                            );
                        }
                    }
                    if res_tx.send((key, decoded)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn audio decode worker");
        DecodeWorker {
            tx,
            rx,
            inflight: HashSet::new(),
        }
    }

    /// Ask for `key` unless it is already in flight.
    ///
    /// Returns whether the request was newly submitted, which is what lets a
    /// test tell dedup from a dropped job.
    pub fn request(&mut self, key: &str) -> bool {
        if self.inflight.contains(key) {
            return false;
        }
        if self.tx.send(key.to_string()).is_err() {
            // The worker is gone. Do NOT mark it in flight — a key recorded as
            // pending against a dead worker would never complete and would
            // wedge its channel silently.
            return false;
        }
        self.inflight.insert(key.to_string());
        true
    }

    /// Whether a decode for `key` is outstanding.
    pub fn is_inflight(&self, key: &str) -> bool {
        self.inflight.contains(key)
    }

    pub fn inflight_count(&self) -> usize {
        self.inflight.len()
    }

    /// Drain everything the worker has finished. Never blocks.
    ///
    /// **If the worker has gone, every key still in flight comes back as a
    /// failure.** Nothing else will ever answer them, and a channel parked on
    /// one would otherwise report "not stopped" for the rest of the session — a
    /// pool slot leaked per key.
    pub fn drain(&mut self) -> Vec<Decoded> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(d) => {
                    self.inflight.remove(&d.0);
                    out.push(d);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if !self.inflight.is_empty() {
                        log::warn!(
                            "audio: the decode worker exited with {} decode(s) in flight",
                            self.inflight.len()
                        );
                    }
                    for key in self.inflight.drain() {
                        out.push((key, Err("the decode worker exited".to_string())));
                    }
                    break;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Counting {
        opens: std::sync::Arc<std::sync::atomic::AtomicU32>,
        block: std::sync::Arc<std::sync::Mutex<()>>,
    }

    impl PcmSource for Counting {
        fn open(&mut self, key: &str) -> Result<Pcm, String> {
            // Held by the test while it submits duplicates, so every duplicate
            // arrives while the first decode is genuinely still running.
            let _g = self.block.lock().unwrap();
            self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if key == "bad" {
                return Err("no such asset".into());
            }
            Ok(Pcm {
                samples: vec![0i16; 8],
                channels: 1,
                sample_rate: 44100,
            })
        }
    }

    fn worker() -> (
        DecodeWorker,
        std::sync::Arc<std::sync::atomic::AtomicU32>,
        std::sync::Arc<std::sync::Mutex<()>>,
    ) {
        let opens = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let block = std::sync::Arc::new(std::sync::Mutex::new(()));
        let w = DecodeWorker::spawn(Counting {
            opens: opens.clone(),
            block: block.clone(),
        });
        (w, opens, block)
    }

    fn wait_for(w: &mut DecodeWorker, n: usize) -> Vec<Decoded> {
        let mut out = Vec::new();
        for _ in 0..2000 {
            out.extend(w.drain());
            if out.len() >= n {
                return out;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        out
    }

    #[test]
    fn a_decode_comes_back_off_the_calling_thread() {
        let (mut w, opens, _b) = worker();
        assert!(w.request("a"));
        let got = wait_for(&mut w, 1);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "a");
        assert!(got[0].1.is_ok());
        assert_eq!(opens.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// **The in-flight dedup, which is what `computeIfAbsent` on a map of
    /// FUTURES buys.**
    ///
    /// The source is held blocked while the duplicates are submitted, so every
    /// one of them genuinely arrives mid-decode. Without the dedup this opens
    /// five times; vanilla opens once and hands four callers the same future.
    #[test]
    fn a_second_request_while_the_first_is_in_flight_does_not_decode_twice() {
        let (mut w, opens, block) = worker();
        let held = block.lock().unwrap();
        assert!(w.request("a"), "the first is submitted");
        for _ in 0..4 {
            assert!(!w.request("a"), "a duplicate is refused while in flight");
        }
        assert_eq!(w.inflight_count(), 1);
        drop(held);
        let got = wait_for(&mut w, 1);
        assert_eq!(got.len(), 1, "one result, not five");
        assert_eq!(
            opens.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the source was opened once"
        );
        // ...and once it has landed the key is requestable again, because the
        // CACHE is the caller's job and this only dedupes what is outstanding.
        assert!(w.request("a"));
    }

    /// A failure comes back as a failure rather than as silence, and it clears
    /// the in-flight mark — otherwise a missing asset would wedge its key
    /// forever.
    #[test]
    fn a_failed_decode_returns_an_error_and_clears_the_flight_mark() {
        let (mut w, _o, _b) = worker();
        assert!(w.request("bad"));
        let got = wait_for(&mut w, 1);
        assert_eq!(got.len(), 1);
        assert!(got[0].1.is_err());
        assert_eq!(w.inflight_count(), 0);
        assert!(!w.is_inflight("bad"));
    }

    /// **A dead worker must not mark a key in flight.**
    ///
    /// If it did, the key would be recorded as pending against a thread that
    /// will never answer — so its channel would wait forever and the sound
    /// would never play, silently. The observable difference is that `request`
    /// answers false and nothing is left outstanding.
    #[test]
    fn a_request_to_a_dead_worker_is_refused_and_leaves_nothing_pending() {
        let (mut w, _o, _b) = worker();
        // Drop the worker thread's receiver by dropping our sender's peer:
        // closing the job channel is what a shut-down worker looks like.
        let dead = DecodeWorker {
            tx: {
                let (tx, rx) = std::sync::mpsc::channel::<String>();
                drop(rx);
                tx
            },
            rx: {
                let (_tx, rx) = std::sync::mpsc::channel::<Decoded>();
                rx
            },
            inflight: HashSet::new(),
        };
        let mut dead = dead;
        assert!(!dead.request("a"), "a dead worker refuses the request");
        assert_eq!(
            dead.inflight_count(),
            0,
            "and leaves NOTHING pending — a key marked in flight against a dead              worker would wedge its channel forever"
        );
        assert!(!dead.is_inflight("a"));
        // The live worker still works, so this is not a broken fixture.
        assert!(w.request("a"));
    }

    /// Draining is non-blocking: an empty queue answers immediately with
    /// nothing, which is what lets the tick call it unconditionally.
    #[test]
    fn draining_an_idle_worker_yields_nothing_and_does_not_block() {
        let (mut w, _o, _b) = worker();
        assert!(w.drain().is_empty());
        assert!(w.drain().is_empty());
    }

    struct Panicky;

    impl PcmSource for Panicky {
        fn open(&mut self, key: &str) -> Result<Pcm, String> {
            if key == "boom" {
                panic!("injected decoder panic");
            }
            Ok(Pcm {
                samples: vec![0i16; 8],
                channels: 1,
                sample_rate: 44100,
            })
        }
    }

    /// **A panicking decode is that job's failure, and the worker carries on.**
    /// Before, the panic killed the thread and every later request was parked
    /// against a worker that would never answer.
    #[test]
    fn a_panicking_decode_is_a_failure_and_the_worker_survives() {
        let mut w = DecodeWorker::spawn(Panicky);
        assert!(w.request("boom"));
        let got = wait_for(&mut w, 1);
        let (key, result) = &got[0];
        assert_eq!(key, "boom");
        let err = result.as_ref().expect_err("a panic is a failed decode");
        assert!(
            err.contains("panicked") && err.contains("injected"),
            "{err}"
        );
        assert_eq!(w.inflight_count(), 0);

        assert!(w.request("fine"), "the worker is still accepting jobs");
        let got = wait_for(&mut w, 1);
        assert!(got[0].1.is_ok(), "and still decoding");
    }

    /// **The backstop: a worker that exits with keys in flight answers them all
    /// as failures**, so channels parked on them are reclaimed rather than
    /// waiting on a thread that no longer exists.
    #[test]
    fn a_worker_that_exits_fails_everything_in_flight() {
        let (tx, job_rx) = std::sync::mpsc::channel::<String>();
        let (res_tx, rx) = std::sync::mpsc::channel::<Decoded>();
        // A worker that takes one job and dies without answering it.
        let t = std::thread::spawn(move || {
            let _ = job_rx.recv();
            drop(res_tx);
        });
        let mut w = DecodeWorker {
            tx,
            rx,
            inflight: HashSet::new(),
        };
        assert!(w.request("a"));
        t.join().unwrap();
        // The job channel is gone too, so a new request is refused rather than
        // parked.
        assert!(!w.request("b"));
        let got = w.drain();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "a");
        assert!(got[0].1.is_err());
        assert_eq!(w.inflight_count(), 0, "nothing left waiting");
    }
}
