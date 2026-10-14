//! The reader thread and the queue it feeds, bounded in **bytes** as well as
//! frames.
//!
//! The play-phase reader thread decodes frames into a `sync_channel` of
//! [`INBOUND_QUEUE_FRAMES`] slots. A slot count alone does not bound memory:
//! a frame inflates to up to `MAX_UNCOMPRESSED` (8 MiB), so 4096 queued
//! frames is a theoretical 32 GiB if the consumer stalls while the server
//! keeps sending. The [`ByteBudget`] caps the decompressed bytes sitting in
//! the queue at [`INBOUND_QUEUE_BYTES`].
//!
//! **Backpressure, never a drop.** When the budget is spent the reader thread
//! blocks before queueing the next frame, stops reading the socket, and the
//! kernel's receive window closes on the server: the same flow control that
//! stalls vanilla's Netty pipeline when its event loop falls behind. Dropping
//! a frame instead would desynchronise the session (a lost `level_chunk`,
//! a missed `remove_entities`), so it is not an option. If the stall lasts,
//! the server's own keep-alive timeout ends the connection, which the
//! session then reports as an ordinary disconnect.
//!
//! [`INBOUND_QUEUE_FRAMES`]: super::INBOUND_QUEUE_FRAMES

use std::io::Read;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Condvar, Mutex};

use rewo_proto::frame::FrameCodec;

use super::ReaderClosed;

/// Decompressed frame bytes the reader thread may hold queued ahead of the
/// consumer. 64 MiB is eight maximum-size frames, and far above anything a
/// vanilla server sends between two frames of a client that is keeping up
/// (a full view-distance chunk burst is a few MiB).
pub const INBOUND_QUEUE_BYTES: usize = 64 << 20;

/// A byte count shared by the reader thread (which reserves before queueing)
/// and the session (which releases after taking a frame off the queue).
pub(crate) struct ByteBudget {
    limit: usize,
    state: Mutex<BudgetState>,
    cv: Condvar,
}

#[derive(Default)]
struct BudgetState {
    queued: usize,
    /// The consumer is gone; a waiting reader must give up.
    closed: bool,
}

impl ByteBudget {
    pub(crate) fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            state: Mutex::new(BudgetState::default()),
            cv: Condvar::new(),
        })
    }

    /// Reserve `n` bytes, blocking while they would take the queue past the
    /// limit. A frame larger than the whole budget is admitted once the queue
    /// is empty, so no frame can wait forever. Returns `false` if the budget
    /// was closed (the consumer is gone) instead.
    pub(crate) fn acquire(&self, n: usize) -> bool {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        while !s.closed && s.queued > 0 && s.queued.saturating_add(n) > self.limit {
            s = self.cv.wait(s).unwrap_or_else(|p| p.into_inner());
        }
        if s.closed {
            return false;
        }
        s.queued += n;
        true
    }

    /// Give back `n` bytes the consumer has taken off the queue.
    pub(crate) fn release(&self, n: usize) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        s.queued = s.queued.saturating_sub(n);
        drop(s);
        self.cv.notify_all();
    }

    /// The consumer is gone: wake and stop a reader waiting for room.
    pub(crate) fn close(&self) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        s.closed = true;
        drop(s);
        self.cv.notify_all();
    }

    /// Bytes currently reserved by queued frames.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).queued
    }
}

/// The reader thread's loop: decode a frame, reserve its bytes, queue it.
/// Ends on a read error (sent as the session's disconnect reason), or when
/// the session has gone away (channel closed or budget closed).
pub(crate) fn run_reader(
    codec: FrameCodec,
    mut stream: impl Read,
    tx: SyncSender<Result<Vec<u8>, ReaderClosed>>,
    budget: Arc<ByteBudget>,
) {
    let mut scratch = Vec::new();
    loop {
        let mut packet = Vec::new();
        if let Err(e) = codec.read_frame(&mut stream, &mut scratch, &mut packet) {
            // The reason travels to the session, which turns it into the
            // disconnect; the channel then drops. It carries no frame bytes,
            // so it needs no reservation.
            let _ = tx.send(Err(super::reader_closed(e)));
            return;
        }
        let n = packet.len();
        if !budget.acquire(n) {
            return;
        }
        if tx.send(Ok(packet)).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{sync_channel, TryRecvError};
    use std::time::{Duration, Instant};

    /// `count` uncompressed frames, each a packet of `size` bytes whose first
    /// byte is its index (so order is checkable).
    fn wire(count: usize, size: usize) -> Vec<u8> {
        let codec = FrameCodec::default();
        let mut out = Vec::new();
        for i in 0..count {
            let mut packet = vec![0xAB; size];
            packet[0] = i as u8;
            codec.write_frame(&mut out, &packet).unwrap();
        }
        out
    }

    /// Wait until `queued` stops changing, i.e. the reader has blocked.
    fn settle(budget: &ByteBudget) -> usize {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut last = budget.queued();
        loop {
            std::thread::sleep(Duration::from_millis(50));
            let now = budget.queued();
            if now == last || Instant::now() > deadline {
                return now;
            }
            last = now;
        }
    }

    #[test]
    fn a_stalled_consumer_blocks_the_reader_at_the_byte_budget_and_loses_nothing() {
        const FRAMES: usize = 20;
        const SIZE: usize = 1000;
        // Room for three frames' bytes, while the channel itself would take
        // all twenty: the byte budget, not the slot count, is what binds.
        let budget = ByteBudget::new(3 * SIZE);
        let (tx, rx) = sync_channel(64);
        let reader = {
            let budget = budget.clone();
            let stream = std::io::Cursor::new(wire(FRAMES, SIZE));
            std::thread::spawn(move || run_reader(FrameCodec::default(), stream, tx, budget))
        };

        // The consumer is stalled: the reader fills the budget and stops.
        let queued = settle(&budget);
        assert_eq!(queued, 3 * SIZE, "the reader must stop at the budget");
        assert!(!reader.is_finished(), "the reader must block, not give up");

        // Drain like `drain_inbound`: release each frame's bytes as it is
        // taken. Every frame arrives, in order, then the end of stream.
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match rx.try_recv() {
                Ok(Ok(p)) => {
                    assert!(budget.queued() <= 3 * SIZE);
                    budget.release(p.len());
                    seen.push(p[0] as usize);
                }
                Ok(Err(closed)) => {
                    assert!(closed.eof, "a clean end of stream: {}", closed.reason);
                    break;
                }
                Err(TryRecvError::Empty) => {
                    assert!(Instant::now() < deadline, "reader stalled for good");
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(TryRecvError::Disconnected) => panic!("no end-of-stream reason"),
            }
        }
        assert_eq!(seen, (0..FRAMES).collect::<Vec<_>>());
        assert_eq!(budget.queued(), 0);
        reader.join().unwrap();
    }

    #[test]
    fn a_frame_larger_than_the_budget_is_admitted_alone() {
        let budget = ByteBudget::new(100);
        let (tx, rx) = sync_channel(8);
        let stream = std::io::Cursor::new(wire(2, 500));
        let reader = {
            let budget = budget.clone();
            std::thread::spawn(move || run_reader(FrameCodec::default(), stream, tx, budget))
        };
        // The first frame goes in on an empty queue; the second waits for it.
        assert_eq!(settle(&budget), 500);
        let first = rx.recv().unwrap().unwrap();
        budget.release(first.len());
        let second = rx.recv().unwrap().unwrap();
        budget.release(second.len());
        assert_eq!((first[0], second[0]), (0, 1));
        assert!(rx.recv().unwrap().is_err(), "then the end of stream");
        reader.join().unwrap();
    }

    #[test]
    fn closing_the_budget_releases_a_blocked_reader() {
        let budget = ByteBudget::new(10);
        let (tx, _rx) = sync_channel(8);
        let stream = std::io::Cursor::new(wire(5, 10));
        let reader = {
            let budget = budget.clone();
            std::thread::spawn(move || run_reader(FrameCodec::default(), stream, tx, budget))
        };
        assert_eq!(settle(&budget), 10);
        assert!(!reader.is_finished());
        // What `PlaySession::drop` does.
        budget.close();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !reader.is_finished() {
            assert!(Instant::now() < deadline, "the reader never woke");
            std::thread::sleep(Duration::from_millis(5));
        }
        reader.join().unwrap();
    }
}
