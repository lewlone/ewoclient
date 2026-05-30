//! Phase H5 — Friends screen. Lists the active account's chickenedin
//! friends + pending requests, lets the user add/accept/decline/remove.
//!
//! State + HTTP live launcher-side in `social/`; this module is a
//! decoupled view that takes already-fetched data + a few interaction
//! states (the add-friend input, button hovers) as render input.
//!
//! Sections:
//!   - Heading + "+ Add a friend" affordance with inline text input
//!   - INCOMING requests list (with Accept / Decline buttons)
//!   - FRIENDS list (with status pill + Remove button)
//!   - OUTGOING requests list (informational)
//!
//! Empty / not-linked / loading variants render a single centered
//! message instead of the three-section layout.

use ewo_core::CubicBezier;
use skia_safe::{Canvas, Color, Color4f, Font, Paint, PaintStyle, Point, RRect, Rect};

use crate::text::{self, FontStore};
use crate::widgets::{
    draw_vbtn, draw_vghost_btn, GhostKind, VbtnState, VghostBtnState,
};

const TEXT_PEARL_HOT: Color = Color::from_argb(0xFF, 0xFF, 0xF6, 0xF0);
const TEXT_PEARL: Color = Color::from_argb(0xFF, 0xF4, 0xE8, 0xEA);
const TEXT_MID_PEARL: Color = Color::from_argb(0xFF, 0xC4, 0xAF, 0xB5);
const TEXT_MAUVE: Color = Color::from_argb(0xFF, 0x9A, 0x80, 0x87);
const TEXT_MAUVE_DEEP: Color = Color::from_argb(0xFF, 0x6B, 0x55, 0x5C);
const ACCENT_ROSE: Color = Color::from_argb(0xFF, 0xE5, 0xB8, 0xC5);
const ACCENT_BERRY: Color = Color::from_argb(0xFF, 0xB4, 0x74, 0x91);
const ACCENT_LAV: Color = Color::from_argb(0xFF, 0xC9, 0xA5, 0xD4);
const ACCENT_CHAMP: Color = Color::from_argb(0xFF, 0xE8, 0xD4, 0xA8);
const ACCENT_EMBER: Color = Color::from_argb(0xFF, 0xC9, 0x6A, 0x7A);

const HEADER_TOP: f32 = 88.0;
const BODY_PAD_X: f32 = 60.0;

/// Top-level state of the Friends screen as the renderer sees it.
/// The slice-bearing variant borrows owned `FriendRowView` Vecs built
/// by `main.rs` once per frame — `FriendRowView` itself is owned to
/// keep the renderer decoupled from `social::FriendsListState`'s
/// lifetime.
#[derive(Copy, Clone, Debug)]
pub enum FriendsViewState<'a> {
    /// Active account has no launcher social_token. Show a "link your
    /// launcher" affordance + a button that opens the launcher-link
    /// modal (handled by main.rs in handle_friends_press).
    NotLinked,
    /// First poll hasn't returned yet. Show a centered "loading…".
    Loading,
    /// Friends list fetched. Sections passed individually.
    Loaded {
        friends: &'a [FriendRowView],
        incoming: &'a [FriendRowView],
        outgoing: &'a [FriendRowView],
    },
    /// Most recent fetch errored. Show a short message.
    Failed(&'a str),
}

#[derive(Clone, Debug)]
pub struct FriendRowView {
    /// The other party's display name (MC name when known, fallback
    /// "Player <last-8-of-uuid>" otherwise).
    pub display_name: String,
    /// Short presence text — "in launcher · main_menu", "in-game ·
    /// play.chickenedin.com", or "offline".
    pub presence: String,
    /// Whether `presence` is "online" enough to render the lit dot.
    pub online: bool,
    /// Stable id for matching click events back to a `discord_id`.
    pub discord_id: String,
}

/// Per-button hover/press state for the screen. Held by `App`.
#[derive(Debug, Default)]
pub struct FriendsPrefs {
    /// Text the user has typed into the "add friend" input.
    pub add_buffer: String,
    /// True while the input is the focus target — driven by clicking
    /// the input box or `+ Add` button.
    pub add_focused: bool,
    /// Caret animation phase since last edit (drives the blink).
    pub add_focus_time: f32,
    /// "+ Add friend" submit button.
    pub add_submit_btn: VbtnState,
    /// Affordance shown in the NotLinked variant — opens the
    /// launcher-link modal.
    pub link_launcher_btn: VbtnState,
}

impl FriendsPrefs {
    pub fn tick(&mut self, dt: f32) {
        self.add_focus_time += dt;
        self.add_submit_btn.tick(dt);
        self.link_launcher_btn.tick(dt);
    }
}

/// Bounds (card-local) of the major hit targets on the Friends screen.
/// Built by both the renderer and `main.rs` so visuals + input agree.
pub struct FriendsLayout {
    pub content_left: f32,
    pub content_right: f32,
    pub header_top: f32,
    pub add_input: Rect,
    pub add_submit: Rect,
    pub link_launcher_btn: Rect,
    /// One per visible friend / request row, in display order. The
    /// kind tells main.rs which mutation to dispatch on click.
    pub row_buttons: Vec<RowButton>,
}

#[derive(Copy, Clone, Debug)]
pub struct RowButton {
    pub kind: RowButtonKind,
    /// Index into the corresponding section's slice.
    pub index: usize,
    pub rect: Rect,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RowButtonKind {
    /// Accept an incoming request.
    Accept,
    /// Decline an incoming request.
    Decline,
    /// Remove a friend.
    Remove,
}

/// Per-section row counts — what `friends_layout` needs to size up.
/// Both `draw_friends` and `main.rs::handle_friends_press` derive this
/// from the same `FriendsViewState`/social state, so the layout they
/// build agrees.
#[derive(Copy, Clone, Debug, Default)]
pub struct FriendsCounts {
    pub friends: usize,
    pub incoming: usize,
    pub outgoing: usize,
}

impl FriendsCounts {
    pub fn from_view(view: FriendsViewState<'_>) -> Self {
        if let FriendsViewState::Loaded { friends, incoming, outgoing } = view {
            FriendsCounts {
                friends: friends.len(),
                incoming: incoming.len(),
                outgoing: outgoing.len(),
            }
        } else {
            Self::default()
        }
    }
}

/// Layout function — used by `draw_friends` and the input handler.
/// Walks the same row geometry as the renderer so `row_buttons` is the
/// canonical hit-rect source for incoming-accept / incoming-decline /
/// friend-remove clicks.
pub fn friends_layout(
    card_w: f32,
    _fonts: &FontStore,
    counts: FriendsCounts,
) -> FriendsLayout {
    let content_left = BODY_PAD_X;
    let content_right = card_w - BODY_PAD_X;
    let header_top = HEADER_TOP;
    let add_input_top = header_top + 56.0;
    let add_input = Rect::from_xywh(
        content_left,
        add_input_top,
        content_right - content_left - 12.0 - 110.0,
        38.0,
    );
    let add_submit = Rect::from_xywh(
        add_input.right + 12.0,
        add_input_top,
        110.0,
        38.0,
    );
    let link_btn_w = 220.0;
    let link_btn_h = 40.0;
    let link_launcher_btn = Rect::from_xywh(
        (card_w - link_btn_w) * 0.5,
        header_top + 120.0,
        link_btn_w,
        link_btn_h,
    );

    // Walk rows in the same order as the renderer to collect button rects.
    let mut row_buttons: Vec<RowButton> = Vec::new();
    if counts.friends + counts.incoming + counts.outgoing > 0 {
        let mut y = add_input.bottom + 36.0;
        let row_h: f32 = 54.0;
        let btn_h: f32 = 28.0;
        let btn_w: f32 = 80.0;
        let section_head_to_first_row: f32 = 24.0;
        let row_gap: f32 = 8.0;
        let section_trailer: f32 = 24.0;

        // Incoming — Accept + Decline buttons on each row.
        if counts.incoming > 0 {
            y += section_head_to_first_row;
            for i in 0..counts.incoming {
                let btn_y = y + row_h * 0.5 - btn_h * 0.5;
                let accept = Rect::from_xywh(
                    content_right - btn_w - 8.0, btn_y, btn_w, btn_h);
                let decline = Rect::from_xywh(
                    accept.left - btn_w - 8.0, btn_y, btn_w, btn_h);
                row_buttons.push(RowButton {
                    kind: RowButtonKind::Accept, index: i, rect: accept,
                });
                row_buttons.push(RowButton {
                    kind: RowButtonKind::Decline, index: i, rect: decline,
                });
                y += row_h + row_gap;
            }
            y += section_trailer;
        }
        // Friends — Remove on each row.
        if counts.friends > 0 {
            y += section_head_to_first_row;
            for i in 0..counts.friends {
                let btn_y = y + row_h * 0.5 - btn_h * 0.5;
                let remove = Rect::from_xywh(
                    content_right - btn_w - 8.0, btn_y, btn_w, btn_h);
                row_buttons.push(RowButton {
                    kind: RowButtonKind::Remove, index: i, rect: remove,
                });
                y += row_h + row_gap;
            }
            y += section_trailer;
        }
        // Outgoing rows have a "pending" tag, no buttons.
        let _ = y;
    }

    FriendsLayout {
        content_left,
        content_right,
        header_top,
        add_input,
        add_submit,
        link_launcher_btn,
        row_buttons,
    }
}

/// Top-level draw entry. Always renders the heading + add bar (when
/// applicable); the central content varies by `view`.
pub fn draw_friends(
    canvas: &Canvas,
    fonts: &FontStore,
    card_w: f32,
    card_h: f32,
    _time: f32,
    prefs: &FriendsPrefs,
    view: FriendsViewState<'_>,
) -> FriendsLayout {
    let layout = friends_layout(card_w, fonts, FriendsCounts::from_view(view));

    // Heading
    let head_font = fonts.fraunces_axes(46.0, 50.0, 0.0, 600.0, None);
    let mut head_paint = Paint::default();
    head_paint.set_anti_alias(true);
    head_paint.set_color(TEXT_PEARL_HOT);
    let (_, hm) = head_font.metrics();
    canvas.draw_str(
        "Friends",
        (layout.content_left, layout.header_top + (-hm.ascent)),
        &head_font,
        &head_paint,
    );

    // Subtitle (italic)
    let sub_font = Font::new(fonts.newsreader_italic_typeface().clone(), 15.0);
    let mut sub_paint = Paint::default();
    sub_paint.set_anti_alias(true);
    sub_paint.set_color(TEXT_MAUVE);
    let (_, sm) = sub_font.metrics();
    let sub_baseline = layout.header_top + hm.descent + 8.0 + (-sm.ascent);
    canvas.draw_str(
        match view {
            FriendsViewState::NotLinked => "link the launcher to chickenedin to see who's online.",
            FriendsViewState::Loading => "fetching your friend list…",
            FriendsViewState::Failed(_) => "couldn't reach the bot.",
            FriendsViewState::Loaded { .. } => "who's around right now.",
        },
        (layout.content_left, sub_baseline),
        &sub_font,
        &sub_paint,
    );

    match view {
        FriendsViewState::NotLinked => {
            draw_not_linked(canvas, fonts, &layout, prefs);
        }
        FriendsViewState::Loading => {
            draw_centered_message(canvas, fonts, card_w, card_h, "loading…", TEXT_MAUVE);
        }
        FriendsViewState::Failed(msg) => {
            draw_centered_message(canvas, fonts, card_w, card_h, msg, ACCENT_EMBER);
        }
        FriendsViewState::Loaded { friends, incoming, outgoing } => {
            draw_add_bar(canvas, fonts, &layout, prefs);
            let mut y = layout.add_input.bottom + 36.0;
            if !incoming.is_empty() {
                y = draw_section(
                    canvas, fonts, &layout, y, "INCOMING REQUESTS",
                    incoming, RowSection::Incoming,
                );
            }
            if !friends.is_empty() {
                y = draw_section(
                    canvas, fonts, &layout, y, "FRIENDS",
                    friends, RowSection::Friends,
                );
            }
            if !outgoing.is_empty() {
                let _ = draw_section(
                    canvas, fonts, &layout, y, "OUTGOING REQUESTS",
                    outgoing, RowSection::Outgoing,
                );
            }
            if friends.is_empty() && incoming.is_empty() && outgoing.is_empty() {
                draw_centered_message(
                    canvas, fonts, card_w, card_h,
                    "no friends yet — add one above.",
                    TEXT_MAUVE,
                );
            }
        }
    }

    layout
}

#[derive(Copy, Clone)]
enum RowSection {
    Friends,
    Incoming,
    Outgoing,
}

fn draw_not_linked(canvas: &Canvas, fonts: &FontStore, layout: &FriendsLayout, prefs: &FriendsPrefs) {
    let msg_font = fonts.newsreader(15.0);
    let mut msg_paint = Paint::default();
    msg_paint.set_anti_alias(true);
    msg_paint.set_color(TEXT_MID_PEARL);
    let (_, mm) = msg_font.metrics();
    let msg = "your launcher needs to be linked before social features unlock.";
    let advance = msg_font.measure_str(msg, Some(&msg_paint)).0;
    let cx = (layout.content_left + layout.content_right) * 0.5;
    canvas.draw_str(
        msg,
        (cx - advance * 0.5, layout.header_top + 96.0 + (-mm.ascent)),
        &msg_font,
        &msg_paint,
    );
    // The button is rendered by the caller (we expose its bounds in
    // `layout.link_launcher_btn`); draw it here using the standard vbtn.
    draw_vbtn_label_only(canvas, layout.link_launcher_btn, "Link launcher", &prefs.link_launcher_btn, fonts);
}

fn draw_add_bar(canvas: &Canvas, fonts: &FontStore, layout: &FriendsLayout, prefs: &FriendsPrefs) {
    // Input chrome — rounded rect, faint rose tint, brighter border on
    // focus (matches the path-field aesthetic).
    let rrect = RRect::new_rect_xy(layout.add_input, 8.0, 8.0);
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(Color4f::new(0.90, 0.72, 0.77, 0.04), None);
    canvas.draw_rrect(rrect, &bg);
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    let border_alpha = if prefs.add_focused { 0.55 } else { 0.20 };
    border.set_color4f(Color4f::new(0.90, 0.72, 0.77, border_alpha), None);
    canvas.draw_rrect(rrect, &border);

    let label_font = fonts.newsreader(14.0);
    let mut label_paint = Paint::default();
    label_paint.set_anti_alias(true);
    let (_, lm) = label_font.metrics();
    let baseline = (layout.add_input.top + layout.add_input.bottom) * 0.5
        + (-lm.ascent - lm.descent) * 0.5;

    if prefs.add_buffer.is_empty() && !prefs.add_focused {
        label_paint.set_color(TEXT_MAUVE_DEEP);
        canvas.draw_str(
            "type a Minecraft name…",
            (layout.add_input.left + 14.0, baseline),
            &label_font,
            &label_paint,
        );
    } else {
        label_paint.set_color(TEXT_PEARL);
        canvas.draw_str(
            &prefs.add_buffer,
            (layout.add_input.left + 14.0, baseline),
            &label_font,
            &label_paint,
        );
        if prefs.add_focused {
            // Caret — blink every 1.06s
            let blink = (prefs.add_focus_time * 1.9).sin() > 0.0;
            if blink {
                let typed_w = label_font
                    .measure_str(&prefs.add_buffer, Some(&label_paint))
                    .0;
                let mut caret = Paint::default();
                caret.set_anti_alias(true);
                caret.set_color(ACCENT_ROSE);
                caret.set_stroke_width(1.0);
                caret.set_style(PaintStyle::Stroke);
                let x = layout.add_input.left + 14.0 + typed_w + 1.0;
                canvas.draw_line(
                    (x, layout.add_input.top + 8.0),
                    (x, layout.add_input.bottom - 8.0),
                    &caret,
                );
            }
        }
    }

    draw_vbtn_label_only(
        canvas,
        layout.add_submit,
        if prefs.add_buffer.is_empty() { "Add" } else { "+ Add" },
        &prefs.add_submit_btn,
        fonts,
    );
}

fn draw_section(
    canvas: &Canvas,
    fonts: &FontStore,
    layout: &FriendsLayout,
    start_y: f32,
    title: &str,
    rows: &[FriendRowView],
    section: RowSection,
) -> f32 {
    // Section header (mono eyebrow)
    let eyebrow_font = fonts.jetbrains_mono(10.0);
    let mut eyebrow_paint = Paint::default();
    eyebrow_paint.set_anti_alias(true);
    eyebrow_paint.set_color(TEXT_MAUVE);
    let (_, em) = eyebrow_font.metrics();
    canvas.draw_str(
        title,
        (layout.content_left, start_y + (-em.ascent)),
        &eyebrow_font,
        &eyebrow_paint,
    );
    // Hairline under the eyebrow
    let mut hl = Paint::default();
    hl.set_anti_alias(true);
    hl.set_color4f(Color4f::new(0.90, 0.72, 0.77, 0.10), None);
    hl.set_stroke_width(0.5);
    hl.set_style(PaintStyle::Stroke);
    canvas.draw_line(
        (layout.content_left, start_y + em.descent + 8.0),
        (layout.content_right, start_y + em.descent + 8.0),
        &hl,
    );

    let mut y = start_y + 24.0;
    for (i, row) in rows.iter().enumerate() {
        let row_h: f32 = 54.0;
        let row_rect = Rect::from_ltrb(
            layout.content_left,
            y,
            layout.content_right,
            y + row_h,
        );
        draw_friend_row(canvas, fonts, layout, &row_rect, row, section, i);
        y += row_h + 8.0;
    }
    y + 24.0
}

fn draw_friend_row(
    canvas: &Canvas,
    fonts: &FontStore,
    _layout: &FriendsLayout,
    row: &Rect,
    view: &FriendRowView,
    section: RowSection,
    _index: usize,
) {
    // Online/offline dot
    let dot_x = row.left + 14.0;
    let dot_y = (row.top + row.bottom) * 0.5;
    let mut dot = Paint::default();
    dot.set_anti_alias(true);
    dot.set_color(if view.online { ACCENT_LAV } else { TEXT_MAUVE_DEEP });
    canvas.draw_circle((dot_x, dot_y), 4.0, &dot);

    // Display name (Fraunces)
    let name_font = fonts.fraunces_axes(20.0, 50.0, 0.0, 500.0, None);
    let mut name_paint = Paint::default();
    name_paint.set_anti_alias(true);
    name_paint.set_color(TEXT_PEARL);
    let (_, nm) = name_font.metrics();
    canvas.draw_str(
        &view.display_name,
        (dot_x + 14.0, row.top + 22.0 + (-nm.ascent) * 0.0),
        &name_font,
        &name_paint,
    );

    // Presence subtext
    let pre_font = fonts.jetbrains_mono(10.0);
    let mut pre_paint = Paint::default();
    pre_paint.set_anti_alias(true);
    pre_paint.set_color(TEXT_MAUVE);
    let (_, pm) = pre_font.metrics();
    text::draw_tracked_em(
        canvas,
        &view.presence,
        (dot_x + 14.0, row.bottom - 12.0 + (-pm.ascent) * 0.0),
        &pre_font,
        &pre_paint,
        0.16,
    );

    // Action button(s) on the right
    let btn_h: f32 = 28.0;
    let btn_w: f32 = 80.0;
    let btn_y = (row.top + row.bottom) * 0.5 - btn_h * 0.5;
    match section {
        RowSection::Incoming => {
            let accept_rect = Rect::from_xywh(
                row.right - btn_w - 8.0, btn_y, btn_w, btn_h,
            );
            let decline_rect = Rect::from_xywh(
                accept_rect.left - btn_w - 8.0, btn_y, btn_w, btn_h,
            );
            draw_compact_label(canvas, fonts, accept_rect, "Accept", ACCENT_LAV);
            draw_compact_label(canvas, fonts, decline_rect, "Decline", TEXT_MAUVE);
        }
        RowSection::Friends => {
            let remove_rect = Rect::from_xywh(
                row.right - btn_w - 8.0, btn_y, btn_w, btn_h,
            );
            draw_compact_label(canvas, fonts, remove_rect, "Remove", ACCENT_EMBER);
        }
        RowSection::Outgoing => {
            // Outgoing requests are passive — show a "pending…" tag.
            let tag_rect = Rect::from_xywh(
                row.right - btn_w - 8.0,
                btn_y,
                btn_w,
                btn_h,
            );
            draw_compact_label(canvas, fonts, tag_rect, "pending", TEXT_MAUVE);
        }
    }

    // Hairline at the bottom of each row
    let mut sep = Paint::default();
    sep.set_anti_alias(true);
    sep.set_color4f(Color4f::new(0.90, 0.72, 0.77, 0.06), None);
    sep.set_stroke_width(0.5);
    sep.set_style(PaintStyle::Stroke);
    canvas.draw_line(
        (row.left + 28.0, row.bottom + 4.0),
        (row.right - 8.0, row.bottom + 4.0),
        &sep,
    );

    // Suppress unused-variable warnings for accent palette we keep
    // available for future variants (online sub-states, badge types).
    let _ = (ACCENT_BERRY, ACCENT_CHAMP);
}

fn draw_compact_label(canvas: &Canvas, fonts: &FontStore, rect: Rect, label: &str, color: Color) {
    // Thin ghost border + label — actions look like text buttons rather
    // than full vbtn pills to keep the row compact.
    let rr = RRect::new_rect_xy(rect, 6.0, 6.0);
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    let c4 = Color4f::new(
        (color.r() as f32) / 255.0,
        (color.g() as f32) / 255.0,
        (color.b() as f32) / 255.0,
        0.40,
    );
    border.set_color4f(c4, None);
    canvas.draw_rrect(rr, &border);

    let font = Font::new(fonts.newsreader_italic_typeface().clone(), 12.0);
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_color(color);
    let (_, m) = font.metrics();
    let advance = font.measure_str(label, Some(&p)).0;
    let baseline = (rect.top + rect.bottom) * 0.5 + (-m.ascent - m.descent) * 0.5;
    canvas.draw_str(
        label,
        ((rect.left + rect.right) * 0.5 - advance * 0.5, baseline),
        &font,
        &p,
    );
}

/// Standalone label-only vbtn helper. The real `draw_vbtn` wants `time`
/// and `motion_speed`; the Friends screen doesn't track those locally,
/// so the call site (`main.rs::draw_frame`) supplies them.
fn draw_vbtn_label_only(
    canvas: &Canvas,
    rect: Rect,
    label: &str,
    state: &VbtnState,
    fonts: &FontStore,
) {
    // Use a basic rounded-rect with the standard rose accent. We
    // deliberately don't reach into draw_vbtn so the screen module
    // stays decoupled from the per-frame `time`/`motion_speed` clock.
    let hover = state.hover;
    let radius = 12.0;
    let rrect = RRect::new_rect_xy(rect, radius, radius);
    let mut bg = Paint::default();
    bg.set_anti_alias(true);
    bg.set_color4f(
        Color4f::new(0.90, 0.72, 0.77, if hover { 0.20 } else { 0.12 }),
        None,
    );
    canvas.draw_rrect(rrect, &bg);
    let mut rim = Paint::default();
    rim.set_anti_alias(true);
    rim.set_style(PaintStyle::Stroke);
    rim.set_stroke_width(1.0);
    rim.set_color4f(
        Color4f::new(0.90, 0.72, 0.77, if hover { 0.55 } else { 0.30 }),
        None,
    );
    canvas.draw_rrect(rrect, &rim);

    let font = fonts.newsreader(14.0);
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_color(TEXT_PEARL_HOT);
    let (_, m) = font.metrics();
    let advance = font.measure_str(label, Some(&p)).0;
    let baseline = (rect.top + rect.bottom) * 0.5 + (-m.ascent - m.descent) * 0.5;
    canvas.draw_str(
        label,
        ((rect.left + rect.right) * 0.5 - advance * 0.5, baseline),
        &font,
        &p,
    );
    // Suppress unused-variable: we may still want to differentiate
    // primary vs secondary vbtn later via the draw_vbtn pipeline.
    let _ = draw_vbtn;
    let _ = draw_vghost_btn;
    let _ = VghostBtnState::default();
    let _ = GhostKind::Pearl;
    let _ = CubicBezier::SILK;
    let _ = Point::new(0.0, 0.0);
}

fn draw_centered_message(
    canvas: &Canvas,
    fonts: &FontStore,
    card_w: f32,
    card_h: f32,
    msg: &str,
    color: Color,
) {
    let f = fonts.newsreader(15.0);
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_color(color);
    let (_, m) = f.metrics();
    let advance = f.measure_str(msg, Some(&p)).0;
    let baseline = card_h * 0.5 + (-m.ascent - m.descent) * 0.5;
    canvas.draw_str(msg, (card_w * 0.5 - advance * 0.5, baseline), &f, &p);
}
