//! Input-state tests for the overlay editor: drags stranded by the overlay
//! closing, and quick-edit presses reaching the widgets.
//!
//! `Editor::new` loads and saves per-profile files under `%APPDATA%`, so every
//! test points `APPDATA` at a scratch directory first (never the player's real
//! profile).

use std::sync::OnceLock;

use super::*;

fn scratch_appdata() {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    let dir = DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("ewo-jni-editor-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch APPDATA");
        dir
    });
    std::env::set_var("APPDATA", dir);
}

fn editor() -> Editor {
    scratch_appdata();
    let mut e = Editor::new();
    e.window = (1920.0, 1080.0);
    e
}

/// Every drag-like field is at rest.
fn assert_no_drag_state(e: &Editor) {
    assert!(e.dragging.is_none(), "dragging");
    assert!(e.resizing.is_none(), "resizing");
    assert!(e.slider_drag.is_none(), "slider_drag");
    assert!(e.pvp_drag.is_none(), "pvp_drag");
    assert!(!e.glass_drag, "glass_drag");
    assert!(e.crosshair_ui.drag.is_none(), "crosshair_ui.drag");
    assert!(e.skin_drag.is_none(), "skin_drag");
    assert!(e.snap_x.is_none() && e.snap_y.is_none(), "snap guides");
}

#[test]
fn closing_the_overlay_ends_every_held_drag() {
    let mut e = editor();
    e.overlay_open = true;
    e.dragging = Some(Drag {
        id: WidgetId::Fps,
        grab_dx: 3.0,
        grab_dy: 4.0,
    });
    e.resizing = Some(ResizeDrag {
        id: WidgetId::Fps,
        start_scale: 1.0,
        grab_dist: 50.0,
    });
    e.slider_drag = Some((0, 0));
    e.pvp_drag = Some(PvpDrag::ZoneVolume(0));
    e.glass_drag = true;
    e.crosshair_ui.drag = Some(CrosshairSlider::SvSquare);
    e.skin_drag = Some(10.0);
    e.module_popover = Some(0);
    e.snap_x = Some(1.0);
    e.snap_y = Some(2.0);

    e.on_overlay_closed();

    assert_no_drag_state(&e);
    assert!(
        e.module_popover.is_none(),
        "popover must not outlive the screen"
    );
    assert!(!e.overlay_open);
}

#[test]
fn a_cursor_move_after_close_does_not_keep_dragging_the_widget() {
    let mut e = editor();
    e.overlay_open = true;
    e.bounds[WidgetId::Fps.index()] = Rect::from_xywh(100.0, 100.0, 200.0, 50.0);
    // Grab the FPS widget in the editor, as a press would.
    e.view = OverlayView::HudEditor;
    e.widget_press(150.0, 120.0);
    assert!(e.dragging.is_some(), "the press should grab the widget");
    let before = e.layout.get(WidgetId::Fps);

    // Esc: no release is ever delivered. Then the player moves the mouse to
    // the window edge (inventory, quick-edit, reopened overlay...).
    e.on_overlay_closed();
    e.on_mouse_move(0.0, 0.0);

    let after = e.layout.get(WidgetId::Fps);
    assert_eq!(
        (after.x, after.y),
        (before.x, before.y),
        "the widget followed the cursor after the overlay closed"
    );
}

#[test]
fn closing_with_nothing_held_is_a_no_op() {
    let mut e = editor();
    let before = e.layout.get(WidgetId::Fps);
    e.on_overlay_closed();
    e.on_overlay_closed();
    assert_no_drag_state(&e);
    let after = e.layout.get(WidgetId::Fps);
    assert_eq!((after.x, after.y), (before.x, before.y));
}

#[test]
fn quick_edit_press_ignores_a_leftover_popover_and_the_invisible_tab_strip() {
    let mut e = editor();
    e.quick_edit = true;
    e.overlay_open = false;
    // State the closed overlay left behind.
    e.view = OverlayView::Modules;
    e.module_popover = Some(0);
    e.bounds[WidgetId::Fps.index()] = Rect::from_xywh(100.0, 600.0, 200.0, 50.0);

    // 1. A press on a widget starts a drag instead of "closing the popover".
    e.on_mouse_button(0, true, 150.0, 620.0);
    assert!(
        matches!(&e.dragging, Some(d) if d.id == WidgetId::Fps),
        "a leftover module popover swallowed the quick-edit press"
    );
    e.on_mouse_button(0, false, 150.0, 620.0);

    // 2. A press where the (invisible) tab strip would be does not switch tabs.
    let (_, tabs) = tab_layout(e.window.0);
    let tab = tabs[0];
    e.on_mouse_button(0, true, tab.center_x(), tab.center_y());
    e.on_mouse_button(0, false, tab.center_x(), tab.center_y());
    assert!(
        e.view == OverlayView::Modules,
        "quick-edit clicked an invisible tab"
    );
}

#[test]
fn quick_edit_right_click_does_not_open_a_hidden_popover() {
    let mut e = editor();
    e.quick_edit = true;
    e.overlay_open = false;
    e.view = OverlayView::Modules;

    let (_, _, rows, _) = modules_layout(e.window.0, e.window.1);
    let row = rows
        .iter()
        .find(|r| !catalog::REGISTRY[r.catalog_index].settings.is_empty())
        .expect("some module has settings");
    e.on_mouse_button(1, true, row.row.center_x(), row.row.center_y());
    assert!(
        e.module_popover.is_none(),
        "right-click in quick-edit opened the hidden MODULES popover"
    );

    // Control: with the overlay genuinely open, the same click does open it.
    e.quick_edit = false;
    e.overlay_open = true;
    e.on_mouse_button(1, true, row.row.center_x(), row.row.center_y());
    assert!(e.module_popover.is_some());
}
