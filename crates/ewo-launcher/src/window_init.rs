//! Window creation: runs on `resumed`. Builds the borderless transparent
//! window, the render backend, backdrop and fonts, and warms the JRE scan.
//! Moved out of `main.rs` unchanged.

use super::*;

impl App {
    /// Create the launcher window and everything that renders into it.
    pub(crate) fn create_window(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("EwoClient")
            .with_decorations(false)
            // Per-pixel alpha: the rounded card floats with transparent corners
            // (the desktop shows through) instead of an opaque black margin +
            // painted bevel.
            .with_transparent(true)
            .with_inner_size(LogicalSize::new(1180.0, 720.0))
            .with_min_inner_size(LogicalSize::new(800.0, 520.0));

        // Windows: create the window WITHOUT a redirection bitmap. The render
        // backend presents through a DirectComposition swapchain (see
        // `ewo-render::gl_backend`), so there must be no opaque GDI/GL
        // redirection surface behind it — otherwise the transparent corners
        // composite against that surface (black) instead of the desktop. This
        // MUST be set at creation time; it can't be added afterwards.
        #[cfg(target_os = "windows")]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs.with_no_redirection_bitmap(true)
        };

        // Runtime taskbar/alt-tab icon for the borderless window (the
        // embedded .ico serves Explorer only). None → OS default icon;
        // a missing asset never blocks startup.
        let attrs = match window::icon::load_app_icon() {
            Some(icon) => attrs.with_window_icon(Some(icon)),
            None => {
                log::warn!("app icon not found — taskbar shows the default icon");
                attrs
            }
        };

        let win = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );

        window::configure(&win);

        // Warm the JRE-detection cache on a background thread. `pick_jre` on
        // the Launch path calls `detect_all`, which shells `java -version` at
        // every installed JDK — a 1-3s scan that, run cold on the UI thread at
        // click time, froze the whole render loop. Kicking it off at window
        // creation means the `OnceLock` cache is populated seconds before the
        // user can reach the Launch button, so the click resolves instantly.
        // (CREATE_NO_WINDOW — see `launch::no_window` — keeps the scan's
        // subprocesses from flashing consoles here too.)
        // The scan's side product, the label strings for the per-instance
        // Java-runtime dropdown, comes back over a channel so the UI never
        // calls `labels()` synchronously either.
        let (jre_tx, jre_rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("ewo-jre-warm".into())
            .spawn(move || {
                let n = launch::detect_jres().len();
                log::info!("jre warm: {n} runtime(s) detected (cache primed)");
                let _ = jre_tx.send(launch::jre::labels());
            })
            .expect("spawn jre-warm thread");
        self.jre_labels_rx = Some(jre_rx);

        let backend = GlBackend::new(event_loop, win.clone());
        // Backdrop particle pools + layout sizes live in logical pixels —
        // the same coord space `draw_frame` paints in. Inputs to
        // `card_content_size` are logical, so divide physical by the DPI
        // scale factor up-front. On 100% scale they're equal; on HiDPI
        // (e.g. 125% on 1440p) they differ — and a physical-sized backdrop
        // would put the particles in a coord space the renderer doesn't
        // share, throwing off both density + bounds.
        let size = win.inner_size();
        let scale = win.scale_factor() as f32;
        let logical_w = ((size.width as f32) / scale) as u32;
        let logical_h = ((size.height as f32) / scale) as u32;
        let (card_w, card_h) = app_window::card_content_size(logical_w, logical_h);
        let backdrop = Backdrop::new(card_w, card_h, &self.settings);
        let fonts = FontStore::new();

        win.request_redraw();

        self.window = Some(win);
        self.backend = Some(backend);
        self.backdrop = Some(backdrop);
        self.fonts = Some(fonts);

        // Initialize per-mod toggle state from the now-built instance list.
        self.instance_prefs.sync_mods(&self.instances);

        // Apply the persisted VSync preference to the GL backend that
        // just came online. The backend defaults to vsync-on, so this is
        // only meaningful when the user had it off last session.
        if let Some(b) = self.backend.as_ref() {
            b.set_vsync(self.prefs.vsync.on);
        }

        if self.dev {
            log::info!("dev overlay enabled");
        }
    }
}
