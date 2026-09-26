//! The desktop clipboard — vanilla's `KeyboardHandler.getClipboard` /
//! `setClipboard`, which read and write the system clipboard through GLFW.
//!
//! The edit boxes keep their own `&mut String` clipboard (`rewo_world::edit_box`);
//! the live client syncs that buffer with this one: the system text is pulled
//! in when Ctrl+V is pressed, and a buffer an in-game copy/cut changed is
//! pushed out. An unavailable clipboard reads as nothing and writes are
//! dropped, which is how GLFW's behaves too.

use std::cell::RefCell;

thread_local! {
    /// One handle for the life of the thread: on X11 the owner of copied
    /// text must stay alive for other programs to paste it.
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut arboard::Clipboard) -> Option<R>) -> Option<R> {
    CLIPBOARD.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            *c = arboard::Clipboard::new()
                .map_err(|e| log::warn!("clipboard unavailable: {e}"))
                .ok();
        }
        c.as_mut().and_then(f)
    })
}

/// The system clipboard's text, if it holds any.
pub(crate) fn read() -> Option<String> {
    with(|c| c.get_text().ok())
}

/// Put `text` on the system clipboard.
pub(crate) fn write(text: &str) {
    with(|c| c.set_text(text.to_owned()).ok());
}
