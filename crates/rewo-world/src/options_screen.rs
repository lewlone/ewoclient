//! The options screens (M157) — `OptionsList`'s geometry and the two
//! sub-screens Rewo's two options live on.
//!
//! # The list is not the layout system
//!
//! Every other screen in Rewo lays out through `Grid`/`RowHelper`
//! ([`crate::layout`]). `OptionsList` does **not**: it is a
//! `ContainerObjectSelectionList` whose entries position their own widgets from
//! literals — `x = screen.width / 2 - 155` and `xOffset += 160` per widget
//! (`OptionsList.java:158-165`), on a fixed 25-px row pitch (`:17-22`). So the
//! two columns are 160 apart on a 310-wide band and the whole thing is centred
//! on the SCREEN rather than on the list's own width.
//!
//! Reaching for `Grid` here would produce a plausible two-column layout that
//! drifts from vanilla's the moment the window is not the width the test used.
//!
//! # What is here and what is not
//!
//! Rewo has two options ([`rewo_net::options`]), and they live on two different
//! vanilla sub-screens: `musicFrequency` on `SoundOptionsScreen` (`:23`) and
//! `hideLightningFlashes` on `AccessibilityOptionsScreen`. Both are rendered
//! as **cycle buttons**, which is `OptionInstance.Enum`'s widget.
//!
//! Vanilla's sliders are absent because neither of Rewo's options is one. That
//! is a scope statement rather than a gap being papered over: the volume
//! sliders it would need are `Options.soundSourceVolumes`, which Rewo already
//! models as `CategoryVolumes` and does not yet surface.

use crate::screen::{Screen, ScreenKind, Widget, WidgetId};

/// `OptionsList` row pitch — `DEFAULT_ITEM_HEIGHT` (`OptionsList.java:17`).
pub const ROW_HEIGHT: i32 = 25;
/// The band the two columns sit in — `BIG_BUTTON_WIDTH` (`:16`).
pub const BAND_WIDTH: i32 = 310;
/// Half of it, minus a hair: the literal at `:159` is `width / 2 - 155`.
pub const BAND_HALF: i32 = 155;
/// `xOffset += 160` (`:164`) — the column pitch, which is **not**
/// `BAND_WIDTH / 2`. The two differ by five pixels and the gap is real.
pub const COLUMN_PITCH: i32 = 160;
/// A small option's button width. Two of them at a 160 pitch inside a 310 band.
pub const SMALL_BUTTON_WIDTH: i32 = 150;
/// `Button.DEFAULT_HEIGHT`.
pub const BUTTON_HEIGHT: i32 = 20;

/// Which options sub-screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionsPage {
    /// The root, whose entries are links to the others.
    Root,
    /// `SoundOptionsScreen` — carries `musicFrequency`.
    Sound,
    /// `AccessibilityOptionsScreen` — carries `hideLightningFlashes`.
    Accessibility,
}

/// One row of the list: one or two widgets.
#[derive(Clone, Debug, PartialEq)]
pub struct OptionRow {
    pub left: String,
    pub right: Option<String>,
}

/// Where a widget in the list lands.
///
/// `x = screen_width / 2 - 155 + column * 160`, `y` the row's content top.
pub fn widget_x(screen_width: i32, column: i32) -> i32 {
    screen_width / 2 - BAND_HALF + column * COLUMN_PITCH
}

/// The label a cycle button shows: `caption: value`.
///
/// `OptionInstance`'s `toString` is `(caption, value) -> value.caption()` for
/// `musicFrequency` (`Options.java:964`) and the generic
/// `genericValueLabel(caption, value)` for a boolean — which renders as
/// `caption: On`/`caption: Off`. Both are "the option's name, a colon, then the
/// value", which is what this builds.
pub fn cycle_label(caption: &str, value: &str) -> String {
    format!("{caption}: {value}")
}

/// `Options.BOOLEAN_VALUES` renders as On/Off.
pub fn bool_label(v: bool) -> &'static str {
    if v {
        "On"
    } else {
        "Off"
    }
}

/// The id of the widget for row `row`, column `col`.
///
/// Two per row, so `row * 2 + col` — a flat number because [`WidgetId`] is a
/// `u32` and the screen's own module owns what the numbers mean.
pub fn widget_id(row: usize, col: u32) -> WidgetId {
    row as u32 * 2 + col
}

/// Build one options page as a [`Screen`].
///
/// The rows are placed on `OptionsList`'s own geometry rather than through
/// [`crate::layout`] — see the module doc.
pub fn build(
    page: OptionsPage,
    rows: &[OptionRow],
    screen_w: i32,
    screen_h: i32,
    header_h: i32,
) -> Screen {
    let _ = page;
    let mut widgets = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let y = header_h + i as i32 * ROW_HEIGHT;
        widgets.push(Widget::button(
            widget_id(i, 0),
            widget_x(screen_w, 0),
            y,
            SMALL_BUTTON_WIDTH,
            BUTTON_HEIGHT,
            row.left.clone(),
        ));
        if let Some(r) = &row.right {
            widgets.push(Widget::button(
                widget_id(i, 1),
                widget_x(screen_w, 1),
                y,
                SMALL_BUTTON_WIDTH,
                BUTTON_HEIGHT,
                r.clone(),
            ));
        }
    }
    Screen::new(ScreenKind::Options, screen_w, screen_h).with_widgets(widgets)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The column pitch is 160 and the band is 310, and they do not agree.**
    ///
    /// `BAND_WIDTH / 2` is 155, so a second column placed at "half the band"
    /// would sit five pixels left of vanilla's. The two literals are a screen
    /// apart in the source and it is easy to derive one from the other.
    #[test]
    fn the_column_pitch_is_not_half_the_band() {
        assert_eq!(COLUMN_PITCH, 160);
        assert_eq!(BAND_WIDTH / 2, 155);
        assert_ne!(
            COLUMN_PITCH,
            BAND_WIDTH / 2,
            "deriving the pitch from the band puts every right-hand widget 5px out"
        );
    }

    /// The band is centred on the SCREEN, not on the list — so it moves with
    /// the window and both columns move together.
    #[test]
    fn the_band_is_centred_on_the_screen() {
        for w in [320, 640, 854, 1920] {
            assert_eq!(widget_x(w, 0), w / 2 - 155);
            assert_eq!(widget_x(w, 1), w / 2 - 155 + 160);
            // The left column's left edge and the right column's right edge
            // straddle the centre evenly enough that the row reads centred.
            let left = widget_x(w, 0);
            let right = widget_x(w, 1) + SMALL_BUTTON_WIDTH;
            assert_eq!(left + right, w, "the row is symmetric about the centre");
        }
    }

    /// A row with one option places one widget, not two with an empty label.
    #[test]
    fn an_odd_row_places_one_widget() {
        let rows = [OptionRow {
            left: "Music Frequency: Default".into(),
            right: None,
        }];
        let s = build(OptionsPage::Sound, &rows, 640, 480, 32);
        assert_eq!(s.widgets.len(), 1);
        assert_eq!(s.widgets[0].y, 32);
    }

    /// Rows advance on the 25-px pitch, which is `DEFAULT_ITEM_HEIGHT` and NOT
    /// the 20-px button height — so there are five pixels of gap per row.
    #[test]
    fn rows_advance_on_the_item_pitch_not_the_button_height() {
        let rows = vec![
            OptionRow {
                left: "a".into(),
                right: Some("b".into()),
            },
            OptionRow {
                left: "c".into(),
                right: None,
            },
        ];
        let s = build(OptionsPage::Sound, &rows, 640, 480, 0);
        assert_eq!(s.widgets.len(), 3);
        assert_eq!(s.widgets[0].y, 0);
        assert_eq!(s.widgets[1].y, 0, "both columns share a row");
        assert_eq!(s.widgets[2].y, ROW_HEIGHT);
        assert_ne!(
            ROW_HEIGHT, BUTTON_HEIGHT,
            "the pitch is the ITEM height; equating them removes the gap"
        );
    }

    /// The label is `caption: value`, which is what both of Rewo's options use.
    #[test]
    fn a_cycle_button_reads_caption_colon_value() {
        assert_eq!(cycle_label("Music Frequency", "Default"), "Music Frequency: Default");
        assert_eq!(bool_label(true), "On");
        assert_eq!(bool_label(false), "Off");
    }
}
