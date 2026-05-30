//! App-level state types: which screen is active, intent enums, etc.

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Hash)]
pub enum Screen {
    #[default]
    MainMenu,
    Instances,
    /// Phase H5 — chickenedin friend graph: see who's online, send/accept
    /// requests, remove friends. Visible only when the active account
    /// has a launcher-linked social_token; otherwise the tab still
    /// renders but the screen shows a "link your launcher" affordance.
    Friends,
    Settings,
    Launching,
}

impl Screen {
    /// All screens in tab-bar order.
    pub const fn all() -> &'static [Screen] {
        &[
            Screen::MainMenu,
            Screen::Instances,
            Screen::Friends,
            Screen::Settings,
            Screen::Launching,
        ]
    }

    /// Uppercase tab label (matches `.state-picker button` text-transform: uppercase).
    pub const fn tab_label(&self) -> &'static str {
        match self {
            Screen::MainMenu => "MAIN MENU",
            Screen::Instances => "INSTANCES",
            Screen::Friends => "FRIENDS",
            Screen::Settings => "SETTINGS",
            Screen::Launching => "LAUNCHING",
        }
    }
}
