//! Pure click decisions: given card-local geometry and a point, which target
//! was hit and what should the click do? No `App`, no side effects, so every
//! rule is unit-tested here; the handlers in `events::click` apply the result.
//!
//! The order each function checks its targets in is the launcher's hit
//! priority (nested widgets first) and is part of the behaviour.

use ewo_core::Screen;
use ewo_render::screens;
use ewo_render::screens::RowButtonKind;
use ewo_render::skia_safe::Rect;
use winit::window::ResizeDirection;

use crate::window_hit::{rect_contains, Zone};

// ── Window chrome ───────────────────────────────────────────────────────

/// A top-right window button.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum WindowButton {
    Close,
    Minimize,
}

/// Which window button (if any) the card-local point is on.
pub(crate) fn window_button_at(pos: (f32, f32), card_w: f32) -> Option<WindowButton> {
    let (min_btn, close_btn) = ewo_render::app_window::window_button_bounds(card_w);
    if rect_contains(&close_btn, pos) {
        Some(WindowButton::Close)
    } else if rect_contains(&min_btn, pos) {
        Some(WindowButton::Minimize)
    } else {
        None
    }
}

/// What a click nothing else claimed does to the window.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum FrameAction {
    /// Start dragging the window (caption strip).
    Drag,
    /// Start a border resize.
    Resize(ResizeDirection),
    /// Plain content: nudge the backdrop particles.
    DisturbBackdrop,
}

pub(crate) fn frame_action(zone: Option<Zone>) -> FrameAction {
    match zone {
        Some(Zone::Caption) => FrameAction::Drag,
        Some(Zone::Resize(dir)) => FrameAction::Resize(dir),
        None => FrameAction::DisturbBackdrop,
    }
}

// ── Navigation ──────────────────────────────────────────────────────────

/// The first tab whose rect contains the point (top tab bar and the
/// Settings sidebar both resolve this way).
pub(crate) fn tab_at<T>(tabs: impl IntoIterator<Item = (T, Rect)>, pos: (f32, f32)) -> Option<T> {
    tabs.into_iter()
        .find(|(_, rect)| rect_contains(rect, pos))
        .map(|(tab, _)| tab)
}

/// The "‹ Main menu" back-link's card-local rect (top-left of the
/// screen-head screens).
pub(crate) fn back_link_rect() -> Rect {
    Rect::from_xywh(34.0, 38.0, 180.0, 42.0)
}

/// Whether the point is on the back-link of `screen` (only Settings and
/// Instances have one).
pub(crate) fn back_link_hit(screen: Screen, pos: (f32, f32)) -> bool {
    matches!(screen, Screen::Settings | Screen::Instances) && rect_contains(&back_link_rect(), pos)
}

/// The main-menu network widget joins the lobby only when the network
/// reports online and a lobby address is configured.
pub(crate) fn lobby_join_target(online: bool, lobby: Option<&str>) -> Option<String> {
    match (online, lobby) {
        (true, Some(addr)) => Some(addr.to_string()),
        _ => None,
    }
}

// ── Modals ──────────────────────────────────────────────────────────────

/// How a modal was dismissed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Dismiss {
    /// Its own Close/Cancel button.
    Button,
    /// A press on the shroud outside the card.
    Shroud,
}

/// The About modal closes from its button or a shroud press; a press
/// anywhere else inside the card does nothing.
pub(crate) fn about_dismissal(
    close_clicked: bool,
    pressed: bool,
    on_shroud: bool,
) -> Option<Dismiss> {
    if close_clicked {
        Some(Dismiss::Button)
    } else if pressed && on_shroud {
        Some(Dismiss::Shroud)
    } else {
        None
    }
}

/// What a click does in the delete-instance confirmation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum DeleteModalAction {
    Cancel,
    /// Start moving the folder to the Recycle Bin.
    Confirm,
    /// Shroud press: close like Cancel.
    Shroud,
    Nothing,
}

/// Cancel beats Confirm beats a shroud press, and while the move runs
/// (`busy`) nothing does anything: it can neither be cancelled half way nor
/// started twice.
pub(crate) fn delete_modal_action(
    cancel_clicked: bool,
    confirm_clicked: bool,
    busy: bool,
    pressed: bool,
    on_shroud: bool,
) -> DeleteModalAction {
    if busy {
        DeleteModalAction::Nothing
    } else if cancel_clicked {
        DeleteModalAction::Cancel
    } else if confirm_clicked {
        DeleteModalAction::Confirm
    } else if pressed && on_shroud {
        DeleteModalAction::Shroud
    } else {
        DeleteModalAction::Nothing
    }
}

/// What a click does in the launcher-link modal.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum LinkModalAction {
    Cancel,
    /// Submit the entered code (only when it is ready).
    Submit,
    /// Shroud press: close like Cancel.
    Shroud,
    Nothing,
}

/// Cancel beats Submit beats a shroud press. A Submit click while the code
/// isn't ready is ignored, and then a press on the shroud still closes.
pub(crate) fn link_modal_action(
    cancel_clicked: bool,
    submit_clicked: bool,
    code_ready: bool,
    pressed: bool,
    on_shroud: bool,
) -> LinkModalAction {
    if cancel_clicked {
        LinkModalAction::Cancel
    } else if submit_clicked && code_ready {
        LinkModalAction::Submit
    } else if pressed && on_shroud {
        LinkModalAction::Shroud
    } else {
        LinkModalAction::Nothing
    }
}

// ── Friends ─────────────────────────────────────────────────────────────

/// What a press on the Friends screen does.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum FriendsClick {
    /// Nothing to act on (social off, or a miss while not linked).
    Nothing,
    /// Not linked: the centred button opens the launcher-link modal.
    OpenLinkModal,
    /// Focus the add-friend text input.
    FocusAddInput,
    /// The "+ Add" button.
    SubmitAdd,
    /// A miss that still drops the add-input's focus.
    Defocus,
    /// The first row button under the cursor. Also drops the input focus.
    Row { kind: RowButtonKind, index: usize },
}

/// `social_enabled`: a social base URL is configured. `linked`: the active
/// account has a social token. A row button under the cursor is returned
/// even when it turns out to have no target; the caller decides.
pub(crate) fn friends_click(
    layout: &screens::FriendsLayout,
    pos: (f32, f32),
    social_enabled: bool,
    linked: bool,
) -> FriendsClick {
    if !social_enabled {
        return FriendsClick::Nothing;
    }
    if !linked {
        return if rect_contains(&layout.link_launcher_btn, pos) {
            FriendsClick::OpenLinkModal
        } else {
            FriendsClick::Nothing
        };
    }
    if rect_contains(&layout.add_input, pos) {
        FriendsClick::FocusAddInput
    } else if rect_contains(&layout.add_submit, pos) {
        FriendsClick::SubmitAdd
    } else {
        layout
            .row_buttons
            .iter()
            .find(|btn| rect_contains(&btn.rect, pos))
            .map_or(FriendsClick::Defocus, |btn| FriendsClick::Row {
                kind: btn.kind,
                index: btn.index,
            })
    }
}

// ── Instances ───────────────────────────────────────────────────────────

/// Card-local hit rects of the Instances list. Without fonts only the "+"
/// button can be placed.
#[derive(Clone, Debug)]
pub(crate) struct InstancesListGeometry {
    pub(crate) plus: Rect,
    pub(crate) rename: Option<Rect>,
    /// One per visible row, in display order.
    pub(crate) rows: Vec<Rect>,
    pub(crate) sort: Option<Rect>,
}

/// What a press on the Instances list does. Row indices are *display*
/// positions; the caller maps them back through `display_order`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstancesListClick {
    /// "+": open the new-instance modal.
    Add,
    /// ✎: rename the selected instance.
    Rename,
    /// ×: delete the row.
    Delete(usize),
    /// Select the row.
    Select(usize),
    /// Cycle the sort mode.
    CycleSort,
}

/// Priority: "+", rename, per-row × (it sits inside the row's hit rect, so
/// it must win), row select, sort.
pub(crate) fn instances_list_click(
    g: &InstancesListGeometry,
    pos: (f32, f32),
) -> Option<InstancesListClick> {
    if rect_contains(&g.plus, pos) {
        return Some(InstancesListClick::Add);
    }
    if g.rename.is_some_and(|r| rect_contains(&r, pos)) {
        return Some(InstancesListClick::Rename);
    }
    if let Some(i) = g
        .rows
        .iter()
        .position(|row| rect_contains(&screens::instances::delete_button_bounds(*row), pos))
    {
        return Some(InstancesListClick::Delete(i));
    }
    if let Some(i) = g.rows.iter().position(|row| rect_contains(row, pos)) {
        return Some(InstancesListClick::Select(i));
    }
    if g.sort.is_some_and(|r| rect_contains(&r, pos)) {
        return Some(InstancesListClick::CycleSort);
    }
    None
}

// ── Launching ───────────────────────────────────────────────────────────

/// The Launching screen's footer buttons (shown after a failed launch).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum LaunchingClick {
    Retry,
    Back,
}

pub(crate) fn launching_click(pos: (f32, f32), card_w: f32, card_h: f32) -> Option<LaunchingClick> {
    if rect_contains(
        &screens::launching::retry_button_bounds(card_w, card_h),
        pos,
    ) {
        Some(LaunchingClick::Retry)
    } else if rect_contains(
        &screens::launching::cancel_button_bounds(card_w, card_h),
        pos,
    ) {
        Some(LaunchingClick::Back)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::test_support::{center, fonts, CARD_H, CARD_W, NOWHERE};

    fn rect(l: f32, t: f32, r: f32, b: f32) -> Rect {
        Rect::from_ltrb(l, t, r, b)
    }

    // ── chrome ──

    #[test]
    fn window_buttons_resolve_close_and_minimize() {
        let (min, close) = ewo_render::app_window::window_button_bounds(CARD_W);
        assert_eq!(
            window_button_at(center(&close), CARD_W),
            Some(WindowButton::Close)
        );
        assert_eq!(
            window_button_at(center(&min), CARD_W),
            Some(WindowButton::Minimize)
        );
        assert_eq!(window_button_at((CARD_W * 0.5, 300.0), CARD_W), None);
        assert_eq!(window_button_at(NOWHERE, CARD_W), None);
    }

    #[test]
    fn frame_action_maps_each_zone() {
        assert_eq!(frame_action(Some(Zone::Caption)), FrameAction::Drag);
        assert_eq!(
            frame_action(Some(Zone::Resize(ResizeDirection::SouthWest))),
            FrameAction::Resize(ResizeDirection::SouthWest)
        );
        assert_eq!(frame_action(None), FrameAction::DisturbBackdrop);
    }

    // ── navigation ──

    #[test]
    fn tab_at_returns_the_first_tab_under_the_point() {
        let tabs = [
            ('a', rect(0.0, 0.0, 10.0, 10.0)),
            ('b', rect(10.0, 0.0, 20.0, 10.0)),
            ('c', rect(5.0, 0.0, 15.0, 10.0)),
        ];
        assert_eq!(tab_at(tabs, (3.0, 5.0)), Some('a'));
        // On the shared edge the earlier tab wins (rects are inclusive).
        assert_eq!(tab_at(tabs, (10.0, 5.0)), Some('a'));
        assert_eq!(tab_at(tabs, (17.0, 5.0)), Some('b'));
        assert_eq!(tab_at(tabs, (30.0, 5.0)), None);
        assert_eq!(tab_at(Vec::<(char, Rect)>::new(), (1.0, 1.0)), None);
    }

    #[test]
    fn the_real_tab_bar_resolves_every_tab_by_its_centre() {
        let Some(fonts) = fonts() else { return };
        let bounds = screens::tab_bounds(CARD_W, &fonts);
        assert!(!bounds.is_empty());
        for (screen, r) in &bounds {
            assert_eq!(tab_at(bounds.clone(), center(r)), Some(*screen));
        }
        assert_eq!(tab_at(bounds, (CARD_W * 0.5, CARD_H - 5.0)), None);
    }

    #[test]
    fn the_back_link_exists_only_on_settings_and_instances() {
        let inside = center(&back_link_rect());
        assert!(back_link_hit(Screen::Settings, inside));
        assert!(back_link_hit(Screen::Instances, inside));
        for s in [Screen::MainMenu, Screen::Friends, Screen::Launching] {
            assert!(!back_link_hit(s, inside), "{s:?}");
        }
        assert!(!back_link_hit(Screen::Settings, NOWHERE));
    }

    #[test]
    fn the_lobby_widget_joins_only_when_online_with_an_address() {
        assert_eq!(
            lobby_join_target(true, Some("play.example:25565")).as_deref(),
            Some("play.example:25565")
        );
        assert_eq!(lobby_join_target(false, Some("play.example")), None);
        assert_eq!(lobby_join_target(true, None), None);
        assert_eq!(lobby_join_target(false, None), None);
    }

    // ── modals ──

    #[test]
    fn about_modal_closes_from_its_button_or_the_shroud_only() {
        assert_eq!(about_dismissal(true, true, false), Some(Dismiss::Button));
        assert_eq!(about_dismissal(true, false, false), Some(Dismiss::Button));
        assert_eq!(about_dismissal(false, true, true), Some(Dismiss::Shroud));
        // A press inside the card, or a release on the shroud, is inert.
        assert_eq!(about_dismissal(false, true, false), None);
        assert_eq!(about_dismissal(false, false, true), None);
    }

    #[test]
    fn delete_modal_priority_is_cancel_confirm_shroud_and_busy_is_inert() {
        use DeleteModalAction::*;
        assert_eq!(delete_modal_action(true, true, false, true, true), Cancel);
        assert_eq!(
            delete_modal_action(false, true, false, true, false),
            Confirm
        );
        assert_eq!(delete_modal_action(false, false, false, true, true), Shroud);
        // A press inside the card, or a release on the shroud, is inert.
        assert_eq!(
            delete_modal_action(false, false, false, true, false),
            Nothing
        );
        assert_eq!(
            delete_modal_action(false, false, false, false, true),
            Nothing
        );
        // While the folder is moving nothing can be clicked, so it can be
        // neither cancelled half way nor started twice.
        for (cancel, confirm, shroud) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            assert_eq!(
                delete_modal_action(cancel, confirm, true, true, shroud),
                Nothing
            );
        }
    }

    #[test]
    fn delete_modal_hit_rects_match_the_drawn_buttons() {
        use ewo_render::screens::delete_instance_modal as m;
        let (w, h) = (1100.0, 700.0);
        let c =
            |r: ewo_render::skia_safe::Rect| ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
        let mut cancel = ewo_render::widgets::VghostBtnState::default();
        let mut confirm = ewo_render::widgets::VghostBtnState::default();
        let (cb, kb) = (
            m::cancel_button_bounds(w, h),
            m::confirm_button_bounds(w, h),
        );
        // The centre of each button hits only that button, off the shroud.
        assert!(cancel.handle(c(cb), cb, true) && !confirm.handle(c(cb), kb, true));
        assert!(confirm.handle(c(kb), kb, true) && !cancel.handle(c(kb), cb, true));
        assert!(!m::shroud_consumes(c(cb), w, h) && !m::shroud_consumes(c(kb), w, h));
        assert!(m::shroud_consumes((1.0, 1.0), w, h));
    }

    #[test]
    fn link_modal_priority_is_cancel_submit_shroud() {
        use LinkModalAction::*;
        assert_eq!(link_modal_action(true, true, true, true, true), Cancel);
        assert_eq!(link_modal_action(false, true, true, true, false), Submit);
        // Submit with an incomplete code is ignored...
        assert_eq!(link_modal_action(false, true, false, true, false), Nothing);
        // ...and then a shroud press still dismisses.
        assert_eq!(link_modal_action(false, true, false, true, true), Shroud);
        assert_eq!(link_modal_action(false, false, true, true, true), Shroud);
        assert_eq!(link_modal_action(false, false, true, false, true), Nothing);
    }

    // ── friends ──

    fn friends_layout_fixture(fonts: &ewo_render::FontStore) -> screens::FriendsLayout {
        screens::friends_layout(
            CARD_W,
            fonts,
            screens::FriendsCounts {
                friends: 2,
                incoming: 1,
                outgoing: 0,
            },
        )
    }

    #[test]
    fn friends_clicks_are_inert_when_social_is_off() {
        let Some(fonts) = fonts() else { return };
        let layout = friends_layout_fixture(&fonts);
        for at in [
            center(&layout.link_launcher_btn),
            center(&layout.add_input),
            center(&layout.add_submit),
        ] {
            assert_eq!(
                friends_click(&layout, at, false, true),
                FriendsClick::Nothing
            );
            assert_eq!(
                friends_click(&layout, at, false, false),
                FriendsClick::Nothing
            );
        }
    }

    #[test]
    fn an_unlinked_friends_screen_only_offers_the_link_button() {
        let Some(fonts) = fonts() else { return };
        let layout = friends_layout_fixture(&fonts);
        assert_eq!(
            friends_click(&layout, center(&layout.link_launcher_btn), true, false),
            FriendsClick::OpenLinkModal
        );
        assert_eq!(
            friends_click(&layout, center(&layout.add_submit), true, false),
            FriendsClick::Nothing
        );
        assert_eq!(
            friends_click(&layout, NOWHERE, true, false),
            FriendsClick::Nothing
        );
    }

    #[test]
    fn a_linked_friends_screen_routes_input_submit_rows_and_misses() {
        let Some(fonts) = fonts() else { return };
        let layout = friends_layout_fixture(&fonts);
        assert_eq!(
            friends_click(&layout, center(&layout.add_input), true, true),
            FriendsClick::FocusAddInput
        );
        assert_eq!(
            friends_click(&layout, center(&layout.add_submit), true, true),
            FriendsClick::SubmitAdd
        );
        assert_eq!(
            friends_click(&layout, NOWHERE, true, true),
            FriendsClick::Defocus
        );
        assert!(!layout.row_buttons.is_empty(), "fixture has rows");
        for btn in &layout.row_buttons {
            let got = friends_click(&layout, center(&btn.rect), true, true);
            // Overlapping buttons resolve to the first in display order, so
            // only the routing (not which button) is asserted here.
            assert!(matches!(got, FriendsClick::Row { .. }), "{got:?}");
        }
        // The Join button of a friend row is reported, not filtered.
        let join = layout
            .row_buttons
            .iter()
            .find(|b| b.kind == RowButtonKind::Join)
            .expect("a friends row has a Join slot");
        assert!(matches!(
            friends_click(&layout, center(&join.rect), true, true),
            FriendsClick::Row { kind: RowButtonKind::Join, index } if index == join.index
        ));
    }

    // ── instances ──

    fn geometry() -> InstancesListGeometry {
        InstancesListGeometry {
            plus: rect(300.0, 0.0, 330.0, 30.0),
            rename: Some(rect(200.0, 0.0, 230.0, 30.0)),
            rows: vec![
                rect(10.0, 100.0, 330.0, 160.0),
                rect(10.0, 170.0, 330.0, 230.0),
            ],
            sort: Some(rect(100.0, 60.0, 180.0, 80.0)),
        }
    }

    #[test]
    fn instances_list_hits_each_target() {
        let g = geometry();
        assert_eq!(
            instances_list_click(&g, center(&g.plus)),
            Some(InstancesListClick::Add)
        );
        assert_eq!(
            instances_list_click(&g, center(&g.rename.unwrap())),
            Some(InstancesListClick::Rename)
        );
        assert_eq!(
            instances_list_click(&g, center(&g.sort.unwrap())),
            Some(InstancesListClick::CycleSort)
        );
        // The row body selects; the second row is display index 1.
        assert_eq!(
            instances_list_click(&g, (20.0, 200.0)),
            Some(InstancesListClick::Select(1))
        );
        assert_eq!(instances_list_click(&g, NOWHERE), None);
    }

    #[test]
    fn the_delete_cross_wins_over_its_own_row() {
        let g = geometry();
        let cross = screens::instances::delete_button_bounds(g.rows[1]);
        assert!(rect_contains(&g.rows[1], center(&cross)), "cross is inside");
        assert_eq!(
            instances_list_click(&g, center(&cross)),
            Some(InstancesListClick::Delete(1))
        );
    }

    #[test]
    fn without_fonts_only_the_plus_button_is_live() {
        let g = InstancesListGeometry {
            plus: rect(300.0, 0.0, 330.0, 30.0),
            rename: None,
            rows: Vec::new(),
            sort: None,
        };
        assert_eq!(
            instances_list_click(&g, center(&g.plus)),
            Some(InstancesListClick::Add)
        );
        assert_eq!(instances_list_click(&g, (20.0, 200.0)), None);
    }

    // ── launching ──

    #[test]
    fn launching_footer_buttons_resolve_retry_and_back() {
        let retry = screens::launching::retry_button_bounds(CARD_W, CARD_H);
        let back = screens::launching::cancel_button_bounds(CARD_W, CARD_H);
        assert_eq!(
            launching_click(center(&retry), CARD_W, CARD_H),
            Some(LaunchingClick::Retry)
        );
        assert_eq!(
            launching_click(center(&back), CARD_W, CARD_H),
            Some(LaunchingClick::Back)
        );
        assert_eq!(launching_click(NOWHERE, CARD_W, CARD_H), None);
    }
}
