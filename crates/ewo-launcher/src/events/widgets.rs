//! Per-screen widget input drivers (continuous drag/hover) and press handlers.
//! The click dispatcher in `events::click` calls these in a fixed order.

pub(crate) mod dev_overlay;
pub(crate) mod instances;
pub(crate) mod modal;
pub(crate) mod settings;

pub(crate) use dev_overlay::*;
pub(crate) use instances::*;
pub(crate) use modal::*;
pub(crate) use settings::*;
