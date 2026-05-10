//! Screens — composed UI surfaces drawn on top of the backdrop.
//!
//! Build-sequence step 7+ scaffold. Currently houses the main menu only;
//! other screens (instances, settings tabs, launching) land in step 13.
//! When `ewo-ui` widgets land (step 10+), most of this moves there — for now
//! the screens are pure-text static compositions in `ewo-render` so they ship
//! ahead of the widget pipeline.

pub mod about_modal;
pub mod dev_overlay;
pub mod instances;
pub mod launching;
pub mod main_menu;
pub mod new_instance_modal;
pub mod settings;
pub mod tab_bar;

pub use about_modal::{draw_modal as draw_about_modal, AboutModalState};
pub use dev_overlay::{draw_dev_overlay, DevOverlayState, FrameStats, Slot as DevSlot};
pub use instances::{draw_instances, InstancePrefs, Slot as InstanceSlot};
pub use launching::{draw_launching, LaunchError, LaunchingState, RealSeverity};
pub use main_menu::draw_main_menu;
pub use new_instance_modal::{
    draw_modal as draw_new_instance_modal, NewInstanceModalState, Slot as ModalSlot,
};
pub use settings::{draw_settings, Prefs, SettingsConfig, SettingsTab, Slot as SettingsSlot};
pub use tab_bar::{draw_tab_bar, tab_bounds};
