//! Widgets — interactive UI primitives. Will move to `ewo-ui` when the
//! widget set grows; lives in `ewo-render` for now so the launcher can call
//! through a single dependency while the UI crate is empty.
//!
//! Step 10 introduces this module with `vbtn`. Subsequent steps add
//! `vslider`, `vdrop`, `vstatus`, `pbar`, `toggle`, `pathfield`, etc.

pub mod glass_panel;
pub mod liquid_glass;
pub mod meta_pill;
pub mod pbar;
pub mod scrollbar;
pub mod vbtn;
pub mod vdrop;
pub mod vghost_btn;
pub mod vpathfield;
pub mod vslider;
pub mod vstatus;
pub mod vtoggle;

pub use glass_panel::{draw_glass_panel, PANEL_RADIUS};
pub use liquid_glass::{
    draw_liquid_glass, Backdrop as GlassBackdrop, Params as LiquidGlassParams,
};
pub use meta_pill::{draw_meta_pill, draw_meta_pill_row, meta_pill_size};
pub use pbar::{draw_pbar, PbarState};
pub use scrollbar::draw_scrollbar;
pub use vbtn::{VbtnState, draw_vbtn};
pub use vdrop::{
    draw_vdrop_head, draw_vdrop_menu, menu_content_height, menu_layout, menu_max_scroll,
    VdropState,
};
pub use vghost_btn::{draw_vghost_btn, GhostKind, VghostBtnState};
pub use vpathfield::{draw_vpathfield, VpathfieldState};
pub use vslider::{draw_vslider, VsliderState};
pub use vstatus::{draw_vstatus, VstatusState};
pub use vtoggle::{draw_vtoggle, VtoggleState, TOGGLE_H, TOGGLE_W};
