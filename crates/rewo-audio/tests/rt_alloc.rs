//! **The audio callback neither allocates nor frees.**
//!
//! Its own test binary because it installs a counting `#[global_allocator]`,
//! which would count every other test's allocations too. The count is per
//! thread and only while armed, so the test harness's own threads cannot touch
//! it, and what is measured is exactly the body `cpal_sink` runs on the audio
//! thread: [`RenderCallback::render_f32`] and [`RenderCallback::render_into`].
//!
//! The scenario walks every path that used to allocate or free in the
//! callback: a new voice (`Vec` growth, `Arc::new` of an empty buffer), a
//! queued stream buffer (`VecDeque` growth), a queue join and an underrun
//! (dropping and replacing the playing `Arc<Pcm>`), an attach that replaces a
//! buffer, `Stop` (dropping a voice with its queue), a finished one-shot
//! (`retire_finished`), a path-carrying attach (a `String`), and the NaN pitch
//! that used to hold a voice forever. The producer's side — building commands,
//! pushing them, draining the retire ring — runs disarmed, because it runs on
//! the engine's thread in the client.

use rewo_audio::buffers::Pcm;
use rewo_audio::callback::{f32_to_i16, CallbackStats, RenderCallback};
use rewo_audio::device::{command_ring, ring, Command, CommandProducer};
use rewo_audio::mixer::{Mixer, Retired};
use rewo_net::sound_engine::ChannelCall as C;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::Ordering;
use std::sync::Arc;

struct Counting;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static FREES: Cell<u64> = const { Cell::new(0) };
}

fn bump(counter: &'static std::thread::LocalKey<Cell<u64>>) {
    // `try_with`: a thread-local touched during thread teardown must not
    // panic inside the allocator.
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = counter.try_with(|c| c.set(c.get() + 1));
        }
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump(&ALLOCS);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump(&ALLOCS);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump(&ALLOCS);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        bump(&FREES);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Run `f` armed and return (allocations, frees) it made on this thread.
fn counted<R>(f: impl FnOnce() -> R) -> (R, u64, u64) {
    ALLOCS.with(|c| c.set(0));
    FREES.with(|c| c.set(0));
    ARMED.with(|a| a.set(true));
    let r = f();
    ARMED.with(|a| a.set(false));
    (r, ALLOCS.with(Cell::get), FREES.with(Cell::get))
}

fn pcm(frames: usize, level: i16) -> Arc<Pcm> {
    Arc::new(Pcm {
        samples: vec![level; frames],
        channels: 1,
        sample_rate: 44_100,
    })
}

/// One engine tick's worth of commands, varied by round so every path is
/// exercised repeatedly and on voices that come and go.
fn push_round(p: &CommandProducer, round: u32) {
    let one_shot = 100 + round % 7;
    // A full `play`: properties, attach, play — a NEW voice each round.
    for call in [
        C::SetPitch(1.25),
        C::SetVolume(0.5),
        C::LinearAttenuation(16.0),
        C::SetLooping(false),
        C::SetSelfPosition(1.0, 0.0, -2.0),
        C::SetRelative(false),
    ] {
        assert!(p.push(Command::Channel(one_shot, call)));
    }
    // Short enough to finish inside one period: `retire_finished` runs.
    assert!(p.push(Command::Attach(one_shot, pcm(64, 4000))));
    assert!(p.push(Command::Channel(one_shot, C::Play)));

    // A stream: the head is promoted, the rest queue, short buffers so the
    // period walks several joins and then underruns.
    let stream = 1;
    for _ in 0..3 {
        assert!(p.push(Command::Queue(stream, pcm(96, 3000))));
    }
    if round == 0 {
        assert!(p.push(Command::Channel(stream, C::Play)));
    }

    // An attach that REPLACES a playing buffer on a long-lived voice.
    let looped = 2;
    assert!(p.push(Command::Attach(looped, pcm(500, 2000))));
    assert!(p.push(Command::Channel(looped, C::SetLooping(true))));
    assert!(p.push(Command::Channel(looped, C::Play)));

    // A voice with a queue that is stopped mid-stream.
    let doomed = 50 + round % 3;
    assert!(p.push(Command::Queue(doomed, pcm(5000, 1000))));
    assert!(p.push(Command::Queue(doomed, pcm(5000, 1000))));
    assert!(p.push(Command::Channel(doomed, C::Play)));
    if round % 2 == 1 {
        for d in 50..53 {
            assert!(p.push(Command::Channel(d, C::Stop)));
        }
    }

    // The two values that are not plain data: a NaN pitch and a
    // path-carrying attach that owns a `String`.
    assert!(p.push(Command::Channel(looped, C::SetPitch(f32::NAN))));
    assert!(p.push(Command::Channel(
        77,
        C::AttachStaticBuffer("minecraft/sounds/x.ogg".into())
    )));
}

#[test]
fn the_callback_neither_allocates_nor_frees() {
    let (producer, commands) = command_ring(1024);
    let (grave_tx, graveyard) = ring::<Retired>(4096);
    let stats = Arc::new(CallbackStats::default());
    let mut cb = RenderCallback::new(Mixer::new(48_000), commands, grave_tx, Arc::clone(&stats));
    // The device's buffers are cpal's, allocated before any callback.
    let mut out_f32 = vec![0f32; 2 * 480];
    let mut out_i16 = vec![0i16; 2 * 9000]; // longer than the scratch buffer

    let mut retired = 0u64;
    let mut freed_by_draining = 0u64;
    for round in 0..40u32 {
        push_round(&producer, round);
        let ((), allocs, frees) = counted(|| {
            if round % 2 == 0 {
                cb.render_f32(&mut out_f32);
            } else {
                cb.render_into(&mut out_i16, f32_to_i16);
            }
        });
        assert_eq!(
            (allocs, frees),
            (0, 0),
            "round {round}: the callback allocated {allocs} time(s) and freed {frees} time(s)"
        );
        // The engine's side: what the callback let go of is freed HERE.
        let ((), _, frees) = counted(|| {
            while let Some(r) = graveyard.pop() {
                retired += 1;
                drop(r);
            }
        });
        freed_by_draining += frees;
    }

    // The test measured something: the callback really did hand buffers out,
    // and dropping them really did free — so the zero above is the frees having
    // MOVED, not a scenario that never released anything.
    assert!(retired > 100, "only {retired} values were retired");
    assert!(
        freed_by_draining > 100,
        "draining freed {freed_by_draining}"
    );
    // And it really did play: a voice ends up audible in the last f32 period.
    assert!(out_f32.iter().any(|s| *s != 0.0));
    let m = cb.mixer();
    assert_eq!(m.ignored, 40, "one path-carrying attach per round");
    assert_eq!(m.freed_in_place(), 0, "the retire ring never overflowed");
    assert_eq!(stats.heartbeat.load(Ordering::Relaxed), 40);
    // The NaN pitch was clamped rather than stored.
    assert_eq!(m.voice(2).map(|v| v.pitch), Some(1.0));
}

/// Overflow is refused, counted and still free of allocation — the documented
/// behaviour when more channels arrive than there are voice slots, or more
/// buffers than a queue holds.
#[test]
fn overflowing_the_preallocation_refuses_without_allocating() {
    use rewo_audio::mixer::{MAX_QUEUED, MAX_VOICES};
    let (producer, commands) = command_ring(4096);
    let (grave_tx, graveyard) = ring::<Retired>(4096);
    let mut cb = RenderCallback::new(Mixer::new(48_000), commands, grave_tx, Arc::default());
    let mut out = vec![0f32; 2 * 64];
    // Commands built on the engine's side, disarmed.
    for id in 0..(MAX_VOICES as u32 + 10) {
        assert!(producer.push(Command::Channel(id, C::SetVolume(1.0))));
    }
    for _ in 0..(MAX_QUEUED + 5) {
        assert!(producer.push(Command::Queue(0, pcm(4800, 100))));
    }
    let ((), allocs, frees) = counted(|| cb.render_f32(&mut out));
    assert_eq!((allocs, frees), (0, 0));
    let m = cb.mixer();
    assert_eq!(m.voice_count(), MAX_VOICES);
    assert_eq!(m.refused, 10);
    // The head is promoted to playing, a queue's worth wait, the rest overflow.
    // (`with_capacity` promises at least MAX_QUEUED, so read the real room.)
    let room = m.voice(0).unwrap().queue.capacity();
    assert!((MAX_QUEUED..MAX_QUEUED + 4).contains(&room), "{room}");
    assert_eq!(m.voice(0).unwrap().queue.len(), room);
    let overflow = MAX_QUEUED + 4 - room;
    assert_eq!(m.queue_overflow as usize, overflow);
    let mut n = 0;
    while graveyard.pop().is_some() {
        n += 1;
    }
    assert_eq!(
        n, overflow,
        "the overflowed buffers were retired, not dropped"
    );
}
