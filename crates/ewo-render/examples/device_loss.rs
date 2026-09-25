//! Exercise GPU device-loss recovery on the real backend.
//!
//! Opens a window, renders, removes the D3D12 device after 120 frames
//! (`EWO_SIMULATE_DEVICE_LOSS`, debug builds only), and checks that rendering
//! resumes on a recreated device. Exits 0 on recovery, 1 otherwise.
//!
//!     cargo run -p ewo-render --example device_loss

use std::sync::Arc;
use std::time::{Duration, Instant};

use ewo_render::gl_backend::{gpu_generation, GlBackend};
use skia_safe::Color;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

struct App {
    window: Option<Arc<Window>>,
    backend: Option<GlBackend>,
    started: Instant,
    frames_after_recovery: u32,
    result: Option<bool>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        let mut attrs = Window::default_attributes()
            .with_title("device loss test")
            .with_inner_size(winit::dpi::LogicalSize::new(400, 300));
        #[cfg(windows)]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs = attrs.with_no_redirection_bitmap(true);
        }
        let window = Arc::new(el.create_window(attrs).expect("window"));
        self.backend = Some(GlBackend::new(el, window.clone()));
        self.window = Some(window);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if let WindowEvent::RedrawRequested = event {
            let generation = gpu_generation();
            if let Some(b) = self.backend.as_mut() {
                b.render(|canvas, _, _| {
                    canvas.clear(Color::from_argb(255, 40, 0, 30));
                });
            }
            if generation > 0 {
                self.frames_after_recovery += 1;
            }
            if self.frames_after_recovery >= 120 {
                self.result = Some(true);
                el.exit();
            } else if self.started.elapsed() > Duration::from_secs(30) {
                self.result = Some(false);
                el.exit();
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    if std::env::var_os("EWO_SIMULATE_DEVICE_LOSS").is_none() {
        std::env::set_var("EWO_SIMULATE_DEVICE_LOSS", "120");
    }
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        window: None,
        backend: None,
        started: Instant::now(),
        frames_after_recovery: 0,
        result: None,
    };
    event_loop.run_app(&mut app).expect("run");
    match app.result {
        Some(true) => {
            println!("RECOVERED: generation {}, 120 frames rendered after recreation", gpu_generation());
        }
        _ => {
            println!("NOT RECOVERED within 30 s (generation {})", gpu_generation());
            std::process::exit(1);
        }
    }
}
