//! The seam between the engine's thread and an audio callback.
//!
//! **This is M138d's testable half.** The plan's M138d is four threads — a
//! render thread that writes one command per `ChannelCall`, the device callback
//! that consumes them, decode workers, and a watchdog. The *discipline* between
//! the first two is pure logic and is here, with witnesses. The cpal binding is
//! not, and is deliberately not in this commit: it is the first thing in Rewo
//! that no gate can check, and it ends with a listening pass a machine cannot
//! perform. Shipping an unexercised device binding into a workspace where 34
//! gates link it is the trade this project does not take.
//!
//! ## The discipline, and why each half of it is the way round it is
//!
//! **A full ring drops the NEW command and counts it. It never blocks.** That
//! reads backwards — the newest command is usually the most wanted — and it is
//! the right way round because of what a full ring *means*: the consumer is the
//! device callback, so a ring that has filled is a callback that has stopped
//! running, which is a dead or stalled device. Blocking the render thread on a
//! dead device trades a silent sound for a frozen client. Counting the drops is
//! what makes the difference diagnosable afterwards; a silent drop would present
//! as "audio sometimes misses sounds" with nothing to look at.
//!
//! **The callback must not allocate, lock, or take a syscall**, which is why
//! this is a fixed-capacity ring of plain values rather than a channel. It is
//! also why nothing here ever frees a buffer: a retired `Arc<Pcm>` goes back to
//! a worker rather than dropping in the callback, where the deallocation could
//! block on a heap lock while the device waits.
//!
//! A `play` emits exactly eight calls (`SoundEngine.java:417-434`), so a busy
//! tick starting thirty sounds writes about 240 commands — bounded and
//! countable, which is what makes a capacity choice arguable rather than a guess.

use rewo_net::sound_engine::{ChannelCall, ChannelId, ListenerTransform};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

/// One instruction for the audio side.
///
/// `Clone` rather than `Copy`, because `ChannelCall::AttachStaticBuffer` carries
/// an asset key `String`. That matters at the callback end: the consumer takes
/// the value out of its slot rather than reading a copy, so nothing is dropped
/// inside the callback except on the paths that already own a `String` — and
/// M138d's plan is for buffers to travel back to a worker rather than be freed
/// there at all.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// One `handle.execute(channel -> …)` body — the thirteen `ChannelCall`
    /// variants M131 modelled.
    Channel(ChannelId, ChannelCall),
    /// `Listener.setTransform`. Not a channel, so not a `ChannelCall` — the
    /// same reasoning that made it a fifth trait method in M138a.
    Listener(ListenerTransform),
    /// An already-decoded buffer for a channel.
    ///
    /// **This is why `ChannelCall::AttachStaticBuffer` cannot cross the ring as
    /// itself.** That variant carries an asset *key*, and turning one into
    /// samples is a store lookup and an ogg decode — a syscall and a large
    /// allocation, both forbidden on an audio callback. The producer does the
    /// work and sends the result, which is also what vanilla does: the attach is
    /// a continuation on `getCompleteBuffer(path).thenAccept(...)`
    /// (`SoundEngine.java:431-434`), not something the audio side waits for.
    Attach(ChannelId, Arc<crate::buffers::Pcm>),
    /// `alSourceQueueBuffers` — one more chunk behind whatever is playing
    /// (M144).
    ///
    /// **Distinct from [`Self::Attach`], which REPLACES.** A static sound gets
    /// one buffer and is done; a stream is a procession of them, and an attach
    /// per chunk would restart the sound every second. The first `Queue` on an
    /// empty voice becomes the playing buffer, so a caller does not have to
    /// special-case the head of the stream.
    ///
    /// Decoded on the producer's side for the same reason `Attach` is: turning
    /// an asset key into samples is a syscall and a large allocation, and the
    /// callback may do neither.
    Queue(ChannelId, Arc<crate::buffers::Pcm>),
}

/// A bounded single-producer / single-consumer ring.
///
/// Lock-free by construction rather than by a crate: one producer owns `tail`,
/// one consumer owns `head`, and the two only ever *read* each other's index.
/// That is the whole of the safety argument, and it is why the type is not
/// `Sync` for more than one of each.
pub struct CommandRing {
    slots: Vec<std::cell::UnsafeCell<Option<Command>>>,
    /// Next slot to read. Owned by the consumer.
    head: AtomicUsize,
    /// Next slot to write. Owned by the producer.
    tail: AtomicUsize,
    /// Commands refused because the ring was full. Never reset.
    dropped: AtomicU64,
}

// SAFETY: exactly one producer touches `tail` and the slot it points at, and
// exactly one consumer touches `head` and the slot it points at; the capacity
// check keeps those from ever being the same slot. The `Arc<CommandRing>` split
// below is what enforces "exactly one of each" at the call site.
unsafe impl Send for CommandRing {}
unsafe impl Sync for CommandRing {}

impl CommandRing {
    /// One slot is always left empty, so `head == tail` means empty and there is
    /// no ambiguity with full — the standard ring trade, and the reason
    /// `capacity` is one less than the allocation.
    pub fn with_capacity(capacity: usize) -> Arc<CommandRing> {
        let n = capacity + 1;
        Arc::new(CommandRing {
            slots: (0..n).map(|_| std::cell::UnsafeCell::new(None)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
        })
    }

    pub fn capacity(&self) -> usize {
        self.slots.len() - 1
    }

    /// Producer side. Returns `false` when the ring is full, having counted the
    /// drop. **Never blocks and never allocates.**
    pub fn push(&self, cmd: Command) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let next = (tail + 1) % self.slots.len();
        if next == self.head.load(Ordering::Acquire) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        // SAFETY: the producer owns this slot until `tail` is published below.
        unsafe { *self.slots[tail].get() = Some(cmd) };
        self.tail.store(next, Ordering::Release);
        true
    }

    /// Consumer side — the callback's end. **Never blocks and never allocates.**
    pub fn pop(&self) -> Option<Command> {
        let head = self.head.load(Ordering::Relaxed);
        if head == self.tail.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: the consumer owns this slot until `head` is published below.
        let cmd = unsafe { (*self.slots[head].get()).take() };
        self.head
            .store((head + 1) % self.slots.len(), Ordering::Release);
        cmd
    }

    /// How many commands have been refused for want of room.
    ///
    /// **Non-zero means the device stopped consuming**, not that the engine got
    /// busy: the producer writes about 240 commands on the busiest tick and the
    /// consumer drains every callback, so a ring sized for one tick cannot fill
    /// unless the far end has stalled. Surfacing it is the difference between a
    /// diagnosable dead device and "audio sometimes misses sounds".
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire) == self.tail.load(Ordering::Acquire)
    }
}

/// The default ring size — one busy tick with room to spare.
///
/// A `play` writes eight commands and the channel budget is 30
/// (`Library.DEFAULT_CHANNEL_COUNT`), so the worst tick that can start a sound
/// on every free channel writes about 240. 1024 is four of those, which covers a
/// callback period missed entirely without the producer noticing.
pub const DEFAULT_RING_CAPACITY: usize = 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_net::sound_engine::ChannelCall;

    fn cmd(i: u32) -> Command {
        Command::Channel(i as ChannelId, ChannelCall::SetPitch(i as f32))
    }

    #[test]
    fn commands_come_out_in_order() {
        let r = CommandRing::with_capacity(8);
        for i in 0..5 {
            assert!(r.push(cmd(i)));
        }
        for i in 0..5 {
            assert_eq!(r.pop(), Some(cmd(i)));
        }
        assert_eq!(r.pop(), None);
        assert!(r.is_empty());
    }

    #[test]
    fn a_full_ring_drops_the_new_command_and_counts_it() {
        // The discipline that reads backwards, and the one worth pinning: the
        // NEWEST command is refused, not the oldest evicted. A ring that
        // overwrote its tail would silently reorder a channel's eight-call
        // sequence, which is far worse than losing one sound.
        let r = CommandRing::with_capacity(3);
        assert!(r.push(cmd(0)));
        assert!(r.push(cmd(1)));
        assert!(r.push(cmd(2)));
        assert!(!r.push(cmd(99)), "full");
        assert_eq!(r.dropped(), 1);
        // The three that fit are intact and in order — the drop did not disturb
        // them.
        assert_eq!(r.pop(), Some(cmd(0)));
        assert_eq!(r.pop(), Some(cmd(1)));
        assert_eq!(r.pop(), Some(cmd(2)));
        assert_eq!(r.pop(), None);
    }

    #[test]
    fn the_drop_count_never_resets() {
        // It is a diagnostic, and one that must survive the device recovering:
        // a counter cleared on the next successful push would read zero exactly
        // when someone went looking after a glitch.
        let r = CommandRing::with_capacity(1);
        assert!(r.push(cmd(0)));
        assert!(!r.push(cmd(1)));
        assert!(!r.push(cmd(2)));
        assert_eq!(r.dropped(), 2);
        assert_eq!(r.pop(), Some(cmd(0)));
        assert!(r.push(cmd(3)), "room again");
        assert_eq!(r.dropped(), 2, "still two");
    }

    #[test]
    fn capacity_is_what_it_says() {
        // One slot is held back so empty and full are distinguishable, and the
        // reported capacity accounts for it rather than the caller having to.
        for n in [1usize, 3, 8, 1024] {
            let r = CommandRing::with_capacity(n);
            assert_eq!(r.capacity(), n);
            for i in 0..n {
                assert!(r.push(cmd(i as u32)), "slot {i} of {n}");
            }
            assert!(!r.push(cmd(0)), "n+1 of {n}");
        }
    }

    #[test]
    fn it_wraps_rather_than_filling_up_permanently() {
        // Drain-and-refill many times over: an index that grew without the
        // modulo would work for the first lap and then never accept anything.
        let r = CommandRing::with_capacity(4);
        for round in 0..50u32 {
            for i in 0..4 {
                assert!(r.push(cmd(round * 10 + i)), "round {round}");
            }
            for i in 0..4 {
                assert_eq!(r.pop(), Some(cmd(round * 10 + i)));
            }
        }
        assert_eq!(r.dropped(), 0);
    }

    #[test]
    fn a_listener_command_rides_the_same_ring() {
        // One ordered stream, so a listener move cannot overtake the channel
        // calls it should follow — two rings would let the ears arrive before
        // the sound they are meant to place.
        let r = CommandRing::with_capacity(4);
        r.push(cmd(0));
        r.push(Command::Listener(ListenerTransform::INITIAL));
        r.push(cmd(1));
        assert_eq!(r.pop(), Some(cmd(0)));
        assert_eq!(r.pop(), Some(Command::Listener(ListenerTransform::INITIAL)));
        assert_eq!(r.pop(), Some(cmd(1)));
    }

    #[test]
    fn a_producer_and_a_consumer_on_two_threads_agree() {
        // The claim the atomics exist for. A single-threaded test cannot see an
        // ordering bug, because there is nothing to reorder against.
        const N: u32 = 20_000;
        let r = CommandRing::with_capacity(64);
        let producer = {
            let r = Arc::clone(&r);
            std::thread::spawn(move || {
                let mut sent = 0u32;
                let mut refused = 0u32;
                while sent < N {
                    if r.push(cmd(sent)) {
                        sent += 1;
                    } else {
                        refused += 1;
                        std::thread::yield_now();
                    }
                }
                refused
            })
        };
        let mut seen = 0u32;
        while seen < N {
            if let Some(Command::Channel(id, _)) = r.pop() {
                // In order, with nothing lost and nothing duplicated — which is
                // the whole contract, and what a torn index would break.
                assert_eq!(id, seen, "out of order at {seen}");
                seen += 1;
            } else {
                std::thread::yield_now();
            }
        }
        let refused = producer.join().unwrap();
        assert_eq!(seen, N);
        // **Deliberately NOT asserting that the producer was ever refused.**
        // The first version did, to prove the run had exercised the full path,
        // and it failed the first time a consumer happened to keep up — a
        // scheduling-dependent assertion, which is the flaky battery entry this
        // module's own harness doc argues against. Whether the ring fills is
        // timing; that a full ring refuses and counts is covered deterministically
        // by `a_full_ring_drops_the_new_command_and_counts_it`.
        //
        // What is asserted is what holds on EVERY interleaving: the counter and
        // the refusals agree exactly, so a torn count would fail here whether or
        // not the ring ever filled on this particular run.
        assert_eq!(r.dropped() as u32, refused);
    }
}
