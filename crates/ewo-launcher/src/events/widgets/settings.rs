//! Settings-screen pointer input: slider drags and hover (`drive_settings_sliders`)
//! and press routing (`handle_settings_press`), each a dispatcher over one
//! function per tab.

use super::super::*;

/// Drive any slider on the active Settings tab with the current cursor +
/// mouse-button state. Continuous-drive semantics: on rising edge inside
/// bounds the slider starts dragging; while dragging, value tracks `mouse.x`
/// even if the cursor leaves the widget bounds; on falling edge dragging
/// ends. See `VsliderState::drive` for the per-widget contract.
///
/// Also updates dropdown menu hover state for any open dropdown so cursor
/// motion lights up the row under the cursor.
pub(crate) fn drive_settings_sliders(
    prefs: &mut Prefs,
    tab: SettingsTab,
    fonts: Option<&FontStore>,
    mouse: (f32, f32),
    mouse_down: bool,
    card_w: f32,
    card_h: f32,
    account_uuids: &[String],
    profile_names: &[String],
    link_status: LinkStatusView,
) -> bool {
    let Some(fonts) = fonts else {
        return false;
    };
    if tab == SettingsTab::Account {
        return drive_account_tab(prefs, fonts, mouse, card_w, account_uuids, link_status);
    }
    if tab == SettingsTab::Profiles {
        return drive_profiles_tab(prefs, fonts, mouse, card_w, profile_names);
    }
    if tab == SettingsTab::Keybinds {
        return drive_keybinds_tab(prefs, fonts, mouse, card_w, card_h);
    }
    if tab == SettingsTab::Modules {
        return drive_modules_tab(prefs, fonts, mouse, mouse_down, card_w, card_h);
    }
    if tab == SettingsTab::PvpUtils {
        return drive_pvp_tab(prefs, fonts, mouse, mouse_down, card_w, card_h);
    }
    drive_general_tab(prefs, tab, fonts, mouse, mouse_down, card_w, card_h)
}

/// Account tab — custom layout. Update row / remove / add-button hover.
fn drive_account_tab(
    prefs: &mut Prefs,
    fonts: &FontStore,
    mouse: (f32, f32),
    card_w: f32,
    account_uuids: &[String],
    link_status: LinkStatusView,
) -> bool {
    let layout =
        screens::settings::account_tab_layout(fonts, card_w, account_uuids.len(), link_status);
    prefs.account_hover = None;
    for rl in &layout.rows {
        if rect_contains(&rl.remove, mouse) {
            prefs.account_hover = Some(AccountHover::Remove(rl.index));
        } else if rect_contains(&rl.row, mouse) {
            prefs.account_hover = Some(AccountHover::Row(rl.index));
        }
    }
    prefs.account_add.handle(mouse, layout.add_button, false);
    false
}

/// Profiles tab — update row / delete / button hover.
fn drive_profiles_tab(
    prefs: &mut Prefs,
    fonts: &FontStore,
    mouse: (f32, f32),
    card_w: f32,
    profile_names: &[String],
) -> bool {
    let layout = screens::settings::profiles_tab_layout(fonts, card_w, profile_names.len());
    let can_delete = profile_names.len() > 1;
    prefs.profile_hover = None;
    for rl in &layout.rows {
        if can_delete && rect_contains(&rl.delete, mouse) {
            prefs.profile_hover = Some(ProfileHover::Delete(rl.index));
        } else if rect_contains(&rl.rename, mouse) {
            prefs.profile_hover = Some(ProfileHover::Rename(rl.index));
        } else if rect_contains(&rl.row, mouse) {
            prefs.profile_hover = Some(ProfileHover::Row(rl.index));
        }
    }
    prefs.profile_new.handle(mouse, layout.new_button, false);
    prefs.profile_dup.handle(mouse, layout.dup_button, false);
    false
}

/// Keybinds tab — update chord + reset hover within the scrolled list.
fn drive_keybinds_tab(
    prefs: &mut Prefs,
    fonts: &FontStore,
    mouse: (f32, f32),
    card_w: f32,
    card_h: f32,
) -> bool {
    let layout = screens::settings::keybinds_tab_layout(
        fonts,
        card_w,
        card_h,
        keybind::REGISTRY.len(),
        prefs.settings_scroll,
    );
    let max = (layout.content_h - layout.list_region.height()).max(0.0);
    prefs.settings_scroll = prefs.settings_scroll.clamp(0.0, max);
    let in_list = rect_contains(&layout.list_region, mouse);
    prefs.keybind_hover = None;
    if in_list {
        for rl in &layout.rows {
            if rect_contains(&rl.chord, mouse) {
                prefs.keybind_hover = Some(rl.index);
            }
        }
    }
    let probe = if in_list { mouse } else { (-1.0, -1.0) };
    prefs
        .keybind_reset
        .handle(probe, layout.reset_button, false);
    false
}

/// Modules tab — toggle hover + the FOV slider, within the scrolled list.
fn drive_modules_tab(
    prefs: &mut Prefs,
    fonts: &FontStore,
    mouse: (f32, f32),
    mouse_down: bool,
    card_w: f32,
    card_h: f32,
) -> bool {
    let layout =
        screens::settings::modules_tab_layout(fonts, card_w, card_h, prefs.settings_scroll);
    let max = (layout.content_h - layout.list_region.height()).max(0.0);
    prefs.settings_scroll = prefs.settings_scroll.clamp(0.0, max);
    let in_list = rect_contains(&layout.list_region, mouse);
    let probe = if in_list { mouse } else { (-1.0, -1.0) };
    for rl in &layout.rows {
        if let Some(toggle) = prefs.module_toggles.get_mut(rl.index) {
            toggle.handle(probe, rl.toggle, false);
        }
        if let Some(slider) = rl.slider {
            if prefs.module_fov.drive(mouse, slider, mouse_down) {
                prefs.modules_changed = true;
            }
        }
    }
    false
}

/// PvP-Utils tab — slider drag and release. Press dispatch (toggles + chip
/// cycles + drag start) lives in the MouseInput Pressed branch.
fn drive_pvp_tab(
    prefs: &mut Prefs,
    fonts: &FontStore,
    mouse: (f32, f32),
    mouse_down: bool,
    card_w: f32,
    card_h: f32,
) -> bool {
    let layout = screens::settings::pvp_tab_layout(fonts, card_w, card_h, prefs.settings_scroll);
    let max = (layout.content_h - layout.list_region.height()).max(0.0);
    prefs.settings_scroll = prefs.settings_scroll.clamp(0.0, max);
    if prefs.pvp_drag.is_some() {
        screens::settings::drive_pvp_drag(prefs, fonts, card_w, card_h, mouse.0);
        if !mouse_down {
            screens::settings::end_pvp_drag(prefs);
        }
    }
    false
}

/// The row-grid tabs (Graphics, Audio, Paths, Advanced): slider drags, path-field
/// and button hover, and the open dropdown's menu-row hover.
fn drive_general_tab(
    prefs: &mut Prefs,
    tab: SettingsTab,
    fonts: &FontStore,
    mouse: (f32, f32),
    mouse_down: bool,
    card_w: f32,
    card_h: f32,
) -> bool {
    let mut changed = false;
    for (slot, rect) in screens::settings::widget_bounds(tab, fonts, card_w, card_h) {
        match slot {
            SettingsSlot::MaxFps => {
                if prefs.max_fps.drive(mouse, rect, mouse_down) {
                    changed = true;
                }
            }
            SettingsSlot::Master => {
                if prefs.master.drive(mouse, rect, mouse_down) {
                    changed = true;
                }
            }
            SettingsSlot::Music => {
                if prefs.music.drive(mouse, rect, mouse_down) {
                    changed = true;
                }
            }
            SettingsSlot::Effects => {
                if prefs.effects.drive(mouse, rect, mouse_down) {
                    changed = true;
                }
            }
            // Path fields and Reset button drive their own hover state.
            SettingsSlot::GameDir => {
                prefs.game_dir.drive_hover(mouse, rect);
            }
            SettingsSlot::Downloads => {
                prefs.downloads.drive_hover(mouse, rect);
            }
            SettingsSlot::ResetPrefs => {
                prefs.reset_prefs.handle(mouse, rect, false);
            }
            // Toggles + dropdowns react to discrete press events, not motion.
            SettingsSlot::Vsync
            | SettingsSlot::AmbientHum
            | SettingsSlot::AutoBackup
            | SettingsSlot::Telemetry
            | SettingsSlot::WindowMode
            | SettingsSlot::Theme
            | SettingsSlot::LogLevel => {}
        }
    }

    // Update menu-row hover for any open dropdown.
    if let Some(slot) = prefs.open_dropdown() {
        if let Some(opts) = screens::settings::dropdown_options(slot) {
            if let Some(head) =
                screens::settings::dropdown_head_for_slot(slot, fonts, card_w, card_h)
            {
                let (menu_bounds, _flip) =
                    ewo_render::widgets::menu_layout(head, opts.len(), card_h);
                if let Some(state) = prefs.dropdown_state_mut(slot) {
                    state.update_menu_hover(mouse, menu_bounds, opts.len());
                }
            }
        }
    }

    changed
}

/// Route a press event on the Settings screen. Returns `true` when the
/// press hit a Settings widget (and so should be considered "handled" and
/// not fall through to the launch-button or window-drag handlers).
///
/// Order of resolution:
///   1. If a dropdown is open and the press is inside its menu → commit row
///      and consume the press.
///   2. If a dropdown is open and the press is *outside* both its head and
///      its menu → close the menu and consume the press.
///   3. Otherwise iterate the active tab's widgets:
///      - dropdown head → toggle open/closed (closing any other open one)
///      - toggle → flip
///      - slider → no-op (already handled by `drive_settings_sliders`)
pub(crate) fn handle_settings_press(
    prefs: &mut Prefs,
    tab: SettingsTab,
    fonts: Option<&FontStore>,
    mouse: (f32, f32),
    card_w: f32,
    card_h: f32,
    account_uuids: &[String],
    profile_names: &[String],
    link_status: LinkStatusView,
) -> (bool, bool) {
    let Some(fonts) = fonts else {
        return (false, false);
    };

    if tab == SettingsTab::Account {
        return press_account_tab(prefs, fonts, mouse, card_w, account_uuids, link_status);
    }

    if tab == SettingsTab::Profiles {
        return press_profiles_tab(prefs, fonts, mouse, card_w, profile_names);
    }

    if tab == SettingsTab::Keybinds {
        return press_keybinds_tab(prefs, fonts, mouse, card_w, card_h);
    }

    if tab == SettingsTab::Modules {
        return press_modules_tab(prefs, fonts, mouse, card_w, card_h);
    }

    if tab == SettingsTab::PvpUtils {
        return press_pvp_tab(prefs, fonts, mouse, card_w, card_h);
    }

    press_general_tab(prefs, tab, fonts, mouse, card_w, card_h)
}

/// Account tab — custom layout, handled outside the row-grid dispatch.
/// Remove buttons are nested inside row rects, so test them first.
fn press_account_tab(
    prefs: &mut Prefs,
    fonts: &FontStore,
    mouse: (f32, f32),
    card_w: f32,
    account_uuids: &[String],
    link_status: LinkStatusView,
) -> (bool, bool) {
    let layout =
        screens::settings::account_tab_layout(fonts, card_w, account_uuids.len(), link_status);
    for rl in &layout.rows {
        if rect_contains(&rl.remove, mouse) {
            if let Some(uuid) = account_uuids.get(rl.index) {
                log::info!("account: remove {}", uuid);
                prefs.account_request = Some(AccountRequest::Remove(uuid.clone()));
            }
            return (true, false);
        }
    }
    for rl in &layout.rows {
        if rect_contains(&rl.row, mouse) {
            if let Some(uuid) = account_uuids.get(rl.index) {
                prefs.account_request = Some(AccountRequest::SetActive(uuid.clone()));
            }
            return (true, false);
        }
    }
    // Phase H2: clickable link-status line (only when status is
    // `LinkedNeedsLauncherLink`, in which case `link_line_bounds`
    // is `Some`).
    if let Some(bounds) = layout.link_line_bounds {
        if rect_contains(&bounds, mouse) {
            log::info!("account: link line clicked → open launcher-link modal");
            prefs.account_request = Some(AccountRequest::OpenLauncherLink);
            return (true, false);
        }
    }
    if prefs.account_add.handle(mouse, layout.add_button, true) {
        log::info!("account: add-account button clicked");
        prefs.account_request = Some(AccountRequest::Add);
        return (true, false);
    }
    (false, false)
}

/// Profiles tab — custom layout. Delete buttons nest inside rows, so
/// test them first; then row clicks (switch); then the action buttons.
fn press_profiles_tab(
    prefs: &mut Prefs,
    fonts: &FontStore,
    mouse: (f32, f32),
    card_w: f32,
    profile_names: &[String],
) -> (bool, bool) {
    let layout = screens::settings::profiles_tab_layout(fonts, card_w, profile_names.len());
    // A click anywhere while renaming commits the in-progress rename.
    if let Some(idx) = prefs.profile_renaming {
        prefs.profile_request = Some(ProfileRequest::Rename {
            index: idx,
            new_name: prefs.profile_rename_buffer.clone(),
        });
        prefs.profile_renaming = None;
        return (true, false);
    }
    let can_delete = profile_names.len() > 1;
    // Rename buttons nest inside rows — test them before row clicks.
    for rl in &layout.rows {
        if rect_contains(&rl.rename, mouse) {
            if let Some(name) = profile_names.get(rl.index) {
                log::info!("profile: rename \"{}\" — entering edit", name);
                prefs.profile_renaming = Some(rl.index);
                prefs.profile_rename_buffer = name.clone();
                prefs.profile_rename_focus_time = 0.0;
            }
            return (true, false);
        }
    }
    if can_delete {
        for rl in &layout.rows {
            if rect_contains(&rl.delete, mouse) {
                if let Some(name) = profile_names.get(rl.index) {
                    log::info!("profile: delete \"{}\"", name);
                    prefs.profile_request = Some(ProfileRequest::Delete(name.clone()));
                }
                return (true, false);
            }
        }
    }
    for rl in &layout.rows {
        if rect_contains(&rl.row, mouse) {
            if let Some(name) = profile_names.get(rl.index) {
                prefs.profile_request = Some(ProfileRequest::Switch(name.clone()));
            }
            return (true, false);
        }
    }
    if prefs.profile_new.handle(mouse, layout.new_button, true) {
        prefs.profile_request = Some(ProfileRequest::New);
        return (true, false);
    }
    if prefs.profile_dup.handle(mouse, layout.dup_button, true) {
        prefs.profile_request = Some(ProfileRequest::Duplicate);
        return (true, false);
    }
    (false, false)
}

/// Keybinds tab — custom layout. A chord button arms a rebind; the reset
/// button restores every registry default. Only acts inside the viewport.
fn press_keybinds_tab(
    prefs: &mut Prefs,
    fonts: &FontStore,
    mouse: (f32, f32),
    card_w: f32,
    card_h: f32,
) -> (bool, bool) {
    let layout = screens::settings::keybinds_tab_layout(
        fonts,
        card_w,
        card_h,
        keybind::REGISTRY.len(),
        prefs.settings_scroll,
    );
    if rect_contains(&layout.list_region, mouse) {
        for rl in &layout.rows {
            if rect_contains(&rl.chord, mouse) {
                prefs.keybind_request = Some(KeybindRequest::Capture(rl.index));
                return (true, false);
            }
        }
        if prefs.keybind_reset.handle(mouse, layout.reset_button, true) {
            prefs.keybind_request = Some(KeybindRequest::ResetAll);
            return (true, false);
        }
    }
    (false, false)
}

/// Modules tab — a toggle click flips the module; a slider press is
/// consumed here (the drag itself runs in `drive_settings_sliders`).
fn press_modules_tab(
    prefs: &mut Prefs,
    fonts: &FontStore,
    mouse: (f32, f32),
    card_w: f32,
    card_h: f32,
) -> (bool, bool) {
    let layout =
        screens::settings::modules_tab_layout(fonts, card_w, card_h, prefs.settings_scroll);
    if rect_contains(&layout.list_region, mouse) {
        for rl in &layout.rows {
            if rect_contains(&rl.toggle, mouse) {
                if let Some(toggle) = prefs.module_toggles.get_mut(rl.index) {
                    if toggle.handle(mouse, rl.toggle, true) {
                        prefs.modules_changed = true;
                    }
                }
                return (true, false);
            }
            if let Some(slider) = rl.slider {
                if rect_contains(&slider, mouse) {
                    return (true, false);
                }
            }
        }
    }
    (false, false)
}

/// PvP-Utils tab — full press dispatch lives in `pvp_tab_press`. It returns
/// `true` when the press was consumed (toggle/cycle/drag start). The drag
/// itself runs through `drive_settings_sliders`.
fn press_pvp_tab(
    prefs: &mut Prefs,
    fonts: &FontStore,
    mouse: (f32, f32),
    card_w: f32,
    card_h: f32,
) -> (bool, bool) {
    let consumed = screens::settings::pvp_tab_press(prefs, fonts, card_w, card_h, mouse.0, mouse.1);
    (consumed, false)
}

/// The row-grid tabs (Graphics, Audio, Paths, Advanced). Resolution order:
/// an open dropdown's menu row commits and consumes the press; a press outside
/// an open dropdown closes it and falls through; then the tab's widgets.
fn press_general_tab(
    prefs: &mut Prefs,
    tab: SettingsTab,
    fonts: &FontStore,
    mouse: (f32, f32),
    card_w: f32,
    card_h: f32,
) -> (bool, bool) {
    let mut changed = false;

    // (1) and (2): handle any open dropdown menu first.
    if let Some(open_slot) = prefs.open_dropdown() {
        if let Some(opts) = screens::settings::dropdown_options(open_slot) {
            if let Some(head) =
                screens::settings::dropdown_head_for_slot(open_slot, fonts, card_w, card_h)
            {
                let (menu_bounds, _flip) =
                    ewo_render::widgets::menu_layout(head, opts.len(), card_h);
                let in_menu = rect_contains(&menu_bounds, mouse);
                let in_head = rect_contains(&head, mouse);
                if in_menu {
                    if let Some(state) = prefs.dropdown_state_mut(open_slot) {
                        if let Some(idx) = state.handle_menu(mouse, menu_bounds, opts.len(), true) {
                            log::info!("dropdown {:?} → {} ({})", open_slot, idx, opts[idx]);
                            changed = true;
                        }
                    }
                    return (true, changed);
                }
                if !in_head {
                    if let Some(state) = prefs.dropdown_state_mut(open_slot) {
                        state.close();
                    }
                    // Don't return — allow the press to fall through so the
                    // user can click another widget while dismissing.
                }
            }
        }
    }

    // (3) Normal widget dispatch.
    for (slot, rect) in screens::settings::widget_bounds(tab, fonts, card_w, card_h) {
        if !rect_contains(&rect, mouse) {
            continue;
        }
        match slot {
            SettingsSlot::Vsync => {
                if prefs.vsync.handle(mouse, rect, true) {
                    log::info!("vsync: {}", prefs.vsync.on);
                    changed = true;
                }
            }
            SettingsSlot::AmbientHum => {
                if prefs.ambient_hum.handle(mouse, rect, true) {
                    log::info!("ambient_hum: {}", prefs.ambient_hum.on);
                    changed = true;
                }
            }
            SettingsSlot::AutoBackup => {
                if prefs.auto_backup.handle(mouse, rect, true) {
                    log::info!("auto_backup: {}", prefs.auto_backup.on);
                    changed = true;
                }
            }
            // UI-only: there is no telemetry backend and nothing reads this
            // flag — the launcher sends no telemetry either way.
            SettingsSlot::Telemetry => {
                if prefs.telemetry.handle(mouse, rect, true) {
                    log::info!("telemetry: {}", prefs.telemetry.on);
                    changed = true;
                }
            }
            SettingsSlot::WindowMode | SettingsSlot::Theme | SettingsSlot::LogLevel => {
                // Close other dropdowns first so only one is ever open.
                close_other_dropdowns(prefs, slot);
                if let Some(state) = prefs.dropdown_state_mut(slot) {
                    if state.handle_head(mouse, rect, true) {
                        log::info!("dropdown {:?}: open={}", slot, state.open);
                    }
                }
            }
            SettingsSlot::GameDir => {
                if prefs.game_dir.handle_press(mouse, rect) {
                    log::info!("game_dir: Browse clicked");
                }
            }
            SettingsSlot::Downloads => {
                if prefs.downloads.handle_press(mouse, rect) {
                    log::info!("downloads: Browse clicked");
                }
            }
            SettingsSlot::ResetPrefs => {
                if prefs.reset_prefs.handle(mouse, rect, true) {
                    log::info!("reset_prefs: clicked → flagging for reset");
                    prefs.reset_requested = true;
                }
            }
            // Sliders driven by drive_settings_sliders — head click consumed there.
            SettingsSlot::MaxFps
            | SettingsSlot::Master
            | SettingsSlot::Music
            | SettingsSlot::Effects => {}
        }
        return (true, changed);
    }
    (false, changed)
}

pub(crate) fn close_other_dropdowns(prefs: &mut Prefs, keep: SettingsSlot) {
    if keep != SettingsSlot::WindowMode {
        prefs.window_mode.close();
    }
    if keep != SettingsSlot::Theme {
        prefs.theme.close();
    }
    if keep != SettingsSlot::LogLevel {
        prefs.log_level.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::test_support::{center, fonts, CARD_H, CARD_W, NOWHERE};

    fn uuids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("uuid-{i}")).collect()
    }
    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("profile-{i}")).collect()
    }
    fn press(
        prefs: &mut Prefs,
        tab: SettingsTab,
        fonts: &FontStore,
        mouse: (f32, f32),
        accounts: &[String],
        profiles: &[String],
        link: LinkStatusView,
    ) -> (bool, bool) {
        handle_settings_press(
            prefs,
            tab,
            Some(fonts),
            mouse,
            CARD_W,
            CARD_H,
            accounts,
            profiles,
            link,
        )
    }
    /// The widget rect for `slot` on whichever tab hosts it.
    fn slot_rect(fonts: &FontStore, slot: SettingsSlot) -> (SettingsTab, skia_safe::Rect) {
        for tab in SettingsTab::ALL {
            for (s, r) in screens::settings::widget_bounds(tab, fonts, CARD_W, CARD_H) {
                if s == slot {
                    return (tab, r);
                }
            }
        }
        panic!("no tab hosts {slot:?}");
    }

    #[test]
    fn without_fonts_nothing_is_handled() {
        let mut prefs = Prefs::default();
        let got = handle_settings_press(
            &mut prefs,
            SettingsTab::Account,
            None,
            (10.0, 10.0),
            CARD_W,
            CARD_H,
            &[],
            &[],
            LinkStatusView::Hidden,
        );
        assert_eq!(got, (false, false));
        assert!(!drive_settings_sliders(
            &mut prefs,
            SettingsTab::Graphics,
            None,
            (10.0, 10.0),
            true,
            CARD_W,
            CARD_H,
            &[],
            &[],
            LinkStatusView::Hidden,
        ));
    }

    #[test]
    fn account_tab_clicks_map_to_account_requests() {
        let Some(fonts) = fonts() else { return };
        let accounts = uuids(2);
        let link = LinkStatusView::LinkedNeedsLauncherLink;
        let layout = screens::settings::account_tab_layout(&fonts, CARD_W, 2, link);

        // The remove button sits inside its row; it wins.
        let mut prefs = Prefs::default();
        let at = center(&layout.rows[1].remove);
        let got = press(
            &mut prefs,
            SettingsTab::Account,
            &fonts,
            at,
            &accounts,
            &[],
            link,
        );
        assert_eq!(got, (true, false));
        assert!(matches!(
            prefs.account_request,
            Some(AccountRequest::Remove(ref u)) if u == "uuid-1"
        ));

        // The row itself activates the account.
        let mut prefs = Prefs::default();
        let at = center(&layout.rows[0].row);
        let at = (layout.rows[0].row.left + 4.0, at.1);
        press(
            &mut prefs,
            SettingsTab::Account,
            &fonts,
            at,
            &accounts,
            &[],
            link,
        );
        assert!(matches!(
            prefs.account_request,
            Some(AccountRequest::SetActive(ref u)) if u == "uuid-0"
        ));

        // The clickable link line opens the launcher-link modal.
        let mut prefs = Prefs::default();
        let line = layout.link_line_bounds.expect("clickable when unlinked");
        press(
            &mut prefs,
            SettingsTab::Account,
            &fonts,
            center(&line),
            &accounts,
            &[],
            link,
        );
        assert!(matches!(
            prefs.account_request,
            Some(AccountRequest::OpenLauncherLink)
        ));

        // The add button.
        let mut prefs = Prefs::default();
        let at = center(&layout.add_button);
        press(
            &mut prefs,
            SettingsTab::Account,
            &fonts,
            at,
            &accounts,
            &[],
            link,
        );
        assert!(matches!(prefs.account_request, Some(AccountRequest::Add)));

        // A click on nothing is not handled and requests nothing.
        let mut prefs = Prefs::default();
        let got = press(
            &mut prefs,
            SettingsTab::Account,
            &fonts,
            NOWHERE,
            &accounts,
            &[],
            link,
        );
        assert_eq!(got, (false, false));
        assert!(prefs.account_request.is_none());
    }

    #[test]
    fn a_hidden_link_line_is_not_clickable() {
        let Some(fonts) = fonts() else { return };
        let layout =
            screens::settings::account_tab_layout(&fonts, CARD_W, 1, LinkStatusView::Linked);
        assert!(layout.link_line_bounds.is_none());
    }

    #[test]
    fn profiles_tab_clicks_map_to_profile_requests() {
        let Some(fonts) = fonts() else { return };
        let two = names(2);
        let layout = screens::settings::profiles_tab_layout(&fonts, CARD_W, 2);
        let link = LinkStatusView::Hidden;

        // Rename icon starts editing with the current name.
        let mut prefs = Prefs::default();
        let at = center(&layout.rows[1].rename);
        press(
            &mut prefs,
            SettingsTab::Profiles,
            &fonts,
            at,
            &[],
            &two,
            link,
        );
        assert_eq!(prefs.profile_renaming, Some(1));
        assert_eq!(prefs.profile_rename_buffer, "profile-1");
        assert!(prefs.profile_request.is_none());

        // Any click while renaming commits the rename and nothing else.
        let at = center(&layout.rows[0].delete);
        let got = press(
            &mut prefs,
            SettingsTab::Profiles,
            &fonts,
            at,
            &[],
            &two,
            link,
        );
        assert_eq!(got, (true, false));
        assert!(matches!(
            prefs.profile_request,
            Some(ProfileRequest::Rename { index: 1, ref new_name }) if new_name == "profile-1"
        ));
        assert_eq!(prefs.profile_renaming, None);

        // Delete.
        let mut prefs = Prefs::default();
        let at = center(&layout.rows[1].delete);
        press(
            &mut prefs,
            SettingsTab::Profiles,
            &fonts,
            at,
            &[],
            &two,
            link,
        );
        assert!(matches!(
            prefs.profile_request,
            Some(ProfileRequest::Delete(ref n)) if n == "profile-1"
        ));

        // Switch.
        let mut prefs = Prefs::default();
        let at = (layout.rows[0].row.left + 4.0, center(&layout.rows[0].row).1);
        press(
            &mut prefs,
            SettingsTab::Profiles,
            &fonts,
            at,
            &[],
            &two,
            link,
        );
        assert!(matches!(
            prefs.profile_request,
            Some(ProfileRequest::Switch(ref n)) if n == "profile-0"
        ));

        // New and Duplicate.
        let mut prefs = Prefs::default();
        press(
            &mut prefs,
            SettingsTab::Profiles,
            &fonts,
            center(&layout.new_button),
            &[],
            &two,
            link,
        );
        assert!(matches!(prefs.profile_request, Some(ProfileRequest::New)));
        let mut prefs = Prefs::default();
        press(
            &mut prefs,
            SettingsTab::Profiles,
            &fonts,
            center(&layout.dup_button),
            &[],
            &two,
            link,
        );
        assert!(matches!(
            prefs.profile_request,
            Some(ProfileRequest::Duplicate)
        ));
    }

    #[test]
    fn the_last_profile_cannot_be_deleted() {
        let Some(fonts) = fonts() else { return };
        let one = names(1);
        let layout = screens::settings::profiles_tab_layout(&fonts, CARD_W, 1);
        let mut prefs = Prefs::default();
        let at = center(&layout.rows[0].delete);
        press(
            &mut prefs,
            SettingsTab::Profiles,
            &fonts,
            at,
            &[],
            &one,
            LinkStatusView::Hidden,
        );
        assert!(
            !matches!(prefs.profile_request, Some(ProfileRequest::Delete(_))),
            "a sole profile must not be deletable"
        );
    }

    #[test]
    fn keybinds_tab_chord_arms_capture_and_reset_restores_defaults() {
        let Some(fonts) = fonts() else { return };
        let layout = screens::settings::keybinds_tab_layout(
            &fonts,
            CARD_W,
            CARD_H,
            keybind::REGISTRY.len(),
            0.0,
        );
        let link = LinkStatusView::Hidden;

        let mut prefs = Prefs::default();
        let row = &layout.rows[0];
        let at = center(&row.chord);
        assert!(
            rect_contains(&layout.list_region, at),
            "first row is visible"
        );
        let got = press(
            &mut prefs,
            SettingsTab::Keybinds,
            &fonts,
            at,
            &[],
            &[],
            link,
        );
        assert_eq!(got, (true, false));
        assert!(matches!(
            prefs.keybind_request,
            Some(KeybindRequest::Capture(i)) if i == row.index
        ));

        // The reset button only acts when it is inside the viewport.
        if rect_contains(&layout.list_region, center(&layout.reset_button)) {
            let mut prefs = Prefs::default();
            let at = center(&layout.reset_button);
            press(
                &mut prefs,
                SettingsTab::Keybinds,
                &fonts,
                at,
                &[],
                &[],
                link,
            );
            assert!(matches!(
                prefs.keybind_request,
                Some(KeybindRequest::ResetAll)
            ));
        }

        // A press outside the list does nothing.
        let mut prefs = Prefs::default();
        let got = press(
            &mut prefs,
            SettingsTab::Keybinds,
            &fonts,
            NOWHERE,
            &[],
            &[],
            link,
        );
        assert_eq!(got, (false, false));
        assert!(prefs.keybind_request.is_none());
    }

    #[test]
    fn a_toggle_press_flips_it_and_reports_a_change() {
        let Some(fonts) = fonts() else { return };
        let (tab, rect) = slot_rect(&fonts, SettingsSlot::Vsync);
        let mut prefs = Prefs::default();
        let before = prefs.vsync.on;
        let got = press(
            &mut prefs,
            tab,
            &fonts,
            center(&rect),
            &[],
            &[],
            LinkStatusView::Hidden,
        );
        assert_eq!(got, (true, true));
        assert_eq!(prefs.vsync.on, !before);
    }

    #[test]
    fn the_reset_button_only_flags_a_reset() {
        let Some(fonts) = fonts() else { return };
        let (tab, rect) = slot_rect(&fonts, SettingsSlot::ResetPrefs);
        let mut prefs = Prefs::default();
        let got = press(
            &mut prefs,
            tab,
            &fonts,
            center(&rect),
            &[],
            &[],
            LinkStatusView::Hidden,
        );
        assert_eq!(got, (true, false));
        assert!(prefs.reset_requested);
    }

    #[test]
    fn a_press_on_a_slider_head_is_consumed_without_a_change() {
        let Some(fonts) = fonts() else { return };
        let (tab, rect) = slot_rect(&fonts, SettingsSlot::MaxFps);
        let mut prefs = Prefs::default();
        let got = press(
            &mut prefs,
            tab,
            &fonts,
            center(&rect),
            &[],
            &[],
            LinkStatusView::Hidden,
        );
        assert_eq!(got, (true, false));
    }

    /// Opening one dropdown closes any other; a press outside an open
    /// dropdown closes it and still falls through to the widget under it.
    #[test]
    fn dropdowns_are_exclusive_and_close_on_an_outside_press() {
        let Some(fonts) = fonts() else { return };
        let link = LinkStatusView::Hidden;
        let (tab, head) = slot_rect(&fonts, SettingsSlot::WindowMode);
        let mut prefs = Prefs::default();
        press(&mut prefs, tab, &fonts, center(&head), &[], &[], link);
        assert_eq!(prefs.open_dropdown(), Some(SettingsSlot::WindowMode));

        // A press that lands on nothing closes the open menu.
        press(&mut prefs, tab, &fonts, NOWHERE, &[], &[], link);
        assert_eq!(prefs.open_dropdown(), None);
    }
}
