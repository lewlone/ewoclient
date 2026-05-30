//! Media controller state — the "currently playing" data feeding the HUD's
//! media controller widget.
//!
//! The renderer reads [`MediaState`] every frame to paint the song title,
//! artist, scrub bar, transport controls, and thumbnail. The data is sourced
//! from the Windows System Media Transport Controls (SMTC) — a per-process
//! global feed that every modern media app (Spotify, browsers, Apple Music,
//! system audio) writes to. The polling + action dispatch lives in a
//! background thread spawned by [`MediaService::start`]; the main thread
//! drains updates each frame via [`MediaService::poll`].
//!
//! All state lives in [`Editor::media`]; rendering reads it through the
//! shared `&MediaState` reference. Transport clicks (play / pause / skip)
//! flow back via [`MediaService::act`] and the polling thread translates them
//! into `TryPlayAsync` / `TryPauseAsync` / etc.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::Instant;

use skia_safe::{Data, Image};

/// One snapshot of the live "now playing" feed.
///
/// `title.is_empty() && artist.is_empty()` is the empty / idle state — the UI
/// branches on this to render the design's `.media-large.empty` / `.w-media.empty`
/// forms ("Silence is also a song" / "Nothing playing").
pub struct MediaState {
    /// Track title. Empty when nothing is playing.
    pub title: String,
    /// Track artist (or `"<artist>"` / `"<artist> · <album>"` if backend provides).
    pub artist: String,
    /// Whether the source is actively playing.
    pub playing: bool,
    /// Anchor playback position, in seconds — the value the SMTC backend
    /// reported in the most recent snapshot. Use [`Self::displayed_position`]
    /// to read the *current* live position (anchor + time since snapshot).
    pub position_seconds: f32,
    /// Wall-clock instant when `position_seconds` was last updated. SMTC
    /// only snapshots every 500ms; between snapshots the scrub bar advances
    /// smoothly by adding `(now - position_updated_at)` if the source is
    /// playing. Set to `Instant::now()` on every apply, including idle resets.
    pub position_updated_at: Instant,
    /// Total track duration, in seconds. `0.0` if unknown.
    pub duration_seconds: f32,
    /// Album art / station logo, decoded from the source's thumbnail bytes.
    pub thumbnail: Option<Image>,
    /// Backend source name for the small eyebrow (e.g. `"SPOTIFY"`, `"BRAVE"`).
    /// Empty when no backend is connected.
    pub source: String,
    /// Wall-clock instant when `title` last changed. The compact widget's
    /// marquee scroll uses this as its time anchor so each new track starts
    /// at the leftmost position with a fresh pause-then-scroll cycle.
    pub title_changed_at: Instant,
}

impl MediaState {
    pub fn empty() -> Self {
        Self {
            title: String::new(),
            artist: String::new(),
            playing: false,
            position_seconds: 0.0,
            position_updated_at: Instant::now(),
            duration_seconds: 0.0,
            thumbnail: None,
            source: String::new(),
            title_changed_at: Instant::now(),
        }
    }

    /// `true` when no source is connected — drives the renderer to draw the
    /// "Silence is also a song" empty card.
    pub fn is_idle(&self) -> bool {
        self.title.is_empty() && self.artist.is_empty()
    }

    /// Marquee-scroll offset (px to translate the title leftward by) for a
    /// title that overflows the visible column. Returns `0.0` when the title
    /// fits. Cycle: pause at left → scroll right-to-left → pause at right →
    /// scroll back to left → loop. Anchored on `title_changed_at` so each
    /// new track starts fresh from the left.
    pub fn marquee_offset(&self, title_w: f32, visible_w: f32) -> f32 {
        let overflow = title_w - visible_w;
        if overflow <= 0.0 {
            return 0.0;
        }
        const SPEED: f32 = 28.0; // pixels per second — readable but not sleepy.
        const PAUSE: f32 = 1.6; // pause at each end so the user can read it.
        let travel = overflow / SPEED;
        let cycle = (PAUSE + travel) * 2.0;
        let t = self.title_changed_at.elapsed().as_secs_f32() % cycle;
        if t < PAUSE {
            0.0
        } else if t < PAUSE + travel {
            (t - PAUSE) * SPEED
        } else if t < PAUSE + travel + PAUSE {
            overflow
        } else {
            overflow - (t - PAUSE - travel - PAUSE) * SPEED
        }
    }

    /// The current playback position, in seconds — interpolates the SMTC
    /// snapshot forward by the elapsed wall-clock time when the source is
    /// playing. Capped at `duration_seconds` so the scrub doesn't overshoot
    /// the end while we wait for the next snapshot.
    pub fn displayed_position(&self) -> f32 {
        if !self.playing {
            return self.position_seconds;
        }
        let elapsed = self.position_updated_at.elapsed().as_secs_f32();
        let live = self.position_seconds + elapsed;
        if self.duration_seconds > 0.0 {
            live.min(self.duration_seconds)
        } else {
            live
        }
    }
}

impl Default for MediaState {
    fn default() -> Self {
        Self::empty()
    }
}

/// A transport action requested by the user pressing one of the controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaAction {
    PlayPause,
    Next,
    Previous,
}

/// One snapshot dispatched from the polling thread back to the main thread.
pub struct MediaSnapshot {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub position_seconds: f32,
    pub duration_seconds: f32,
    /// Encoded PNG/JPG bytes — decoded into a Skia `Image` on the main thread
    /// (the polling thread can't, Skia images aren't `Send`).
    pub thumbnail_bytes: Option<Vec<u8>>,
    pub source: String,
}

pub enum MediaUpdate {
    Snapshot(MediaSnapshot),
    Idle,
}

/// The background SMTC subscriber. Spawned once at HUD init; its polling
/// thread feeds updates back through the channel.
pub struct MediaService {
    rx_updates: Receiver<MediaUpdate>,
    tx_actions: Sender<MediaAction>,
    /// Fingerprint of the bytes we last decoded into `state.thumbnail` — used
    /// to skip the PNG decode + GPU upload on snapshots that carry identical
    /// bytes. `0` means "nothing decoded yet"; the actual hash is never `0`
    /// in practice because we mix the length in. See `fingerprint_thumb`.
    last_thumb_fingerprint: u64,
}

impl MediaService {
    /// Spawn the background poller. Runs until the process exits.
    pub fn start() -> Self {
        let (tx_updates, rx_updates) = channel();
        let (tx_actions, rx_actions) = channel();
        thread::Builder::new()
            .name("ewo-smtc".into())
            .spawn(move || smtc_thread(tx_updates, rx_actions))
            .ok();
        Self { rx_updates, tx_actions, last_thumb_fingerprint: 0 }
    }

    /// Drain pending updates into `state`. Cheap to call every frame.
    pub fn poll(&mut self, state: &mut MediaState) {
        while let Ok(update) = self.rx_updates.try_recv() {
            self.apply(state, update);
        }
    }

    fn apply(&mut self, state: &mut MediaState, update: MediaUpdate) {
        match update {
            MediaUpdate::Snapshot(snap) => {
                let title_changed = snap.title != state.title;
                state.title = snap.title.clone();
                state.artist = snap.artist;
                state.playing = snap.playing;
                state.position_seconds = snap.position_seconds;
                state.position_updated_at = Instant::now();
                state.duration_seconds = snap.duration_seconds;
                state.source = snap.source;
                if title_changed {
                    state.title_changed_at = Instant::now();
                }

                // Thumbnail: decode only when the *bytes* differ from what we
                // last decoded. The SMTC thread re-reads the thumbnail every
                // poll, so a source that updates its title before its album-
                // art stream is ready (browser tabs do this) eventually sends
                // us the matching bytes — we pick them up on a subsequent
                // poll without spurious re-decodes when the bytes are stable.
                match snap.thumbnail_bytes.as_ref() {
                    Some(bytes) => {
                        let fp = fingerprint_thumb(bytes);
                        if fp != self.last_thumb_fingerprint {
                            state.thumbnail =
                                Image::from_encoded(Data::new_copy(bytes));
                            self.last_thumb_fingerprint = fp;
                        }
                    }
                    None => {
                        // No thumbnail data available. Drop any cached image
                        // (and its GPU texture) so the renderer's fallback
                        // takes over.
                        if self.last_thumb_fingerprint != 0 {
                            state.thumbnail = None;
                            self.last_thumb_fingerprint = 0;
                        }
                    }
                }
            }
            MediaUpdate::Idle => {
                *state = MediaState::empty();
                self.last_thumb_fingerprint = 0;
            }
        }
    }

    /// Send a transport action to the SMTC thread.
    pub fn act(&self, action: MediaAction) {
        let _ = self.tx_actions.send(action);
    }
}

/// Cheap fingerprint of a thumbnail byte blob: length + first/last 16-byte
/// chunks mixed with FNV-1a. Different PNGs reliably hash differently; the
/// same bytes always hash to the same value. Skips the full memcmp + decode.
/// Returns a non-zero hash for non-empty input so callers can use `0` as the
/// "nothing cached" sentinel.
fn fingerprint_thumb(bytes: &[u8]) -> u64 {
    if bytes.is_empty() {
        return 0;
    }
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut h = FNV_OFFSET ^ (bytes.len() as u64).wrapping_mul(FNV_PRIME);
    // Hash up to 16 bytes from the head + 16 bytes from the tail. Cheap, but
    // discriminates between any two PNGs in practice (their headers differ
    // and so does the trailing IDAT/IEND boundary).
    let head_len = bytes.len().min(16);
    for &b in &bytes[..head_len] {
        h = (h ^ b as u64).wrapping_mul(FNV_PRIME);
    }
    let tail_start = bytes.len().saturating_sub(16);
    for &b in &bytes[tail_start..] {
        h = (h ^ b as u64).wrapping_mul(FNV_PRIME);
    }
    if h == 0 {
        1
    } else {
        h
    }
}

// ────────────────────────────────────────────────────────────────────────
// Windows SMTC polling + transport dispatch.
// ────────────────────────────────────────────────────────────────────────

#[cfg(windows)]
fn smtc_thread(tx: Sender<MediaUpdate>, rx_actions: Receiver<MediaAction>) {
    use std::time::Duration;
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
    };
    use windows::Storage::Streams::DataReader;

    // The SMTC manager is a one-shot async; if it fails the thread quietly
    // exits and the widget stays in the empty state. Wrapped in a fn to use
    // `?` for early-return on any error.
    fn get_manager() -> windows::core::Result<Manager> {
        Manager::RequestAsync()?.get()
    }
    let manager = match get_manager() {
        Ok(m) => m,
        Err(_) => return,
    };

    loop {
        // Apply any pending transport actions first (so the next snapshot
        // already reflects the resulting playback state).
        let current_session = manager.GetCurrentSession().ok();
        if let Some(session) = current_session.as_ref() {
            while let Ok(action) = rx_actions.try_recv() {
                apply_action(session, action);
            }
        } else {
            // Drain queued actions even with no session, so they don't apply
            // to a future session unexpectedly.
            while rx_actions.try_recv().is_ok() {}
        }

        // Snapshot the current session (if any).
        let snapshot = current_session.as_ref().and_then(capture_snapshot);
        match snapshot {
            Some(snap) => {
                let _ = tx.send(MediaUpdate::Snapshot(snap));
            }
            None => {
                let _ = tx.send(MediaUpdate::Idle);
            }
        }

        thread::sleep(Duration::from_millis(500));
        // Helper closures defined inside the loop scope so they capture the
        // imports. `apply_action` and `capture_snapshot` are below.
        #[allow(unused_imports)]
        use windows::Foundation::TimeSpan;

        fn apply_action(session: &Session, action: MediaAction) {
            let _ = match action {
                MediaAction::PlayPause => {
                    // Toggle based on current status — if playing, pause; else play.
                    let status = session
                        .GetPlaybackInfo()
                        .and_then(|p| p.PlaybackStatus())
                        .unwrap_or(PlaybackStatus(0));
                    if status == PlaybackStatus(4) {
                        session.TryPauseAsync().and_then(|op| op.get())
                    } else {
                        session.TryPlayAsync().and_then(|op| op.get())
                    }
                    .map(|_| ())
                }
                MediaAction::Next => session
                    .TrySkipNextAsync()
                    .and_then(|op| op.get())
                    .map(|_| ()),
                MediaAction::Previous => session
                    .TrySkipPreviousAsync()
                    .and_then(|op| op.get())
                    .map(|_| ()),
            };
        }

        fn capture_snapshot(session: &Session) -> Option<MediaSnapshot> {
            let props = session.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
            let title = props.Title().map(|h| h.to_string_lossy()).unwrap_or_default();
            let artist = props.Artist().map(|h| h.to_string_lossy()).unwrap_or_default();

            let playback = session.GetPlaybackInfo().ok();
            let playing = playback
                .as_ref()
                .and_then(|p| p.PlaybackStatus().ok())
                .map(|s| s == PlaybackStatus(4))
                .unwrap_or(false);

            // Timeline — Position + EndTime are TimeSpans (100ns units).
            let timeline = session.GetTimelineProperties().ok();
            let (pos_s, dur_s) = timeline
                .map(|t| {
                    let pos = t
                        .Position()
                        .map(|d| d.Duration as f32 / 10_000_000.0)
                        .unwrap_or(0.0);
                    let end = t
                        .EndTime()
                        .map(|d| d.Duration as f32 / 10_000_000.0)
                        .unwrap_or(0.0);
                    (pos.max(0.0), end.max(0.0))
                })
                .unwrap_or((0.0, 0.0));

            // Source app — shorten "AppName.AppName_publisher!App" to "AppName".
            let source_id = session
                .SourceAppUserModelId()
                .map(|h| h.to_string_lossy())
                .unwrap_or_default();
            let source = shorten_source(&source_id);

            // Thumbnail — read every poll. Sources that update the title
            // field before the album-art stream is ready (browsers do this)
            // eventually expose the matching bytes; the renderer dedups by
            // fingerprint so a stable image only decodes once.
            let thumbnail_bytes = if title.is_empty() {
                None
            } else {
                read_thumbnail_bytes(&props)
            };

            Some(MediaSnapshot {
                title,
                artist,
                playing,
                position_seconds: pos_s,
                duration_seconds: dur_s,
                thumbnail_bytes,
                source,
            })
        }

        fn read_thumbnail_bytes(
            props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties,
        ) -> Option<Vec<u8>> {
            let thumb = props.Thumbnail().ok()?;
            let stream = thumb.OpenReadAsync().ok()?.get().ok()?;
            let size = stream.Size().ok()? as u32;
            if size == 0 {
                return None;
            }
            let reader = DataReader::CreateDataReader(&stream).ok()?;
            reader.LoadAsync(size).ok()?.get().ok()?;
            let mut bytes = vec![0u8; size as usize];
            reader.ReadBytes(&mut bytes).ok()?;
            Some(bytes)
        }
    }
}

#[cfg(not(windows))]
fn smtc_thread(_tx: Sender<MediaUpdate>, _rx_actions: Receiver<MediaAction>) {
    // SMTC is a Windows-only API. On other platforms the media-controller
    // widget renders its empty state and that's that.
}

/// Trim a Windows AppUserModelID down to a short display label.
/// `"Spotify.Spotify"` → `"SPOTIFY"`, `"AppName_PublisherID!App"` → `"APPNAME"`.
#[cfg(windows)]
fn shorten_source(id: &str) -> String {
    // Strip the publisher hash + `!App` suffix.
    let before_bang = id.split('!').next().unwrap_or(id);
    // Strip the publisher suffix `_xxxxxxxx`.
    let before_underscore = before_bang.split('_').next().unwrap_or(before_bang);
    // Take the last `.`-separated component — typically the app name.
    let app = before_underscore.rsplit('.').next().unwrap_or(before_underscore);
    app.to_uppercase()
}
