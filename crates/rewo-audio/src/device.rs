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

/// The shared storage behind one [`Producer`] / [`Consumer`] pair.
///
/// Lock-free by construction rather than by a crate: the producer owns `tail`,
/// the consumer owns `head`, and the two only ever *read* each other's index.
/// **Private, and deliberately neither `Send` nor `Sync`** (the `UnsafeCell`s
/// see to that): the only way to reach it is through the two handles, and the
/// type system rather than a comment is what limits them to one of each.
struct Ring<T> {
    slots: Box<[std::cell::UnsafeCell<Option<T>>]>,
    /// Next slot to read. Owned by the consumer.
    head: AtomicUsize,
    /// Next slot to write. Owned by the producer.
    tail: AtomicUsize,
    /// Values refused because the ring was full. Never reset.
    dropped: AtomicU64,
}

/// The write end of a single-producer / single-consumer ring.
///
/// **Not `Clone` and not `Sync`**, which is the whole safety argument written
/// into the type: [`ring`] makes exactly one producer and one consumer, a
/// producer cannot be duplicated, and `&Producer` cannot cross a thread, so at
/// most one thread is ever inside [`Producer::push`]. It is `Send`, so the end
/// can be moved to whichever thread writes.
///
/// ```compile_fail
/// let (p, _c) = rewo_audio::device::command_ring(4);
/// let _second = p.clone(); // one producer only
/// ```
///
/// ```compile_fail
/// fn shared<T: Sync>(_: &T) {}
/// let (p, _c) = rewo_audio::device::command_ring(4);
/// shared(&p); // `&Producer` must not cross threads
/// ```
pub struct Producer<T> {
    ring: Arc<Ring<T>>,
}

/// The read end. Exactly as constrained as [`Producer`], for the same reason.
///
/// ```compile_fail
/// let (_p, c) = rewo_audio::device::command_ring(4);
/// let _second = c.clone(); // one consumer only
/// ```
///
/// ```compile_fail
/// fn shared<T: Sync>(_: &T) {}
/// let (_p, c) = rewo_audio::device::command_ring(4);
/// shared(&c);
/// ```
pub struct Consumer<T> {
    ring: Arc<Ring<T>>,
}

// SAFETY: a `Producer` touches only `tail` and the slot `tail` points at; a
// `Consumer` only `head` and the slot `head` points at. The capacity check
// keeps those from ever being the same slot, and the Release/Acquire pair on
// the index hands a slot's contents from one side to the other. `ring()` makes
// exactly one of each and neither is `Clone` or `Sync`, so "one thread per end"
// holds by construction. Moving an end to another thread moves `T`s with it,
// hence `T: Send`.
unsafe impl<T: Send> Send for Producer<T> {}
unsafe impl<T: Send> Send for Consumer<T> {}

/// A bounded SPSC ring of `capacity` values, as its two ends.
///
/// One slot is always left empty, so `head == tail` means empty and there is no
/// ambiguity with full — the standard ring trade, and the reason the allocation
/// is one more than `capacity`. **This is the only allocation the ring ever
/// makes**; neither end allocates afterwards.
pub fn ring<T>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let n = capacity + 1;
    let ring = Arc::new(Ring {
        slots: (0..n).map(|_| std::cell::UnsafeCell::new(None)).collect(),
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        dropped: AtomicU64::new(0),
    });
    (
        Producer {
            ring: Arc::clone(&ring),
        },
        Consumer { ring },
    )
}

/// The engine-to-callback command ring.
pub fn command_ring(capacity: usize) -> (CommandProducer, CommandConsumer) {
    ring(capacity)
}

/// The engine's end of the command ring.
pub type CommandProducer = Producer<Command>;
/// The audio callback's end of the command ring.
pub type CommandConsumer = Consumer<Command>;

impl<T> Ring<T> {
    fn capacity(&self) -> usize {
        self.slots.len() - 1
    }

    fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire) == self.tail.load(Ordering::Acquire)
    }
}

impl<T> Producer<T> {
    pub fn capacity(&self) -> usize {
        self.ring.capacity()
    }

    /// Returns `false` when the ring is full, having counted the drop and
    /// dropped `value` **on this thread**. **Never blocks and never allocates.**
    pub fn push(&self, value: T) -> bool {
        self.try_push(value).is_ok()
    }

    /// [`Self::push`], handing a refused value back instead of dropping it.
    ///
    /// For a producer that must not free either — the audio callback writing
    /// retired buffers out to a worker, where dropping the last `Arc<Pcm>` is
    /// exactly the deallocation the ring exists to move.
    pub fn try_push(&self, value: T) -> Result<(), T> {
        let r = &*self.ring;
        let tail = r.tail.load(Ordering::Relaxed);
        let next = (tail + 1) % r.slots.len();
        if next == r.head.load(Ordering::Acquire) {
            r.dropped.fetch_add(1, Ordering::Relaxed);
            return Err(value);
        }
        // SAFETY: the producer owns this slot until `tail` is published below,
        // and there is only one producer (see the `Send` impl).
        unsafe { *r.slots[tail].get() = Some(value) };
        r.tail.store(next, Ordering::Release);
        Ok(())
    }

    /// How many values have been refused for want of room.
    ///
    /// **Non-zero on the command ring means the device stopped consuming**, not
    /// that the engine got busy: the producer writes about 240 commands on the
    /// busiest tick and the consumer drains every callback, so a ring sized for
    /// one tick cannot fill unless the far end has stalled. Surfacing it is the
    /// difference between a diagnosable dead device and "audio sometimes misses
    /// sounds".
    pub fn dropped(&self) -> u64 {
        self.ring.dropped.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}

impl<T> Consumer<T> {
    pub fn capacity(&self) -> usize {
        self.ring.capacity()
    }

    /// The consumer's end — the callback's, for commands. **Never blocks and
    /// never allocates.**
    ///
    /// Moves the value out of its slot rather than copying it, so the slot is
    /// left holding `None` and nothing is dropped here: whatever the value owns
    /// is dropped wherever the caller lets go of it.
    pub fn pop(&self) -> Option<T> {
        let r = &*self.ring;
        let head = r.head.load(Ordering::Relaxed);
        if head == r.tail.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: the consumer owns this slot until `head` is published below,
        // and there is only one consumer.
        let value = unsafe { (*r.slots[head].get()).take() };
        r.head.store((head + 1) % r.slots.len(), Ordering::Release);
        value
    }

    /// Values the producer has had refused. The same counter as
    /// [`Producer::dropped`], readable from this end too.
    pub fn dropped(&self) -> u64 {
        self.ring.dropped.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
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
        let (r, c) = command_ring(8);
        for i in 0..5 {
            assert!(r.push(cmd(i)));
        }
        for i in 0..5 {
            assert_eq!(c.pop(), Some(cmd(i)));
        }
        assert_eq!(c.pop(), None);
        assert!(r.is_empty() && c.is_empty());
    }

    #[test]
    fn a_full_ring_drops_the_new_command_and_counts_it() {
        // The discipline that reads backwards, and the one worth pinning: the
        // NEWEST command is refused, not the oldest evicted. A ring that
        // overwrote its tail would silently reorder a channel's eight-call
        // sequence, which is far worse than losing one sound.
        let (r, c) = command_ring(3);
        assert!(r.push(cmd(0)));
        assert!(r.push(cmd(1)));
        assert!(r.push(cmd(2)));
        assert!(!r.push(cmd(99)), "full");
        assert_eq!(r.dropped(), 1);
        // The three that fit are intact and in order — the drop did not disturb
        // them.
        assert_eq!(c.pop(), Some(cmd(0)));
        assert_eq!(c.pop(), Some(cmd(1)));
        assert_eq!(c.pop(), Some(cmd(2)));
        assert_eq!(c.pop(), None);
    }

    #[test]
    fn the_drop_count_never_resets() {
        // It is a diagnostic, and one that must survive the device recovering:
        // a counter cleared on the next successful push would read zero exactly
        // when someone went looking after a glitch.
        let (r, c) = command_ring(1);
        assert!(r.push(cmd(0)));
        assert!(!r.push(cmd(1)));
        assert!(!r.push(cmd(2)));
        assert_eq!(r.dropped(), 2);
        assert_eq!(c.pop(), Some(cmd(0)));
        assert!(r.push(cmd(3)), "room again");
        assert_eq!(r.dropped(), 2, "still two");
    }

    #[test]
    fn capacity_is_what_it_says() {
        // One slot is held back so empty and full are distinguishable, and the
        // reported capacity accounts for it rather than the caller having to.
        for n in [1usize, 3, 8, 1024] {
            let (r, _c) = command_ring(n);
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
        let (r, c) = command_ring(4);
        for round in 0..50u32 {
            for i in 0..4 {
                assert!(r.push(cmd(round * 10 + i)), "round {round}");
            }
            for i in 0..4 {
                assert_eq!(c.pop(), Some(cmd(round * 10 + i)));
            }
        }
        assert_eq!(r.dropped(), 0);
    }

    #[test]
    fn a_listener_command_rides_the_same_ring() {
        // One ordered stream, so a listener move cannot overtake the channel
        // calls it should follow — two rings would let the ears arrive before
        // the sound they are meant to place.
        let (r, c) = command_ring(4);
        r.push(cmd(0));
        r.push(Command::Listener(ListenerTransform::INITIAL));
        r.push(cmd(1));
        assert_eq!(c.pop(), Some(cmd(0)));
        assert_eq!(c.pop(), Some(Command::Listener(ListenerTransform::INITIAL)));
        assert_eq!(c.pop(), Some(cmd(1)));
    }

    #[test]
    fn a_producer_and_a_consumer_on_two_threads_agree() {
        // The claim the atomics exist for. A single-threaded test cannot see an
        // ordering bug, because there is nothing to reorder against.
        const N: u32 = 20_000;
        let (r, c) = command_ring(64);
        let producer = {
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
                (refused, r.dropped())
            })
        };
        let mut seen = 0u32;
        while seen < N {
            if let Some(Command::Channel(id, _)) = c.pop() {
                // In order, with nothing lost and nothing duplicated — which is
                // the whole contract, and what a torn index would break.
                assert_eq!(id, seen, "out of order at {seen}");
                seen += 1;
            } else {
                std::thread::yield_now();
            }
        }
        let (refused, dropped) = producer.join().unwrap();
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
        assert_eq!(dropped as u32, refused);
        assert_eq!(c.dropped(), dropped, "both ends read one counter");
    }

    #[test]
    fn the_handles_are_send_and_neither_clone_nor_sync() {
        // The compile_fail doc tests on `Producer` and `Consumer` are the
        // negative half; this is the positive half, so a change that made the
        // ends unusable across threads fails here rather than in a caller.
        fn sendable<T: Send>(_: &T) {}
        let (p, c) = command_ring(4);
        sendable(&p);
        sendable(&c);
        // The `impls`-crate probe: an inherent method with a bound wins over the
        // trait's default only when the type satisfies the bound.
        trait No {
            fn is_clone(&self) -> bool {
                false
            }
            fn is_sync(&self) -> bool {
                false
            }
        }
        struct CloneProbe<T>(std::marker::PhantomData<T>);
        impl<T: Clone> CloneProbe<T> {
            fn is_clone(&self) -> bool {
                true
            }
        }
        impl<T> No for CloneProbe<T> {}
        struct SyncProbe<T>(std::marker::PhantomData<T>);
        impl<T: Sync> SyncProbe<T> {
            fn is_sync(&self) -> bool {
                true
            }
        }
        impl<T> No for SyncProbe<T> {}
        assert!(!CloneProbe::<CommandProducer>(std::marker::PhantomData).is_clone());
        assert!(!CloneProbe::<CommandConsumer>(std::marker::PhantomData).is_clone());
        assert!(!SyncProbe::<CommandProducer>(std::marker::PhantomData).is_sync());
        assert!(!SyncProbe::<CommandConsumer>(std::marker::PhantomData).is_sync());
        // And the probe does see the traits when they are there.
        assert!(CloneProbe::<u8>(std::marker::PhantomData).is_clone());
        assert!(SyncProbe::<u8>(std::marker::PhantomData).is_sync());
    }

    #[test]
    fn a_refused_value_comes_back_from_try_push() {
        // The retire ring's contract: the callback must get a refused buffer
        // back, because dropping it there is the free the ring exists to move.
        let (p, c) = ring::<Box<u32>>(1);
        assert!(p.try_push(Box::new(1)).is_ok());
        let back = p.try_push(Box::new(2)).expect_err("full");
        assert_eq!(*back, 2);
        assert_eq!(p.dropped(), 1);
        assert_eq!(c.pop().map(|b| *b), Some(1));
    }
}
