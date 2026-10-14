//! Streaming decode on the sound-engine thread (M159).
//!
//! M156 moved the STATIC decode to a worker and deliberately left this half,
//! recording why: vanilla has **two executors of different shapes** and only one
//! of them is a pool.
//!
//! ```java
//! this.soundBuffers.getStream(sound.getPath(), isLooping)      // supplyAsync, nonCriticalIoPool
//!     .thenAccept(stream -> handle.execute(channel -> {        // -> the SOUND ENGINE thread
//!         channel.attachBufferStream(stream);                  //    pumpBuffers(4)
//!         channel.play();
//!     }));
//! ```
//! (`SoundEngine.java:436-439`.) And every refill after that is
//! `ChannelAccess.scheduleTick` -> `channel.updateStream()` -> `pumpBuffers`
//! (`ChannelAccess.java:44-56`, `Channel.java:151-156`), posted to the same
//! `SoundEngineExecutor` — **one daemon thread named "Sound engine"**, not a
//! pool. So the open is parallel and every read is serialised.
//!
//! # What this cost, measured before it was moved
//!
//! M156's entry says its brief had no number and that this mattered. Measured
//! over all **98 streamed variants** in the 26.2 store, warm:
//!
//! | | mean | worst |
//! |---|---|---|
//! | open (file read + Ogg headers) | 2.3 ms | 3.3 ms |
//! | prime — the four `attachBufferStream` pumps | 2.5 ms | 6.3 ms |
//! | one steady-state refill | 0.6 ms | 0.9 ms |
//!
//! Cold, the open is **9.4 ms** mean and the first-touch buffer 2.5 ms, because
//! it is dominated by reading the file; every music track is a first touch.
//!
//! **So this is a much smaller cost than M156's, and saying so is the point.**
//! The largest static decode was 20.1 ms against a 50 ms tick; here the
//! expensive event is *starting* a stream — open plus prime, ~4.7 ms warm and
//! ~12 ms cold, in one hitch — while the steady state is 0.6 ms per stream per
//! second. Both are moved because vanilla runs neither on the client thread, not
//! because either was threatening a frame.
//!
//! **The briefed shape was "move the refills"**, and the measurement says the
//! refills are the cheap half. The start is where the milliseconds are, so the
//! open moved too.
//!
//! # Three things this cannot copy, each for a stated reason
//!
//! **The open runs on this same single thread, where vanilla uses a pool.** That
//! serialises opens vanilla would parallelise. The justification is the
//! measurement — 2.3 ms warm, and concurrent stream starts are rare (a music
//! track and an ambient bed) — plus what it buys: the `PcmStream` is created and
//! read on one thread and **never crosses a boundary after that**, which is
//! `Channel.stream`'s own lifetime. M156 made the same deviation for the same
//! kind of reason.
//!
//! **Decoded chunks go back to [`crate::live_sink::LiveSink`] rather than
//! straight to the device.** Vanilla's sound-engine thread calls
//! `alSourceQueueBuffers` itself; this thread cannot, because
//! [`crate::device::CommandProducer`] is **single-producer by construction** — that
//! is the whole of its safety argument — and `LiveSink` is the producer. So a
//! chunk waits for the next tick to be pushed. With four seconds queued and a
//! 50 ms tick that is 1.25% of the slack.
//!
//! **A stream carries an EPOCH and a static decode does not.**
//! `ChannelState::pending` (M156) documents that it needs none, because
//! `release` removes the whole state and a channel re-acquired for the same key
//! wants that same buffer anyway. **That argument does not extend here**: a
//! stream chunk is a *position*, not an asset. A channel released and
//! re-acquired for the same key starts a new stream at position 0, and a late
//! chunk from the old one would splice the middle of a track into its beginning.
//! The epoch is what makes a late chunk droppable.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use crate::buffers::{Pcm, PcmSource, PcmStream};
use rewo_net::sound_engine::ChannelId;

/// A stream's identity across a thread boundary.
///
/// The channel alone is not enough — see the module doc's third point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StreamKey {
    pub channel: ChannelId,
    pub epoch: u64,
}

/// What [`LiveSink`](crate::live_sink::LiveSink) asks the worker to do.
pub enum StreamRequest {
    /// `SoundBufferLibrary.getStream` — open, and report the format.
    Open {
        key: StreamKey,
        asset: String,
        looping: bool,
    },
    /// `Channel.pumpBuffers(n)` — read up to `buffers` more chunks.
    Pump { key: StreamKey, buffers: usize },
    /// `Channel.destroy`'s `this.stream.close()`. Idempotent.
    Close { key: StreamKey },
}

/// What comes back.
///
/// Every variant carries the [`StreamKey`] so a landing for a stream that has
/// since been replaced is droppable at the seam rather than by inspection.
pub enum StreamEvent {
    /// The format, which `attachBufferStream` reads *before* it pumps anything
    /// (`Channel.java:125-127`).
    Opened {
        key: StreamKey,
        channels: u16,
        rate: u32,
    },
    OpenFailed {
        key: StreamKey,
        error: String,
    },
    /// One `alSourceQueueBuffers` worth.
    Chunk {
        key: StreamKey,
        pcm: Arc<Pcm>,
    },
    /// The stream ran out, or a read failed.
    ///
    /// **Never sent for a looping stream**, because `LoopingAudioStream`
    /// restarts rather than returning empty — the property that keeps an ambient
    /// bed alive with no special case on either side.
    Ended {
        key: StreamKey,
    },
}

/// The single "Sound engine" thread, as far as streaming is concerned.
pub struct StreamWorker {
    tx: Sender<StreamRequest>,
    rx: Receiver<StreamEvent>,
    /// Chunks asked for and not yet returned, per live stream.
    ///
    /// **Held on this side because the queue invariant is enforced here.** The
    /// worker reads what it is told to; deciding *how many* is
    /// `updateStream`'s job and stays with the clock that computes `processed`.
    inflight: HashMap<StreamKey, usize>,
    /// Every stream asked to open and not yet ended, failed or closed, and
    /// whether its `Opened` has landed.
    ///
    /// **The backstop for a worker that dies.** Without it a channel waiting on
    /// an open, or on a stream that will now never end, reports "not stopped"
    /// for the rest of the session and its pool slot leaks; `drain` answers
    /// every live key with `OpenFailed` or `Ended` instead.
    live: HashMap<StreamKey, bool>,
}

impl StreamWorker {
    /// Spawn the worker, handing it the source it opens streams from.
    pub fn spawn<S>(mut source: S) -> StreamWorker
    where
        S: PcmSource + Send + 'static,
    {
        let (tx, req_rx) = std::sync::mpsc::channel::<StreamRequest>();
        let (ev_tx, rx) = std::sync::mpsc::channel::<StreamEvent>();
        std::thread::Builder::new()
            // Vanilla's is `new Thread(this::run, "Sound engine")`
            // (`SoundEngineExecutor.java:17`).
            .name("rewo-audio-stream".into())
            .spawn(move || {
                // The open streams, owned here for their whole lives.
                let mut open: HashMap<StreamKey, (Box<dyn PcmStream>, usize)> = HashMap::new();
                // Ends when the sender drops, i.e. the app is shutting down.
                // Daemon-equivalent: it holds no device, and vanilla's thread is
                // `setDaemon(true)` for the same reason.
                while let Ok(req) = req_rx.recv() {
                    let sent = match req {
                        StreamRequest::Open {
                            key,
                            asset,
                            looping,
                        } => match crate::buffers::contained(|| {
                            // The format read is inside the guard too: it is
                            // the decoder's first look at the headers.
                            let st = source.open_stream(&asset, looping)?;
                            let format = st.format();
                            Ok((st, format))
                        }) {
                            Ok((st, (channels, rate))) => {
                                let per = crate::buffers::calculate_buffer_size(
                                    channels,
                                    rate,
                                    crate::buffers::BUFFER_DURATION_SECONDS,
                                ) / 2;
                                open.insert(key, (st, per.max(1)));
                                ev_tx.send(StreamEvent::Opened {
                                    key,
                                    channels,
                                    rate,
                                })
                            }
                            Err(e) => ev_tx.send(StreamEvent::OpenFailed { key, error: e }),
                        },
                        StreamRequest::Pump { key, buffers } => {
                            let mut result = Ok(());
                            let mut finished = false;
                            if let Some((st, per)) = open.get_mut(&key) {
                                for _ in 0..buffers {
                                    // A panicking read ends this stream (the
                                    // `Err` arm), not the thread.
                                    match crate::buffers::contained(|| st.read(*per)) {
                                        // Empty is exhaustion. A looping stream
                                        // never gets here.
                                        Ok(c) if c.is_empty() => {
                                            finished = true;
                                            break;
                                        }
                                        Ok(samples) => {
                                            let pcm = Arc::new(Pcm {
                                                samples,
                                                channels: st.format().0,
                                                sample_rate: st.format().1,
                                            });
                                            result = ev_tx.send(StreamEvent::Chunk { key, pcm });
                                            if result.is_err() {
                                                break;
                                            }
                                        }
                                        Err(e) => {
                                            log::warn!(
                                                "audio: stream read failed on channel {}: {e}",
                                                key.channel
                                            );
                                            finished = true;
                                            break;
                                        }
                                    }
                                }
                            }
                            if finished {
                                open.remove(&key);
                                result = result.and(ev_tx.send(StreamEvent::Ended { key }));
                            }
                            result
                        }
                        StreamRequest::Close { key } => {
                            open.remove(&key);
                            Ok(())
                        }
                    };
                    if sent.is_err() {
                        break;
                    }
                }
            })
            .expect("spawn audio stream worker");
        StreamWorker {
            tx,
            rx,
            inflight: HashMap::new(),
            live: HashMap::new(),
        }
    }

    /// `getStream` — ask for a stream to be opened.
    pub fn open(&mut self, key: StreamKey, asset: &str, looping: bool) -> bool {
        let sent = self
            .tx
            .send(StreamRequest::Open {
                key,
                asset: asset.to_string(),
                looping,
            })
            .is_ok();
        if sent {
            self.live.insert(key, false);
        }
        sent
    }

    /// `pumpBuffers(n)`. Records the request so the queue invariant can count
    /// what is in flight.
    pub fn pump(&mut self, key: StreamKey, buffers: usize) -> bool {
        if buffers == 0 {
            return false;
        }
        if self.tx.send(StreamRequest::Pump { key, buffers }).is_err() {
            // Do NOT record it: a request against a dead worker would never
            // land, and the channel would sit forever believing its queue was
            // full. M156's `request` declines the same way and for the same
            // reason.
            return false;
        }
        *self.inflight.entry(key).or_insert(0) += buffers;
        true
    }

    /// Chunks asked for and not yet returned.
    pub fn inflight(&self, key: StreamKey) -> usize {
        self.inflight.get(&key).copied().unwrap_or(0)
    }

    /// Forget a stream: the worker drops it and its in-flight count goes.
    pub fn close(&mut self, key: StreamKey) {
        self.inflight.remove(&key);
        self.live.remove(&key);
        let _ = self.tx.send(StreamRequest::Close { key });
    }

    /// Everything the worker has finished. Never blocks.
    ///
    /// **If the worker has gone**, every live stream is answered here — an
    /// open that never landed with `OpenFailed`, an opened stream with `Ended`
    /// — so no channel waits on a thread that no longer exists.
    pub fn drain(&mut self) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        loop {
            let ev = match self.rx.try_recv() {
                Ok(ev) => ev,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if !self.live.is_empty() {
                        log::warn!(
                            "audio: the stream worker exited with {} stream(s) live",
                            self.live.len()
                        );
                    }
                    self.inflight.clear();
                    for (key, opened) in self.live.drain() {
                        out.push(if opened {
                            StreamEvent::Ended { key }
                        } else {
                            StreamEvent::OpenFailed {
                                key,
                                error: "the stream worker exited".to_string(),
                            }
                        });
                    }
                    break;
                }
            };
            match &ev {
                StreamEvent::Chunk { key, .. } => {
                    if let Some(n) = self.inflight.get_mut(key) {
                        *n = n.saturating_sub(1);
                    }
                }
                // A stream that has ended will never answer its remaining
                // requests, so they are not in flight any more — leaving them
                // counted would make a re-used key look permanently busy.
                StreamEvent::Ended { key } | StreamEvent::OpenFailed { key, .. } => {
                    self.inflight.remove(key);
                    self.live.remove(key);
                }
                StreamEvent::Opened { key, .. } => {
                    // Not re-added if it was closed in the meantime.
                    if let Some(opened) = self.live.get_mut(key) {
                        *opened = true;
                    }
                }
            }
            out.push(ev);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: StreamKey = StreamKey {
        channel: 1,
        epoch: 1,
    };
    const B: StreamKey = StreamKey {
        channel: 2,
        epoch: 2,
    };

    /// A worker body the test scripts: answer `Opened` for the first open and
    /// then die, leaving the second open unanswered.
    fn dying_worker() -> (StreamWorker, std::thread::JoinHandle<()>) {
        let (tx, req_rx) = std::sync::mpsc::channel::<StreamRequest>();
        let (ev_tx, rx) = std::sync::mpsc::channel::<StreamEvent>();
        let t = std::thread::spawn(move || {
            if let Ok(StreamRequest::Open { key, .. }) = req_rx.recv() {
                let _ = ev_tx.send(StreamEvent::Opened {
                    key,
                    channels: 1,
                    rate: 44_100,
                });
            }
            let _ = req_rx.recv();
        });
        let w = StreamWorker {
            tx,
            rx,
            inflight: HashMap::new(),
            live: HashMap::new(),
        };
        (w, t)
    }

    /// **A worker that exits answers every live stream**: the one that opened
    /// ends, the one that never opened fails — so neither channel waits on a
    /// thread that is gone.
    #[test]
    fn a_dead_worker_ends_its_streams_and_fails_its_opens() {
        let (mut w, t) = dying_worker();
        assert!(w.open(A, "a", false));
        assert!(w.open(B, "b", false));
        t.join().unwrap();
        let events = w.drain();
        let opened = events
            .iter()
            .any(|e| matches!(e, StreamEvent::Opened { key, .. } if *key == A));
        let ended = events
            .iter()
            .any(|e| matches!(e, StreamEvent::Ended { key } if *key == A));
        let failed = events
            .iter()
            .any(|e| matches!(e, StreamEvent::OpenFailed { key, .. } if *key == B));
        assert!(opened && ended && failed, "{} event(s)", events.len());
        assert!(w.live.is_empty());
        // A closed stream is not resurrected by the backstop.
        assert!(w.drain().is_empty());
    }

    /// A closed stream is forgotten, so a dead worker does not answer it.
    #[test]
    fn a_closed_stream_is_not_answered_by_the_backstop() {
        let (mut w, t) = dying_worker();
        assert!(w.open(A, "a", false));
        w.close(A);
        t.join().unwrap();
        let events = w.drain();
        assert!(
            !events.iter().any(|e| matches!(
                e,
                StreamEvent::Ended { .. } | StreamEvent::OpenFailed { .. }
            )),
            "a closed stream was answered"
        );
    }
}
