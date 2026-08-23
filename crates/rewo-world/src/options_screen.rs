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
//! The volume sliders arrived with M173: the Sound page is vanilla
//! `SoundOptionsScreen`'s `addBig(MASTER)` + five `addSmall` category pairs +
//! the music-frequency cycle button. `addBig` is one 310-wide widget spanning
//! both columns; `addSmall` pairs two 150-wide ones at the 160 pitch. The
//! rows vanilla has that Rewo does not model — the sound DEVICE (no device
//! enumeration), Closed Captions, Directional Audio and the music toast —
//! are absent rather than stubbed.

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

/// One widget of a row (M173).
#[derive(Clone, Debug, PartialEq)]
pub enum RowItem {
    /// A cycle/link button with this label.
    Button(String),
    /// An `AbstractSliderButton` with this label and value.
    Slider { label: String, value: f32 },
}

/// One row of the list: one or two widgets, or one BIG widget spanning the
/// whole 310 band (`OptionsList.addBig` — the master volume, the device).
#[derive(Clone, Debug, PartialEq)]
pub struct OptionRow {
    pub left: RowItem,
    pub right: Option<RowItem>,
    /// `addBig` — `left` spans [`BAND_WIDTH`]; `right` must be `None`.
    pub big: bool,
}

impl OptionRow {
    pub fn small(left: RowItem, right: Option<RowItem>) -> Self {
        Self { left, right, big: false }
    }
    pub fn big(item: RowItem) -> Self {
        Self { left: item, right: None, big: true }
    }
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
    let place = |widgets: &mut Vec<Widget>, item: &RowItem, id, x, y, w| match item {
        RowItem::Button(label) => {
            widgets.push(Widget::button(id, x, y, w, BUTTON_HEIGHT, label.clone()));
        }
        RowItem::Slider { label, value } => {
            widgets.push(Widget::slider(id, x, y, w, BUTTON_HEIGHT, label.clone(), *value));
        }
    };
    for (i, row) in rows.iter().enumerate() {
        let y = header_h + i as i32 * ROW_HEIGHT;
        let w0 = if row.big { BAND_WIDTH } else { SMALL_BUTTON_WIDTH };
        place(&mut widgets, &row.left, widget_id(i, 0), widget_x(screen_w, 0), y, w0);
        if let Some(r) = &row.right {
            debug_assert!(!row.big, "a big row spans the band; it has no right column");
            place(&mut widgets, r, widget_id(i, 1), widget_x(screen_w, 1), y, SMALL_BUTTON_WIDTH);
        }
    }
    // The footer Done — `OptionsSubScreen`'s `HeaderAndFooterLayout` puts a
    // 200-wide Done centred in the 33-px footer band.
    widgets.push(Widget::button(
        DONE,
        (screen_w - 200) / 2,
        screen_h - FOOTER_HEIGHT + (FOOTER_HEIGHT - BUTTON_HEIGHT) / 2,
        200,
        BUTTON_HEIGHT,
        "Done",
    ));
    Screen::new(ScreenKind::Options, screen_w, screen_h).with_widgets(widgets)
}

/// `HeaderAndFooterLayout`'s default header/footer height (`:11`).
pub const FOOTER_HEIGHT: i32 = 33;

/// The footer Done's widget id — clear of every `widget_id(row, col)` value.
pub const DONE: WidgetId = 10_000;

/// What a widget id on the SOUND page means (M173). The page's rows, in
/// vanilla's order: `addBig(MASTER)`, five `addSmall` category pairs in
/// `SoundSource.values()` order with MASTER filtered out (MUSIC+RECORDS,
/// WEATHER+BLOCKS, HOSTILE+NEUTRAL, PLAYERS+AMBIENT, VOICE+UI), then the
/// music-frequency cycle button alone in the left column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundSlot {
    /// A volume slider, by `SoundSource` ordinal (0 = MASTER).
    Volume(i32),
    /// The `musicFrequency` cycle button.
    MusicFrequency,
    /// The footer Done.
    Done,
}

/// Row layout of the sound page: id -> slot. `None` for an id the page does
/// not place.
pub fn sound_slot(id: WidgetId) -> Option<SoundSlot> {
    if id == DONE {
        return Some(SoundSlot::Done);
    }
    let (row, col) = (id / 2, id % 2);
    match row {
        0 if col == 0 => Some(SoundSlot::Volume(0)), // MASTER, addBig
        1..=5 => {
            // Pairs of the ten non-master sources, in enum order: the row's
            // left is ordinal `1 + (row-1)*2`, its right one past that.
            let ordinal = 1 + (row as i32 - 1) * 2 + col as i32;
            Some(SoundSlot::Volume(ordinal))
        }
        6 if col == 0 => Some(SoundSlot::MusicFrequency),
        _ => None,
    }
}

/// `Options.percentValueOrOffLabel` — `caption: NN%` with INT TRUNCATION
/// (`(int)(value * 100.0)`, so 0.699999 renders `69%`), and exactly 0.0 —
/// not 0.004 — renders `caption: OFF`. The caller passes the translated
/// caption and the translated OFF; the `%s: %s%%` / `%s: %s` templates
/// reduce to this shape for every locale that keeps the order, which is the
/// same approximation the cycle label makes.
pub fn percent_label(caption: &str, value: f32, off: &str) -> String {
    if value == 0.0 {
        format!("{caption}: {off}")
    } else {
        format!("{caption}: {}%", (value * 100.0) as i32)
    }
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
        let rows = [OptionRow::small(
            RowItem::Button("Music Frequency: Default".into()),
            None,
        )];
        let s = build(OptionsPage::Sound, &rows, 640, 480, 32);
        assert_eq!(s.widgets.len(), 2, "the row plus the footer Done");
        assert_eq!(s.widgets[0].y, 32);
        assert_eq!(s.widgets[1].id, DONE);
    }

    /// Rows advance on the 25-px pitch, which is `DEFAULT_ITEM_HEIGHT` and NOT
    /// the 20-px button height — so there are five pixels of gap per row.
    #[test]
    fn rows_advance_on_the_item_pitch_not_the_button_height() {
        let rows = vec![
            OptionRow::small(RowItem::Button("a".into()), Some(RowItem::Button("b".into()))),
            OptionRow::small(RowItem::Button("c".into()), None),
        ];
        let s = build(OptionsPage::Sound, &rows, 640, 480, 0);
        assert_eq!(s.widgets.len(), 4, "three row widgets plus the footer Done");
        assert_eq!(s.widgets[0].y, 0);
        assert_eq!(s.widgets[1].y, 0, "both columns share a row");
        assert_eq!(s.widgets[2].y, ROW_HEIGHT);
        assert_ne!(
            ROW_HEIGHT, BUTTON_HEIGHT,
            "the pitch is the ITEM height; equating them removes the gap"
        );
    }

    /// `addBig` spans the whole 310 band; the sound page's master slider is
    /// one, and the row's widget carries the slider VALUE.
    #[test]
    fn a_big_slider_row_spans_the_band() {
        let rows = [OptionRow::big(RowItem::Slider {
            label: "Master Volume: 50%".into(),
            value: 0.5,
        })];
        let s = build(OptionsPage::Sound, &rows, 640, 480, 32);
        let w = &s.widgets[0];
        assert_eq!(w.width, BAND_WIDTH);
        assert_eq!(w.x, 640 / 2 - BAND_HALF);
        assert!(matches!(w.kind, crate::screen::WidgetKind::Slider { value } if value == 0.5));
    }

    /// The sound page's id -> slot map, in vanilla's row order: `addBig`
    /// MASTER, five pairs in `SoundSource.values()` order with MASTER
    /// filtered out, then musicFrequency alone in the left column.
    #[test]
    fn the_sound_page_slots_follow_vanillas_row_order() {
        assert_eq!(sound_slot(widget_id(0, 0)), Some(SoundSlot::Volume(0)), "MASTER");
        assert_eq!(sound_slot(widget_id(1, 0)), Some(SoundSlot::Volume(1)), "MUSIC");
        assert_eq!(sound_slot(widget_id(1, 1)), Some(SoundSlot::Volume(2)), "RECORDS");
        assert_eq!(sound_slot(widget_id(3, 1)), Some(SoundSlot::Volume(6)), "NEUTRAL");
        assert_eq!(sound_slot(widget_id(5, 1)), Some(SoundSlot::Volume(10)), "UI");
        assert_eq!(sound_slot(widget_id(6, 0)), Some(SoundSlot::MusicFrequency));
        assert_eq!(sound_slot(widget_id(6, 1)), None, "no music toast in Rewo");
        assert_eq!(sound_slot(DONE), Some(SoundSlot::Done));
    }

    /// `percentValueOrOffLabel`: INT TRUNCATION (`0.699999` renders 69%),
    /// and only EXACTLY 0.0 is OFF — 0.004 is `0%`.
    #[test]
    fn the_percent_label_truncates_and_only_exact_zero_is_off() {
        assert_eq!(percent_label("Music", 0.699999, "OFF"), "Music: 69%");
        assert_eq!(percent_label("Music", 1.0, "OFF"), "Music: 100%");
        assert_eq!(percent_label("Music", 0.0, "OFF"), "Music: OFF");
        assert_eq!(percent_label("Music", 0.004, "OFF"), "Music: 0%");
    }

    /// The slider mouse math: `(mx - (x + 4)) / (width - 8)` clamped — the
    /// +4 is the handle HALF width; and the arrow step is one handle-pixel,
    /// `1/(width-8)`, which differs between the 310- and 150-wide sliders by
    /// design.
    #[test]
    fn the_slider_math_matches_abstract_slider_button() {
        use crate::screen::{slider_handle_x, slider_value_from_mouse};
        // Click dead centre of a 310-wide slider at x=165: mx = 165+4+151.
        assert!((slider_value_from_mouse(165, 310, 320.0) - 0.5).abs() < 0.01);
        assert_eq!(slider_value_from_mouse(165, 310, 0.0), 0.0, "clamped left");
        assert_eq!(slider_value_from_mouse(165, 310, 9999.0), 1.0, "clamped right");
        // The handle: x + (int)(value * (width - 8)).
        assert_eq!(slider_handle_x(165, 310, 0.0), 165);
        assert_eq!(slider_handle_x(165, 310, 1.0), 165 + 302);
        assert_eq!(slider_handle_x(165, 310, 0.5), 165 + 151);
    }

    /// The label is `caption: value`, which is what both of Rewo's options use.
    #[test]
    fn a_cycle_button_reads_caption_colon_value() {
        assert_eq!(cycle_label("Music Frequency", "Default"), "Music Frequency: Default");
        assert_eq!(bool_label(true), "On");
        assert_eq!(bool_label(false), "Off");
    }
}
