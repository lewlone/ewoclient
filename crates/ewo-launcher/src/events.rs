//! The launcher window's event handlers, one module per `WindowEvent` arm.
//! `App::window_event` (`main.rs`) is the dispatcher; each handler is an
//! `App` method defined here.
//!
//! - `click`: left mouse button, layered (window buttons, dev overlay,
//!   modals, then the screens), applying the pure decisions in `hit`
//! - `hover`: cursor move, hover state and the mouse cursor icon
//! - `keys`: key presses, routed to one focus owner
//! - `wheel`: scrolling
//! - `redraw`: one frame, in phases (services, UI requests, animation,
//!   views, draw)
//! - `widgets`: the per-screen slider/hover drivers and press routers the
//!   click and hover handlers share
//! - `hit`: pure "what was hit" rules, no `App`

use super::*;

mod click;
mod hit;
mod hover;
mod keys;
mod redraw;
#[cfg(test)]
mod test_support;
mod wheel;
mod widgets;
use widgets::*;
