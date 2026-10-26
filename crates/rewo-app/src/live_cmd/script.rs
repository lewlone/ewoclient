//! `REWO_SCRIPT=<scenario.json>` — the shared playtest harness's scenario
//! interpreter (`--features gates` only).
//!
//! The harness (`Desktop\playtest-harness`) drives vanilla 26.2 through a
//! Fabric "puppet" mod and compares runs frame by frame, sound by sound and
//! correction by correction. This module is the Rewo half: it reads the same
//! `puppet-scenario.json` the puppet reads and writes the same files the
//! puppet writes (`captures/*.png`, `positions.jsonl`, `sounds.jsonl`,
//! `corrections.jsonl`, `events.jsonl`, `summary.json`) into
//! `REWO_SCRIPT_OUT`, so `python -m harness compare` reads either client.
//!
//! **Input goes through the production handlers.** A key is
//! [`LiveApp::key_input`] (the body of the `KeyboardInput` arm), a click or a
//! cursor move or a wheel notch is a synthesized `WindowEvent` handed to
//! `window_event`. The window is never shown, the cursor is never grabbed,
//! and real input is dropped while a script runs (`app.rs`).
//!
//! **Ticks.** Tick 0 is the first client tick after the player spawned, the
//! setup commands were sent, `settle_ticks` passed and the world was ready
//! (no mesh in flight, nothing dirty, the columns around the player loaded)
//! for 20 ticks in a row — the puppet's rule. While running, the script owns
//! the 20 Hz clock: one tick per 50 ms of wall time, applied steps at the
//! tick's head, the position at its tail, and **no tick while a capture
//! waits** for the world to be ready again. Frames render at partial tick 1.0
//! (`tick_accum` is pinned to `TICK_DT`), so a capture shows exactly the
//! state at the end of its tick.

use super::*;
use std::io::Write;

use winit::dpi::PhysicalPosition;
use winit::event::{MouseScrollDelta, TouchPhase};

/// Scenario steps and setup, as the harness writes them (`puppet_view`).
struct Scenario {
    name: String,
    setup_commands: Vec<String>,
    late_commands: Vec<String>,
    late_after: u32,
    settle_ticks: u32,
    clear_chat: bool,
    steps: Vec<serde_json::Value>,
    end_tick: i64,
}

fn strings(v: Option<&serde_json::Value>) -> Vec<String> {
    match v {
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

impl Scenario {
    fn parse(text: &str) -> Result<Scenario, String> {
        let v: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("REWO_SCRIPT: {e}"))?;
        let setup = v.get("setup").cloned().unwrap_or_default();
        let int =
            |o: &serde_json::Value, k: &str, d: i64| o.get(k).and_then(|x| x.as_i64()).unwrap_or(d);
        let mut steps: Vec<serde_json::Value> = v
            .get("steps")
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default();
        for (i, s) in steps.iter().enumerate() {
            if s.get("at").and_then(|a| a.as_i64()).is_none() {
                return Err(format!("REWO_SCRIPT: step {i} has no integer 'at'"));
            }
        }
        // Sorted by `at`; same-tick steps keep file order (a stable sort).
        steps.sort_by_key(|s| s["at"].as_i64().unwrap_or(0));
        let last = steps.last().map_or(0, |s| s["at"].as_i64().unwrap_or(0));
        Ok(Scenario {
            name: v
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("scenario")
                .to_string(),
            setup_commands: strings(setup.get("commands")),
            late_commands: strings(setup.get("late_commands")),
            late_after: int(&setup, "late_after", 10).max(0) as u32,
            settle_ticks: int(&setup, "settle_ticks", 40).max(0) as u32,
            clear_chat: setup
                .get("clear_chat")
                .and_then(|c| c.as_bool())
                .unwrap_or(true),
            steps,
            end_tick: int(&v, "end_tick", last + 1),
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Joining,
    Settling,
    Running,
    Done,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Joining => "joining",
            Phase::Settling => "settling",
            Phase::Running => "running",
            Phase::Done => "done",
        }
    }
}

/// A capture in progress: waiting for a ready world, then one frame.
struct PendingCapture {
    name: String,
    tick: i64,
    wait_frames: u32,
    ready_frames: u32,
    ready_since: Option<Instant>,
    last_columns: usize,
    timed_out: bool,
    /// Set when the next submitted frame is the capture frame.
    render: bool,
}

const READY_HOLD: Duration = Duration::from_millis(250);
const CAPTURE_TIMEOUT_FRAMES: u32 = 2_000;
const SETTLE_TIMEOUT_TICKS: u32 = 1_200;
const JOIN_TIMEOUT: Duration = Duration::from_secs(180);
const FRAME_PACE: Duration = Duration::from_micros(16_667);

struct Jsonl(std::io::BufWriter<std::fs::File>);

impl Jsonl {
    fn create(path: &std::path::Path) -> Result<Jsonl, String> {
        std::fs::File::create(path)
            .map(|f| Jsonl(std::io::BufWriter::new(f)))
            .map_err(|e| format!("{}: {e}", path.display()))
    }
    fn write(&mut self, v: serde_json::Value) {
        let _ = writeln!(self.0, "{v}");
    }
    fn flush(&mut self) {
        let _ = self.0.flush();
    }
}

pub(super) struct Script {
    sc: Scenario,
    out: PathBuf,
    pub(super) width: u32,
    pub(super) height: u32,
    /// True while `script.rs` hands a synthesized event to `window_event`.
    pub(super) injecting: bool,
    phase: Phase,
    tick: i64,
    settle: u32,
    ready_ticks: u32,
    last_ready_columns: usize,
    step_index: usize,
    held: Vec<String>,
    /// `(yaw per tick, pitch per tick, ticks left)`.
    turns: Vec<(f32, f32, u32)>,
    capture_at_tail: Option<String>,
    capture: Option<PendingCapture>,
    captures: Vec<serde_json::Value>,
    /// A tick is due this frame (set at the frame head, consumed by the tick).
    tick_now: bool,
    clock: Option<Instant>,
    clock_ticks: u64,
    last_frame: Option<Instant>,
    started: Instant,
    /// `(teleports, local motions, explosion knockbacks)` last seen.
    seen: (u32, u32, u32),
    last_screen: String,
    status: String,
    positions: Jsonl,
    sounds: Jsonl,
    corrections: Jsonl,
    events: Jsonl,
    grabs_skipped: u32,
}

/// The scenario-wide knob the harness sets: `REWO_SCRIPT` names the
/// `puppet-scenario.json`, `REWO_SCRIPT_OUT` the run directory (default: the
/// scenario's own directory).
pub(super) fn load_script() -> Result<Option<Script>, String> {
    let Ok(path) = crate::knobs::var("REWO_SCRIPT") else {
        return Ok(None);
    };
    let path = PathBuf::from(path);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let sc = Scenario::parse(&text)?;
    let out = crate::knobs::var("REWO_SCRIPT_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| path.parent().map(PathBuf::from).unwrap_or_default());
    let size = |k: &str, d: u32| {
        crate::knobs::var(k)
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(d)
    };
    std::fs::create_dir_all(out.join("captures")).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut s = Script {
        width: size("REWO_SCRIPT_WIDTH", 1280),
        height: size("REWO_SCRIPT_HEIGHT", 720),
        injecting: false,
        phase: Phase::Joining,
        tick: -1,
        settle: 0,
        ready_ticks: 0,
        last_ready_columns: 0,
        step_index: 0,
        held: Vec::new(),
        turns: Vec::new(),
        capture_at_tail: None,
        capture: None,
        captures: Vec::new(),
        tick_now: false,
        clock: None,
        clock_ticks: 0,
        last_frame: None,
        started: Instant::now(),
        seen: (0, 0, 0),
        last_screen: String::new(),
        status: "running".into(),
        positions: Jsonl::create(&out.join("positions.jsonl"))?,
        sounds: Jsonl::create(&out.join("sounds.jsonl"))?,
        corrections: Jsonl::create(&out.join("corrections.jsonl"))?,
        events: Jsonl::create(&out.join("events.jsonl"))?,
        grabs_skipped: 0,
        out,
        sc,
    };
    let name = s.sc.name.clone();
    s.event(
        "start",
        serde_json::json!({ "scenario": name, "client": "rewo" }),
    );
    log::info!("script: {} ({} steps)", s.sc.name, s.sc.steps.len());
    Ok(Some(s))
}

impl Script {
    fn event(&mut self, kind: &str, data: serde_json::Value) {
        let mut o = match data {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        o.insert("event".into(), kind.into());
        o.insert("tick".into(), self.tick.into());
        o.insert(
            "ms".into(),
            (self.started.elapsed().as_millis() as u64).into(),
        );
        self.events.write(serde_json::Value::Object(o));
    }

    fn flush(&mut self) {
        self.positions.flush();
        self.sounds.flush();
        self.corrections.flush();
        self.events.flush();
    }

    /// Whether the script owns the tick clock (the running phase).
    pub(super) fn owns_clock(&self) -> bool {
        self.phase == Phase::Running
    }

    fn correction(
        &mut self,
        kind: &str,
        player: &rewo_world::physics::PlayerState,
    ) -> serde_json::Value {
        serde_json::json!({
            "tick": self.tick,
            "phase": self.phase.name(),
            "type": kind,
            "player_after": [player.x, player.y, player.z],
            "velocity": [player.vx, player.vy, player.vz],
        })
    }
}

/// `key` step names → the physical key Rewo's handler matches on.
fn key_code(name: &str) -> Option<KeyCode> {
    let n = name.to_ascii_lowercase();
    Some(match n.as_str() {
        "forward" | "w" => KeyCode::KeyW,
        "back" | "s" => KeyCode::KeyS,
        "left" | "a" => KeyCode::KeyA,
        "right" | "d" => KeyCode::KeyD,
        "jump" | "space" => KeyCode::Space,
        "sneak" | "shift" => KeyCode::ShiftLeft,
        "sprint" | "control" | "ctrl" => KeyCode::ControlLeft,
        "inventory" | "e" => KeyCode::KeyE,
        "escape" | "esc" => KeyCode::Escape,
        "drop" | "q" => KeyCode::KeyQ,
        "swap" | "swap_offhand" | "f" => KeyCode::KeyF,
        "chat" | "t" => KeyCode::KeyT,
        "command" | "slash" | "/" => KeyCode::Slash,
        "enter" | "return" => KeyCode::Enter,
        "tab" | "playerlist" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "up" => KeyCode::ArrowUp,
        "down" => KeyCode::ArrowDown,
        "page_left" | "arrow_left" => KeyCode::ArrowLeft,
        "page_right" | "arrow_right" => KeyCode::ArrowRight,
        "advancements" | "l" => KeyCode::KeyL,
        "f1" => KeyCode::F1,
        "f3" => KeyCode::F3,
        "f5" => KeyCode::F5,
        "1" => KeyCode::Digit1,
        "2" => KeyCode::Digit2,
        "3" => KeyCode::Digit3,
        "4" => KeyCode::Digit4,
        "5" => KeyCode::Digit5,
        "6" => KeyCode::Digit6,
        "7" => KeyCode::Digit7,
        "8" => KeyCode::Digit8,
        "9" => KeyCode::Digit9,
        _ => return None,
    })
}

/// The character a key types, for the screens that read `event.text`.
fn key_text(name: &str) -> Option<String> {
    let n = name.to_ascii_lowercase();
    match n.as_str() {
        "space" => Some(" ".into()),
        _ if n.len() == 1 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '/') => Some(n),
        _ => None,
    }
}

/// Spawned, no mesh in flight, nothing dirty, and every column within the
/// render distance minus one (euclidean) loaded.
fn world_ready(session: &PlaySession, pool: &MeshPool, render_distance: i32) -> bool {
    if !session.spawned || pool.in_flight() != 0 || session.dirty_len() != 0 {
        return false;
    }
    let r = (render_distance - 1).max(1);
    let cx = (session.player.x.floor() as i32) >> 4;
    let cz = (session.player.z.floor() as i32) >> 4;
    (-r..=r).all(|dx| {
        (-r..=r)
            .all(|dz| dx * dx + dz * dz > r * r || session.world.column(cx + dx, cz + dz).is_some())
    })
}

fn mouse_button(v: Option<&serde_json::Value>) -> MouseButton {
    match v.and_then(|b| b.as_str()) {
        Some("right") => MouseButton::Right,
        Some("middle") => MouseButton::Middle,
        _ => MouseButton::Left,
    }
}

impl LiveApp {
    /// Pace a scripted run's frames to ~60 fps (the puppet's `maxFps`).
    pub(super) fn script_pace(&mut self) {
        let Some(s) = self.script.as_mut() else {
            return;
        };
        let now = Instant::now();
        if let Some(last) = s.last_frame {
            let next = last + FRAME_PACE;
            if next > now {
                std::thread::sleep(next - now);
            }
        }
        s.last_frame = Some(Instant::now());
    }

    fn script_key(
        &mut self,
        event_loop: &ActiveEventLoop,
        code: KeyCode,
        pressed: bool,
        text: Option<String>,
    ) {
        self.key_input(
            event_loop,
            KeyInput {
                physical_key: PhysicalKey::Code(code),
                state: if pressed {
                    ElementState::Pressed
                } else {
                    ElementState::Released
                },
                text: if pressed { text } else { None },
                repeat: false,
            },
        );
    }

    fn script_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        if let Some(s) = self.script.as_mut() {
            s.injecting = true;
        }
        self.window_event(event_loop, WindowId::dummy(), event);
        if let Some(s) = self.script.as_mut() {
            s.injecting = false;
        }
    }

    fn script_mouse_button(
        &mut self,
        event_loop: &ActiveEventLoop,
        button: MouseButton,
        pressed: bool,
    ) {
        self.script_event(
            event_loop,
            WindowEvent::MouseInput {
                device_id: DeviceId::dummy(),
                state: if pressed {
                    ElementState::Pressed
                } else {
                    ElementState::Released
                },
                button,
            },
        );
    }

    /// Move the cursor to GUI coordinates `(x, y)` (the scenario's unit:
    /// vanilla's scaled mouse position).
    fn script_mouse_to(&mut self, event_loop: &ActiveEventLoop, v: &serde_json::Value) {
        let (gx, gy) = (
            v.get("x").and_then(|x| x.as_f64()).unwrap_or(0.0),
            v.get("y").and_then(|y| y.as_f64()).unwrap_or(0.0),
        );
        let scale = self
            .state
            .as_ref()
            .map(|s| s.window.inner_size())
            .map(|sz| rewo_gpu::hud::gui_scale(sz.width as f32, sz.height as f32) as f64)
            .unwrap_or(1.0);
        self.script_event(
            event_loop,
            WindowEvent::CursorMoved {
                device_id: DeviceId::dummy(),
                position: PhysicalPosition::new(gx * scale, gy * scale),
            },
        );
    }

    /// Hold or release one managed key (`hold` / `release`).
    fn script_hold(&mut self, event_loop: &ActiveEventLoop, name: &str, down: bool) {
        match name {
            "attack" => self.script_mouse_button(event_loop, MouseButton::Left, down),
            "use" => self.script_mouse_button(event_loop, MouseButton::Right, down),
            _ => match key_code(name) {
                Some(code) => self.script_key(event_loop, code, down, None),
                None => log::warn!("script: unknown key '{name}'"),
            },
        }
    }

    /// One step, at the head of its tick.
    fn script_apply(&mut self, event_loop: &ActiveEventLoop, step: &serde_json::Value) {
        for k in strings(step.get("hold")) {
            let new = self.script.as_mut().is_some_and(|s| {
                if s.held.contains(&k) {
                    false
                } else {
                    s.held.push(k.clone());
                    true
                }
            });
            if new {
                self.script_hold(event_loop, &k, true);
            }
        }
        let release = strings(step.get("release"));
        if !release.is_empty() {
            let held = self
                .script
                .as_ref()
                .map(|s| s.held.clone())
                .unwrap_or_default();
            let gone: Vec<String> = if release.iter().any(|r| r == "all") {
                held
            } else {
                held.into_iter().filter(|h| release.contains(h)).collect()
            };
            for k in &gone {
                self.script_hold(event_loop, k, false);
            }
            if let Some(s) = self.script.as_mut() {
                s.held.retain(|h| !gone.contains(h));
            }
        }
        if let (Some(look), Some(session)) = (step.get("look"), self.session.as_mut()) {
            if let Some(y) = look.get("yaw").and_then(|v| v.as_f64()) {
                session.player.yaw = y as f32;
            }
            if let Some(p) = look.get("pitch").and_then(|v| v.as_f64()) {
                session.player.pitch = p as f32;
            }
        }
        if let Some(turn) = step.get("turn") {
            let ticks = turn
                .get("ticks")
                .and_then(|t| t.as_u64())
                .unwrap_or(1)
                .max(1) as u32;
            let yaw = turn.get("yaw").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let pitch = turn.get("pitch").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            if let Some(s) = self.script.as_mut() {
                s.turns
                    .push((yaw / ticks as f32, pitch / ticks as f32, ticks));
            }
        }
        for cmd in strings(step.get("command")) {
            self.script_command(&cmd);
        }
        for msg in strings(step.get("chat")) {
            if let Some(session) = self.session.as_mut() {
                let _ = session.send_chat(&msg);
            }
            if let Some(s) = self.script.as_mut() {
                s.event("chat", serde_json::json!({ "message": msg }));
            }
        }
        for what in strings(step.get("click")) {
            let b = if what == "use" {
                MouseButton::Right
            } else {
                MouseButton::Left
            };
            self.script_note_target(&what);
            self.script_mouse_button(event_loop, b, true);
            // A click is a press and a release, unless the key is held.
            let held = self.script.as_ref().is_some_and(|s| s.held.contains(&what));
            if !held {
                self.script_mouse_button(event_loop, b, false);
            }
        }
        if let Some(slot) = step.get("hotbar").and_then(|h| h.as_u64()) {
            let name = slot.clamp(1, 9).to_string();
            if let Some(code) = key_code(&name) {
                self.script_key(event_loop, code, true, None);
                self.script_key(event_loop, code, false, None);
            }
        }
        // GUI steps (phase 3): a key tap, the cursor, a click and the wheel.
        for name in strings(step.get("key")) {
            match key_code(&name) {
                Some(code) => {
                    self.script_key(event_loop, code, true, key_text(&name));
                    self.script_key(event_loop, code, false, None);
                }
                None => log::warn!("script: unknown key '{name}'"),
            }
        }
        if let Some(v) = step.get("mouse") {
            self.script_mouse_to(event_loop, v);
        }
        if let Some(v) = step.get("mouse_click") {
            self.script_mouse_to(event_loop, v);
            let b = mouse_button(v.get("button"));
            self.script_mouse_button(event_loop, b, true);
            self.script_mouse_button(event_loop, b, false);
        }
        if let Some(v) = step.get("mouse_scroll") {
            self.script_mouse_to(event_loop, v);
            let dy = v.get("dy").and_then(|d| d.as_f64()).unwrap_or(-1.0) as f32;
            self.script_event(
                event_loop,
                WindowEvent::MouseWheel {
                    device_id: DeviceId::dummy(),
                    delta: MouseScrollDelta::LineDelta(0.0, dy),
                    phase: TouchPhase::Moved,
                },
            );
        }
        if let Some(name) = step.get("capture").and_then(|c| c.as_str()) {
            if let Some(s) = self.script.as_mut() {
                if s.capture_at_tail.is_some() {
                    s.event("capture_dropped", serde_json::json!({ "name": name }));
                } else {
                    s.capture_at_tail = Some(name.to_string());
                }
            }
        }
        if let Some(label) = step.get("mark").and_then(|m| m.as_str()) {
            if let Some(s) = self.script.as_mut() {
                s.event("mark", serde_json::json!({ "label": label }));
            }
        }
    }

    /// `events.jsonl` gets what a click is aimed at (the crosshair's block,
    /// as the use and attack paths pick it), so a scenario that misses shows it.
    fn script_note_target(&mut self, what: &str) {
        let target = self.session.as_ref().and_then(|session| {
            let hit = session.target_shape(
                eye_f64(session),
                look_dir(session.player.yaw, session.player.pitch),
                block_reach(session),
            )?;
            let b = hit.hit.block;
            let state = session.world.block_state_at(b[0], b[1], b[2]);
            Some(serde_json::json!({
                "block": b,
                "state": state,
                "name": self.blocks.block_name(state),
                "face": format!("{:?}", hit.hit.face),
                "distance": hit.hit.distance,
            }))
        });
        if let Some(s) = self.script.as_mut() {
            s.event(
                "click",
                serde_json::json!({ "what": what, "target": target }),
            );
        }
    }

    fn script_command(&mut self, cmd: &str) {
        let c = cmd.strip_prefix('/').unwrap_or(cmd).to_string();
        if let Some(session) = self.session.as_mut() {
            if let Err(e) = session.send_command(&c) {
                log::warn!("script: command '{c}': {e}");
            }
        }
        if let Some(s) = self.script.as_mut() {
            s.event("command", serde_json::json!({ "command": c }));
        }
    }

    /// The head of a scripted frame, before anything else in `frame`:
    /// capture readiness, then (running, no capture waiting, a tick due)
    /// the steps for the coming tick.
    pub(super) fn script_frame_head(&mut self, event_loop: &ActiveEventLoop) {
        let Some(s) = self.script.as_ref() else {
            return;
        };
        if s.phase == Phase::Done {
            return;
        }
        if s.phase == Phase::Joining && s.started.elapsed() > JOIN_TIMEOUT {
            self.script_finish(event_loop, "join_timeout");
            return;
        }
        if self.session.as_ref().is_none_or(|s| s.disconnect.is_some()) {
            if let Some(reason) = self.session.as_ref().and_then(|s| s.disconnect.clone()) {
                log::error!("script: disconnected: {reason}");
            }
            self.script_finish(event_loop, "disconnected");
            return;
        }
        // A capture waits for a ready world, held for 250 ms and three frames.
        let ready = self.script_world_ready();
        let columns = self
            .session
            .as_ref()
            .map_or(0, |s| s.world.loaded_columns());
        if let Some(c) = self.script.as_mut().and_then(|s| s.capture.as_mut()) {
            if !c.render {
                c.wait_frames += 1;
                let stable = ready && columns == c.last_columns;
                c.last_columns = columns;
                if stable {
                    if c.ready_frames == 0 {
                        c.ready_since = Some(Instant::now());
                    }
                    c.ready_frames += 1;
                } else {
                    c.ready_frames = 0;
                }
                let held =
                    c.ready_frames >= 3 && c.ready_since.is_some_and(|t| t.elapsed() >= READY_HOLD);
                c.timed_out = c.wait_frames >= CAPTURE_TIMEOUT_FRAMES;
                if held || c.timed_out {
                    c.render = true;
                }
            }
            return;
        }
        let s = self.script.as_mut().expect("checked above");
        if s.phase != Phase::Running {
            return;
        }
        // One tick per 50 ms of wall time, counted from the first running frame.
        let now = Instant::now();
        let start = *s.clock.get_or_insert(now);
        let due = (now.duration_since(start).as_secs_f64() / TICK_DT as f64) as u64;
        if due < s.clock_ticks {
            return;
        }
        // Never fall more than a tick behind (a long capture wait).
        s.clock_ticks = s.clock_ticks.max(due.saturating_sub(1)) + 1;
        if s.tick > s.sc.end_tick {
            self.script_finish(event_loop, "ok");
            return;
        }
        let tick = s.tick;
        let mut steps = Vec::new();
        while s.step_index < s.sc.steps.len()
            && s.sc.steps[s.step_index]["at"].as_i64().unwrap_or(0) <= tick
        {
            steps.push(s.sc.steps[s.step_index].clone());
            s.step_index += 1;
        }
        s.tick_now = true;
        for step in &steps {
            self.script_apply(event_loop, step);
        }
        // Turns after the steps, so a turn starts on its own tick (the puppet's order).
        let turns: Vec<(f32, f32)> = match self.script.as_mut() {
            Some(s) => {
                let now = s.turns.iter().map(|t| (t.0, t.1)).collect();
                for t in s.turns.iter_mut() {
                    t.2 -= 1;
                }
                s.turns.retain(|t| t.2 > 0);
                now
            }
            None => Vec::new(),
        };
        if let Some(session) = self.session.as_mut() {
            for (dy, dp) in turns {
                session.player.yaw += dy;
                session.player.pitch = (session.player.pitch + dp).clamp(-90.0, 90.0);
            }
        }
        self.script_note_screen();
    }

    /// The world a capture waits for: spawned, no mesh in flight, nothing
    /// dirty, the columns around the player loaded. Unlike settling, an open
    /// screen does not count against it (a capture of a screen must come back).
    fn script_world_ready(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| world_ready(s, &self.pool, self.options.render_distance))
    }

    fn script_note_screen(&mut self) {
        let name = if self.chat_screen.is_some() {
            "chat".to_string()
        } else if self.screen.inventory_open() {
            match self.session.as_ref().and_then(|s| s.menus.open()) {
                Some(m) => format!("container:{}", m.layout.name),
                None => "inventory".to_string(),
            }
        } else if let Some(sc) = self.screen.screens.current() {
            format!("{:?}", sc.kind)
        } else {
            "none".to_string()
        };
        if let Some(s) = self.script.as_mut() {
            if s.last_screen != name {
                s.last_screen = name.clone();
                s.event("screen", serde_json::json!({ "screen": name }));
            }
        }
    }

    /// How many ticks `tick_session` runs this frame when the script owns the
    /// clock: `Some(0)` or `Some(1)`. `None` leaves the normal accumulator.
    pub(super) fn script_ticks(&mut self) -> Option<u32> {
        let s = self.script.as_mut()?;
        if !s.owns_clock() {
            return None;
        }
        Some(u32::from(std::mem::take(&mut s.tick_now)))
    }

    /// After `session.pump`: server corrections addressed to the player.
    pub(super) fn script_after_pump(&mut self, session: &PlaySession) {
        let Some(s) = self.script.as_mut() else {
            return;
        };
        let now = (
            session.teleports,
            session.motion_stats.local_motions,
            session.motion_stats.explosion_knockbacks,
        );
        let kinds = [
            (now.0.saturating_sub(s.seen.0), "teleport"),
            (now.1.saturating_sub(s.seen.1), "velocity"),
            (now.2.saturating_sub(s.seen.2), "explosion_knockback"),
        ];
        s.seen = now;
        for (n, kind) in kinds {
            for _ in 0..n {
                let row = s.correction(kind, &session.player);
                s.corrections.write(row);
            }
        }
    }

    /// The tail of one client tick.
    pub(super) fn script_after_tick(&mut self, session: &mut PlaySession) {
        // Settling also wants no screen up (the puppet's `worldReady`).
        let ready = world_ready(session, &self.pool, self.options.render_distance)
            && !self.screen.any_open()
            && self.chat_screen.is_none();
        let Some(s) = self.script.as_mut() else {
            return;
        };
        // The sounds this tick (and the frames since the last one) played.
        let played = self
            .sounds
            .system
            .engine
            .play_log
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default();
        for rec in played {
            let i = &rec.instance;
            let result = match rec.result {
                rewo_net::sound_engine::PlayResult::Started => "started",
                rewo_net::sound_engine::PlayResult::StartedSilently => "started_silently",
                rewo_net::sound_engine::PlayResult::NotStarted(_) => "not_started",
            };
            s.sounds.write(serde_json::json!({
                "tick": s.tick,
                "phase": s.phase.name(),
                "id": i.identifier,
                "source": i.source.name(),
                "volume": i.volume,
                "pitch": i.pitch,
                "x": i.x,
                "y": i.y,
                "z": i.z,
                "relative": i.relative,
                "attenuation": match i.attenuation {
                    rewo_net::sound_instance::Attenuation::None => "none",
                    _ => "linear",
                },
                "resolved": rec.resolved,
                "result": result,
            }));
        }
        match s.phase {
            Phase::Joining => {
                if session.spawned {
                    let p = &session.player;
                    let pos = serde_json::json!({ "position": [p.x, p.y, p.z], "yaw": p.yaw, "pitch": p.pitch });
                    s.event("joined", pos);
                    let cmds = s.sc.setup_commands.clone();
                    for c in cmds {
                        let c = c.strip_prefix('/').unwrap_or(&c).to_string();
                        if let Err(e) = session.send_command(&c) {
                            log::warn!("script: setup '{c}': {e}");
                        }
                        s.event("command", serde_json::json!({ "command": c }));
                    }
                    s.phase = Phase::Settling;
                }
            }
            Phase::Settling => {
                s.settle += 1;
                if s.settle == s.sc.late_after {
                    let cmds = s.sc.late_commands.clone();
                    for c in cmds {
                        let c = c.strip_prefix('/').unwrap_or(&c).to_string();
                        let _ = session.send_command(&c);
                        s.event("command", serde_json::json!({ "command": c }));
                    }
                }
                let columns = session.world.loaded_columns();
                let stable = ready && columns == s.last_ready_columns;
                s.last_ready_columns = columns;
                s.ready_ticks = if stable { s.ready_ticks + 1 } else { 0 };
                let settled =
                    s.settle >= s.sc.settle_ticks.max(s.sc.late_after + 10) && s.ready_ticks >= 20;
                if settled || s.settle >= SETTLE_TIMEOUT_TICKS {
                    if !settled {
                        let n = s.settle;
                        s.event("settle_timeout", serde_json::json!({ "ticks": n }));
                    }
                    // Tick 0: the chat and the held-item popup setup left
                    // behind are cleared, as the puppet does. (The puppet
                    // also clears particles and reseeds the client randoms;
                    // Rewo has no seam for either, so a Rewo run is
                    // reproducible only where those do not show.)
                    if s.sc.clear_chat {
                        session.chat.clear_messages(false);
                        session.chat_log.clear();
                    }
                    self.tool_highlight.timer = 0;
                    s.phase = Phase::Running;
                    s.tick = 0;
                    let p = &session.player;
                    let pos = serde_json::json!({ "position": [p.x, p.y, p.z], "yaw": p.yaw, "pitch": p.pitch });
                    s.event("tick0", pos);
                    // The first running tick comes from the script's clock
                    // (`tick_session` stops its accumulator loop here).
                    self.tick_accum = 0.0;
                }
            }
            Phase::Running => {
                let p = &session.player;
                s.positions.write(serde_json::json!({
                    "tick": s.tick,
                    "x": p.x, "y": p.y, "z": p.z,
                    "yaw": p.yaw, "pitch": p.pitch,
                    "vx": p.vx, "vy": p.vy, "vz": p.vz,
                    "on_ground": p.on_ground,
                    "sprinting": self.keys.sprint && self.keys.w,
                    "crouching": p.crouching,
                    "mounted": session.is_mounted(),
                    // The state fields the puppet logs beside the position.
                    "gliding": p.pose == rewo_world::physics::Pose::FallFlying,
                    "health": session.health,
                    "food": session.food,
                    "using_item": session.local_use_state().using,
                    "selected": session.inventory.selected(),
                }));
                if let Some(name) = s.capture_at_tail.take() {
                    s.capture = Some(PendingCapture {
                        name,
                        tick: s.tick,
                        wait_frames: 0,
                        ready_frames: 0,
                        ready_since: None,
                        last_columns: usize::MAX,
                        timed_out: false,
                        render: false,
                    });
                }
                s.flush();
                s.tick += 1;
            }
            Phase::Done => {}
        }
    }

    /// End the run: release keys, write `summary.json`, exit the loop.
    pub(super) fn script_finish(&mut self, event_loop: &ActiveEventLoop, status: &str) {
        let held = self
            .script
            .as_ref()
            .map(|s| s.held.clone())
            .unwrap_or_default();
        for k in held {
            self.script_hold(event_loop, &k, false);
        }
        let device = self
            .state
            .as_ref()
            .map(|st| st.gpu.device_name.clone())
            .unwrap_or_default();
        let validation = self.want_validation;
        let Some(s) = self.script.as_mut() else {
            return;
        };
        if s.phase == Phase::Done {
            return;
        }
        s.status = status.to_string();
        s.phase = Phase::Done;
        let tick = s.tick;
        s.event(
            "finish",
            serde_json::json!({ "status": status, "tick": tick.to_string() }),
        );
        s.flush();
        let summary = serde_json::json!({
            "scenario": s.sc.name,
            "client": "rewo",
            "status": s.status,
            "ticks": s.tick,
            "end_tick": s.sc.end_tick,
            "wall_ms": s.started.elapsed().as_millis() as u64,
            "minecraft": "26.2",
            "captures": s.captures,
            "window": {
                "hidden": true,
                "show_window_skipped": 0,
                "grab_attempts_blocked": s.grabs_skipped,
                "focus_events": [],
            },
            "gpu": { "name": device, "backend": "vulkan", "validation": validation },
        });
        let path = s.out.join("summary.json");
        if let Err(e) = std::fs::write(
            &path,
            serde_json::to_string_pretty(&summary).unwrap_or_default(),
        ) {
            log::error!("script: {}: {e}", path.display());
        }
        log::info!("script: finished ({status}) at tick {tick}");
        event_loop.exit();
    }
}

impl Script {
    /// In `submit`, after every `set_*` of the frame: the capture frame.
    pub(super) fn capture_frame(
        &mut self,
        gpu: &mut rewo_gpu::Gpu,
        world: &mut rewo_gpu::world::WorldRenderer,
        view_proj: [[f32; 4]; 4],
        draw: &OverlayDraw,
        format: vk::Format,
        extent: vk::Extent2D,
    ) {
        let s = self;
        if !s.capture.as_ref().is_some_and(|c| c.render) {
            return;
        }
        let c = s.capture.take().expect("checked above");
        let file = s.out.join("captures").join(format!("{}.png", c.name));
        let res = (|| -> Result<(), String> {
            let mut off = Offscreen::with_format(gpu, extent.width, extent.height, format)?;
            let r = off
                .render(gpu, Some((world, view_proj)), draw, [0.0, 0.0, 0.0, 1.0])
                .and_then(|()| off.save_png(gpu, &file));
            off.destroy(gpu);
            r
        })();
        let rec = serde_json::json!({
            "name": c.name,
            "tick": c.tick,
            "file": format!("captures/{}.png", c.name),
            "width": extent.width,
            "height": extent.height,
            "wait_frames": c.wait_frames,
            "timed_out": c.timed_out,
        });
        match res {
            Ok(()) => {
                s.captures.push(rec.clone());
                s.event("capture", rec);
            }
            Err(e) => {
                log::error!("script: capture {}: {e}", c.name);
                s.event(
                    "capture_error",
                    serde_json::json!({ "name": c.name, "error": e }),
                );
            }
        }
        s.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_sort_stably_and_end_tick_defaults_past_the_last() {
        let sc = Scenario::parse(
            r#"{"name":"t","setup":{"commands":["tp @s 0 0 0"],"late_after":3},
                "steps":[{"at":5,"capture":"b"},{"at":2,"click":"use"},{"at":5,"mark":"c"}]}"#,
        )
        .unwrap();
        let order: Vec<i64> = sc.steps.iter().map(|s| s["at"].as_i64().unwrap()).collect();
        assert_eq!(order, [2, 5, 5]);
        assert_eq!(sc.steps[1]["capture"], "b");
        assert_eq!(sc.steps[2]["mark"], "c");
        assert_eq!(sc.end_tick, 6);
        assert_eq!(sc.late_after, 3);
        assert_eq!(sc.settle_ticks, 40);
        assert_eq!(sc.setup_commands, ["tp @s 0 0 0"]);
        assert!(sc.clear_chat);
    }

    #[test]
    fn a_step_without_a_tick_is_refused() {
        assert!(Scenario::parse(r#"{"name":"t","steps":[{"click":"use"}]}"#).is_err());
    }

    #[test]
    fn key_names_cover_every_managed_key() {
        for k in [
            "forward", "back", "left", "right", "jump", "sneak", "sprint",
        ] {
            assert!(key_code(k).is_some(), "{k}");
        }
        assert_eq!(key_code("inventory"), Some(KeyCode::KeyE));
        assert_eq!(key_code("Escape"), Some(KeyCode::Escape));
        assert_eq!(key_code("9"), Some(KeyCode::Digit9));
        assert_eq!(key_code("nonsense"), None);
        assert_eq!(key_text("e").as_deref(), Some("e"));
        assert_eq!(key_text("escape"), None);
    }
}
