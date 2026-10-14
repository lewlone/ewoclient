//! Delete-instance confirmation — a small modal that names the instance and
//! says what happens to its folder before anything is touched: it leaves the
//! list and the folder (worlds, screenshots, mods, settings) moves to the
//! Recycle Bin, where it can be restored. Never a permanent delete.
//!
//! Same chrome as the About modal (it reuses its shroud and card chrome):
//! the chrome scales in, the text never does (non-negotiable #2), the
//! entrance runs on the silk curve and through `motion::dur`, so reduced
//! motion collapses it. Dismisses on Esc / outside-click / Cancel; while the
//! move is running (`busy`) it can't be dismissed.

use crate::tokens;
use ewo_core::motion;
use skia_safe::{canvas::SaveLayerRec, Canvas, ClipOp, Contains, Font, Paint, Point, RRect, Rect};

use crate::screens::about_modal::{draw_card_chrome, draw_shroud};
use crate::text::{self, FontStore};
use crate::widgets::{draw_vghost_btn, GhostKind, VghostBtnState};

const CARD_RADIUS: f32 = 22.0;
const CARD_W: f32 = 480.0;
const CARD_H: f32 = 360.0;
const PAD_X: f32 = 40.0;
const PAD_TOP: f32 = 34.0;
const PAD_BOTTOM: f32 = 28.0;
const ANIM_DURATION: f32 = 0.24;
const BTN_H: f32 = 38.0;
const CANCEL_W: f32 = 110.0;
const CONFIRM_W: f32 = 200.0;
/// Most lines the explanation and the error may take each.
const MAX_BODY_LINES: usize = 5;
const MAX_ERROR_LINES: usize = 3;

/// Where deleted instance folders go, in the user's words.
pub const BIN_NAME: &str = if cfg!(windows) {
    "Recycle Bin"
} else {
    "Trash"
};

#[derive(Debug, Clone, Default)]
pub struct DeleteInstanceModalState {
    pub open: bool,
    /// 0..1, drives the entrance animation.
    pub anim: f32,
    /// The instance's stable id (opaque to the renderer; the launcher
    /// matches the confirmed delete against it).
    pub instance_id: String,
    /// Display name, shown as the title.
    pub instance_name: String,
    /// The folder that will move, as shown to the user.
    pub folder_label: String,
    /// The move is running on a worker: buttons are inert.
    pub busy: bool,
    /// Why the last attempt failed. The instance and its folder are kept.
    pub error: Option<String>,
    pub cancel_btn: VghostBtnState,
    pub confirm_btn: VghostBtnState,
}

impl DeleteInstanceModalState {
    pub fn open(&mut self, instance_id: &str, instance_name: &str, folder_label: &str) {
        *self = Self {
            open: true,
            instance_id: instance_id.to_string(),
            instance_name: instance_name.to_string(),
            folder_label: folder_label.to_string(),
            ..Self::default()
        };
    }

    pub fn close(&mut self) {
        self.open = false;
        self.anim = 0.0;
        self.busy = false;
        self.error = None;
    }

    pub fn tick(&mut self, dt: f32) {
        if self.open && self.anim < 1.0 {
            self.anim = (self.anim + dt / motion::dur(ANIM_DURATION)).min(1.0);
        }
        self.cancel_btn.tick(dt);
        self.confirm_btn.tick(dt);
    }
}

pub fn card_rect(card_w: f32, card_h: f32) -> Rect {
    let w = CARD_W.min(card_w - 80.0);
    let h = CARD_H.min(card_h - 80.0);
    let cx = card_w * 0.5;
    let cy = card_h * 0.5;
    Rect::from_xywh(cx - w * 0.5, cy - h * 0.5, w, h)
}

pub fn cancel_button_bounds(card_w: f32, card_h: f32) -> Rect {
    let card = card_rect(card_w, card_h);
    Rect::from_xywh(
        card.left + PAD_X,
        card.bottom - PAD_BOTTOM - BTN_H,
        CANCEL_W,
        BTN_H,
    )
}

pub fn confirm_button_bounds(card_w: f32, card_h: f32) -> Rect {
    let card = card_rect(card_w, card_h);
    Rect::from_xywh(
        card.right - PAD_X - CONFIRM_W,
        card.bottom - PAD_BOTTOM - BTN_H,
        CONFIRM_W,
        BTN_H,
    )
}

/// True if a press at `mouse` lands on the shroud (outside the card).
pub fn shroud_consumes(mouse: (f32, f32), card_w: f32, card_h: f32) -> bool {
    !card_rect(card_w, card_h).contains(Point::new(mouse.0, mouse.1))
}

/// `text` shortened with an ellipsis until `measure` says it fits `max_w`.
pub fn fit_with_ellipsis(measure: &dyn Fn(&str) -> f32, text: &str, max_w: f32) -> String {
    if measure(text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    for keep in (0..chars.len()).rev() {
        let candidate: String = chars[..keep]
            .iter()
            .collect::<String>()
            .trim_end()
            .to_string()
            + "\u{2026}";
        if measure(&candidate) <= max_w {
            return candidate;
        }
    }
    "\u{2026}".to_string()
}

/// Greedy word wrap of `text` to `max_w`, at most `max_lines` lines (the last
/// one ellipsized when the text runs on). A single word wider than the line
/// is cut by [`fit_with_ellipsis`].
pub fn wrap_lines(
    measure: &dyn Fn(&str) -> f32,
    text: &str,
    max_w: f32,
    max_lines: usize,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut overflow = false;
    for word in text.split_whitespace() {
        let trial = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if cur.is_empty() || measure(&trial) <= max_w {
            cur = trial;
            continue;
        }
        lines.push(std::mem::take(&mut cur));
        if lines.len() == max_lines {
            overflow = true;
            break;
        }
        cur = word.to_string();
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if overflow {
        if let Some(last) = lines.last_mut() {
            let with_mark = format!("{last}\u{2026}");
            *last = if measure(&with_mark) <= max_w {
                with_mark
            } else {
                fit_with_ellipsis(measure, last, max_w)
            };
        }
    }
    for l in &mut lines {
        if measure(l) > max_w {
            *l = fit_with_ellipsis(measure, l, max_w);
        }
    }
    lines
}

/// The explanation the modal shows under the instance's name.
pub fn explanation() -> String {
    format!(
        "It leaves your list, and its folder (worlds, screenshots, mods and settings) \
         is moved to the {BIN_NAME}. You can restore it from there. \
         Nothing is deleted permanently."
    )
}

pub fn draw_modal(
    canvas: &Canvas,
    fonts: &FontStore,
    card_w: f32,
    card_h: f32,
    state: &DeleteInstanceModalState,
) {
    if !state.open && state.anim <= 0.001 {
        return;
    }
    let anim = tokens::silk().eval(state.anim.clamp(0.0, 1.0));

    draw_shroud(canvas, card_w, card_h, (state.anim / 0.67).clamp(0.0, 1.0));

    // Chrome scales about the card centre; the text only fades and moves by
    // whole pixels (non-negotiable #2). At rest both layers coincide.
    let card = card_rect(card_w, card_h);
    let ty = (10.0 * (1.0 - anim)).round();
    let scale = 0.97 + 0.03 * anim;

    let saved = canvas.save();
    canvas.translate((0.0, ty));
    let mut layer_paint = Paint::default();
    layer_paint.set_alpha_f(anim);
    canvas.save_layer(&SaveLayerRec::default().bounds(&card).paint(&layer_paint));

    let cx = (card.left + card.right) * 0.5;
    let cy = (card.top + card.bottom) * 0.5;
    let chrome_saved = canvas.save();
    canvas.translate((cx, cy));
    canvas.scale((scale, scale));
    canvas.translate((-cx, -cy));
    draw_card_chrome(canvas, &card);
    canvas.restore_to_count(chrome_saved);

    let clip_saved = canvas.save();
    canvas.clip_rrect(
        RRect::new_rect_xy(card, CARD_RADIUS, CARD_RADIUS),
        Some(ClipOp::Intersect),
        Some(true),
    );
    draw_text(canvas, fonts, &card, state);
    canvas.restore_to_count(clip_saved);

    draw_buttons(canvas, fonts, card_w, card_h, state);

    canvas.restore();
    canvas.restore_to_count(saved);
}

fn draw_text(canvas: &Canvas, fonts: &FontStore, card: &Rect, state: &DeleteInstanceModalState) {
    let left = card.left + PAD_X;
    let max_w = card.width() - 2.0 * PAD_X;

    // Eyebrow.
    let eyebrow = fonts.jetbrains_mono(10.0);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(tokens::accent_berry());
    let (_, em) = eyebrow.metrics();
    let eb_base = card.top + PAD_TOP + (-em.ascent);
    text::draw_tracked_em(
        canvas,
        "DELETE INSTANCE",
        (left, eb_base),
        &eyebrow,
        &paint,
        0.18,
    );

    // Title: the instance's name, never wider than the card.
    let title_font = fonts.fraunces_axes(28.0, 50.0, 0.0, 600.0, None);
    let measure_title = |s: &str| title_font.measure_str(s, None).0;
    let title = fit_with_ellipsis(&measure_title, &state.instance_name, max_w);
    paint.set_color(tokens::text_pearl_hot());
    let (_, tm) = title_font.metrics();
    let title_base = eb_base + em.descent + 14.0 + (-tm.ascent);
    canvas.draw_str(&title, (left, title_base), &title_font, &paint);

    // Explanation.
    let body = Font::new(fonts.newsreader_italic_typeface().clone(), 15.0);
    let measure_body = |s: &str| body.measure_str(s, None).0;
    let (_, bm) = body.metrics();
    let line_h = bm.descent - bm.ascent + 4.0;
    paint.set_color(tokens::text_pearl());
    let mut y = title_base + tm.descent + 16.0 + (-bm.ascent);
    for line in wrap_lines(&measure_body, &explanation(), max_w, MAX_BODY_LINES) {
        canvas.draw_str(&line, (left, y), &body, &paint);
        y += line_h;
    }

    // The folder that moves.
    let mono = fonts.jetbrains_mono(10.0);
    let (_, mm) = mono.metrics();
    paint.set_color(tokens::text_mauve());
    let measure_mono = |s: &str| text::measure_tracked_em(&mono, s, 0.06);
    let folder = fit_with_ellipsis(&measure_mono, &state.folder_label, max_w);
    y += 6.0 + (-mm.ascent);
    text::draw_tracked_em(canvas, &folder, (left, y), &mono, &paint, 0.06);

    // Status: progress while it runs, the reason when it failed.
    let status = fonts.newsreader(13.0);
    let (_, sm) = status.metrics();
    let measure_status = |s: &str| status.measure_str(s, None).0;
    y += mm.descent + 14.0 + (-sm.ascent);
    if state.busy {
        paint.set_color(tokens::text_mauve());
        let msg = format!("moving it to the {BIN_NAME}\u{2026}");
        canvas.draw_str(&msg, (left, y), &status, &paint);
    } else if let Some(err) = &state.error {
        paint.set_color(tokens::accent_ember_text());
        let msg = format!("Could not move it: {err}. The folder was kept.");
        for line in wrap_lines(&measure_status, &msg, max_w, MAX_ERROR_LINES) {
            canvas.draw_str(&line, (left, y), &status, &paint);
            y += sm.descent - sm.ascent + 2.0;
        }
    }
}

fn draw_buttons(
    canvas: &Canvas,
    fonts: &FontStore,
    card_w: f32,
    card_h: f32,
    state: &DeleteInstanceModalState,
) {
    draw_vghost_btn(
        canvas,
        cancel_button_bounds(card_w, card_h),
        "Cancel",
        &state.cancel_btn,
        GhostKind::Pearl,
        fonts,
    );
    let label = if state.busy {
        "Moving\u{2026}".to_string()
    } else {
        format!("Move to {BIN_NAME}")
    };
    draw_vghost_btn(
        canvas,
        confirm_button_bounds(card_w, card_h),
        &label,
        &state.confirm_btn,
        GhostKind::Danger,
        fonts,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 units per character.
    fn m(s: &str) -> f32 {
        s.chars().count() as f32 * 10.0
    }

    #[test]
    fn buttons_sit_inside_the_card_on_either_side_and_do_not_overlap() {
        let (w, h) = (1100.0, 700.0);
        let card = card_rect(w, h);
        let (cancel, confirm) = (cancel_button_bounds(w, h), confirm_button_bounds(w, h));
        assert!(card.contains(cancel) && card.contains(confirm));
        assert!(cancel.right < confirm.left, "no overlap");
        assert!(cancel.left < confirm.left);
    }

    #[test]
    fn the_shroud_is_everything_outside_the_card() {
        let (w, h) = (1100.0, 700.0);
        let card = card_rect(w, h);
        assert!(shroud_consumes((2.0, 2.0), w, h));
        assert!(shroud_consumes((card.left - 1.0, card.top + 5.0), w, h));
        assert!(!shroud_consumes((card.left + 5.0, card.top + 5.0), w, h));
        assert!(!shroud_consumes(
            ((card.left + card.right) / 2.0, card.bottom - 1.0),
            w,
            h
        ));
    }

    #[test]
    fn a_small_window_keeps_the_card_and_buttons_on_screen() {
        let (w, h) = (520.0, 420.0);
        let card = card_rect(w, h);
        assert!(card.left >= 0.0 && card.right <= w && card.top >= 0.0 && card.bottom <= h);
        assert!(card.contains(confirm_button_bounds(w, h)));
    }

    #[test]
    fn long_names_are_ellipsized_to_fit() {
        assert_eq!(fit_with_ellipsis(&m, "short", 100.0), "short");
        let fitted = fit_with_ellipsis(&m, "an extremely long instance name", 100.0);
        assert!(m(&fitted) <= 100.0, "{fitted}");
        assert!(fitted.ends_with('\u{2026}'));
        assert_eq!(fit_with_ellipsis(&m, "abc", 5.0), "\u{2026}");
    }

    #[test]
    fn wrapping_respects_the_width_and_the_line_cap() {
        let lines = wrap_lines(&m, "aaa bbb ccc ddd eee", 80.0, 5);
        assert_eq!(lines, ["aaa bbb", "ccc ddd", "eee"]);
        assert!(lines.iter().all(|l| m(l) <= 80.0));
        let capped = wrap_lines(&m, "aaa bbb ccc ddd eee fff", 40.0, 2);
        assert_eq!(capped.len(), 2);
        assert!(capped[1].ends_with('\u{2026}'), "{capped:?}");
        assert!(capped.iter().all(|l| m(l) <= 40.0));
        assert!(wrap_lines(&m, "", 80.0, 3).is_empty());
        // One word wider than the line is cut, not overflowed.
        let wide = wrap_lines(&m, "abcdefghijklmnop", 50.0, 3);
        assert!(wide.iter().all(|l| m(l) <= 50.0), "{wide:?}");
    }

    #[test]
    fn the_explanation_names_the_bin_and_rules_out_a_permanent_delete() {
        let e = explanation();
        assert!(e.contains(BIN_NAME));
        for what in ["worlds", "screenshots", "mods"] {
            assert!(e.contains(what), "{what}");
        }
        assert!(e.contains("permanently"));
    }

    #[test]
    fn opening_resets_stale_state_and_closing_clears_busy_and_error() {
        let mut s = DeleteInstanceModalState {
            busy: true,
            error: Some("old".into()),
            anim: 1.0,
            ..Default::default()
        };
        s.open("id-1", "Velvet Hours", "instances/id-1");
        assert!(s.open && !s.busy && s.error.is_none() && s.anim == 0.0);
        assert_eq!(
            (s.instance_id.as_str(), s.instance_name.as_str()),
            ("id-1", "Velvet Hours")
        );
        s.busy = true;
        s.error = Some("x".into());
        s.close();
        assert!(!s.open && !s.busy && s.error.is_none());
    }
}
