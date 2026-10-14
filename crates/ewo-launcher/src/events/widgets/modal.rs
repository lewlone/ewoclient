//! The new-instance modal's pointer input and the `commit_new_instance` glue that
//! turns it into an instance and a download job. Moved out of `main.rs` unchanged.

use super::super::*;

/// Drive the modal's slider drag + dropdown menu hover + button hover
/// states from the current cursor + mouse-button state. Mirrors
/// `drive_settings_sliders` for the modal's three interactive controls.
pub(crate) fn drive_modal_widgets(
    modal: &mut NewInstanceModalState,
    fonts: Option<&FontStore>,
    mouse: (f32, f32),
    mouse_down: bool,
    card_w: f32,
    card_h: f32,
) {
    let Some(fonts) = fonts else {
        return;
    };
    for (slot, rect) in screens::new_instance_modal::widget_bounds(card_w, card_h, fonts) {
        match slot {
            ModalSlot::Ram => {
                modal.ram.drive(mouse, rect, mouse_down);
            }
            ModalSlot::Cancel => {
                modal.cancel_btn.handle(mouse, rect, false);
            }
            ModalSlot::Create => {
                modal.create_btn.update(mouse, rect, mouse_down, 0.0);
            }
            ModalSlot::Version | ModalSlot::Loader => {}
        }
    }
    // Update menu hover for any open dropdown. Extract just the option
    // count up-front so the `Vec<&str>` borrow ends before we ask for a
    // `&mut VdropState` on the same modal.
    if let Some(slot) = modal.open_dropdown() {
        let opt_count = modal.dropdown_options(slot).map(|v| v.len());
        if let Some(opt_count) = opt_count {
            if let Some(head) = screens::new_instance_modal::widget_bounds(card_w, card_h, fonts)
                .into_iter()
                .find_map(|(s, r)| if s == slot { Some(r) } else { None })
            {
                let (menu_bounds, _flip) =
                    ewo_render::widgets::menu_layout(head, opt_count, card_h);
                if let Some(state) = modal.dropdown_state_mut(slot) {
                    state.update_menu_hover(mouse, menu_bounds, opt_count);
                }
            }
        }
    }
    let _ = fonts;
}

/// Route a press inside the open modal. Returns `true` if the press was
/// consumed (always — the modal absorbs all clicks while open). Calls
/// `modal.close()` when the press lands on the shroud, Cancel button, or
/// a successfully-validated Create button. Create with empty name sets
/// `modal.name_error = true` and keeps the modal open so the user sees
/// the inline error.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_modal_press(
    modal: &mut NewInstanceModalState,
    instances: &mut Vec<Instance>,
    instance_prefs: &mut InstancePrefs,
    versions: &versions::VersionService,
    downloads: &mut downloads::DownloadService,
    fonts: Option<&FontStore>,
    mouse: (f32, f32),
    card_w: f32,
    card_h: f32,
    time: f32,
) -> bool {
    let Some(fonts) = fonts else {
        return true;
    };

    // (0) Name input focus hit-test. Compute the input rect from the
    // shared layout. If the click lands inside, focus it; otherwise
    // unfocus before any other widget dispatch.
    let layout = screens::new_instance_modal::compute_layout(card_w, card_h, fonts);
    if rect_contains(&layout.name_input, mouse) {
        modal.focus_name(true);
        return true;
    } else {
        modal.focus_name(false);
    }

    // (1) Open dropdown menu hit-test first. Snapshot the option strings
    // into an owned `Vec<String>` so the borrow on `modal` ends before
    // we ask for a `&mut VdropState` on the same modal below.
    if let Some(open_slot) = modal.open_dropdown() {
        let opts: Option<Vec<String>> = modal
            .dropdown_options(open_slot)
            .map(|v| v.iter().map(|s| (*s).to_string()).collect());
        if let Some(opts) = opts {
            if let Some(head) = screens::new_instance_modal::widget_bounds(card_w, card_h, fonts)
                .into_iter()
                .find_map(|(s, r)| if s == open_slot { Some(r) } else { None })
            {
                let (menu_bounds, _flip) =
                    ewo_render::widgets::menu_layout(head, opts.len(), card_h);
                let in_menu = rect_contains(&menu_bounds, mouse);
                let in_head = rect_contains(&head, mouse);
                if in_menu {
                    if let Some(state) = modal.dropdown_state_mut(open_slot) {
                        if let Some(idx) = state.handle_menu(mouse, menu_bounds, opts.len(), true) {
                            log::info!("modal dropdown {:?} → {} ({})", open_slot, idx, opts[idx]);
                        }
                    }
                    return true;
                }
                if !in_head {
                    if let Some(state) = modal.dropdown_state_mut(open_slot) {
                        state.close();
                    }
                }
            }
        }
    }

    // (2) Form widget dispatch.
    for (slot, rect) in screens::new_instance_modal::widget_bounds(card_w, card_h, fonts) {
        if !rect_contains(&rect, mouse) {
            continue;
        }
        match slot {
            ModalSlot::Version | ModalSlot::Loader => {
                close_other_modal_dropdowns(modal, slot);
                if let Some(state) = modal.dropdown_state_mut(slot) {
                    if state.handle_head(mouse, rect, true) {
                        log::info!("modal dropdown {:?}: open={}", slot, state.open);
                    }
                }
                return true;
            }
            ModalSlot::Cancel => {
                if modal.cancel_btn.handle(mouse, rect, true) {
                    log::info!("modal: Cancel clicked");
                    modal.close();
                }
                return true;
            }
            ModalSlot::Create => {
                modal.create_btn.update(mouse, rect, true, 0.0);
                if let Some(form) = try_submit_modal(modal) {
                    commit_new_instance(instances, instance_prefs, versions, downloads, form, time);
                    modal.close();
                } else {
                    // Empty name → keep modal open; `modal.name_error` is
                    // now `true`, the renderer surfaces the inline message.
                    log::info!("modal: Create blocked — name required");
                }
                return true;
            }
            // Sliders consumed by drive_modal_widgets.
            ModalSlot::Ram => return true,
        }
    }

    // (3) Anywhere else inside the card body → consume but no-op.
    let card = screens::new_instance_modal::card_rect(card_w, card_h);
    if rect_contains(&card, mouse) {
        return true;
    }

    // (4) Shroud click outside the card → dismiss.
    if screens::new_instance_modal::shroud_consumes(mouse, card_w, card_h) {
        log::info!("modal: shroud click → closing");
        modal.close();
        return true;
    }

    let _ = fonts;
    true
}

/// `modal.try_submit()` plus name validation: a name with path separators,
/// `..`, a reserved device name or control characters is rejected the same
/// way a blank one is (inline error, modal stays open).
pub(crate) fn try_submit_modal(
    modal: &mut NewInstanceModalState,
) -> Option<screens::new_instance_modal::NewInstanceForm> {
    let mut form = modal.try_submit()?;
    match persistence::validate_instance_name(&form.name) {
        Ok(name) => {
            form.name = name.to_string();
            Some(form)
        }
        Err(e) => {
            log::info!("modal: Create blocked — invalid name ({:?})", e);
            modal.name_error = true;
            None
        }
    }
}

pub(crate) fn close_other_modal_dropdowns(modal: &mut NewInstanceModalState, keep: ModalSlot) {
    if keep != ModalSlot::Version {
        modal.version.close();
    }
    if keep != ModalSlot::Loader {
        modal.loader.close();
    }
}

/// Insert a new instance at the front of the launcher's list (so it
/// shows first under the default "newest first" sort) and select it.
/// `time` is the current wall-clock seconds; used to drive the row
/// drop-in animation. Called by the Create button + Enter-key paths.
pub(crate) fn commit_new_instance(
    instances: &mut Vec<Instance>,
    prefs: &mut InstancePrefs,
    versions: &versions::VersionService,
    downloads: &mut downloads::DownloadService,
    form: screens::new_instance_modal::NewInstanceForm,
    time: f32,
) {
    let version_meta = format!("{} · {}", form.loader.to_uppercase(), form.version);
    // Map the modal's loader-string back to the typed `InstanceLoader`.
    // "Ewo (development)" → Ewo with the version-line-keyed dev manifest
    // URL (26.2 instances get 26.2.json, etc.); anything else → Vanilla.
    // Other loaders will land here once the dropdown grows back.
    let loader = if form.loader.starts_with("Ewo") {
        ewo_render::screens::instances::InstanceLoader::Ewo {
            manifest_url: loaders::base::stored_marker(&form.version),
        }
    } else if form.loader.starts_with("Native") {
        // Rewo — spawns the native client at launch. Downloads still run
        // the vanilla profile: Rewo bakes textures/data from the client jar.
        ewo_render::screens::instances::InstanceLoader::Native
    } else {
        ewo_render::screens::instances::InstanceLoader::Vanilla
    };
    // Derived before `loader` is moved into `Instance::with_loader` below.
    // The job needs the manifest URL up front so it can fetch + merge
    // before counting bytes for the progress bar.
    let loader_spec = loader_spec_for(&loader, &form.version);
    // Seed the instance's mods list from the bundled catalog so the
    // Instances UI shows real toggles immediately. Only Ewo instances get
    // mods — vanilla launches don't run any mods so the list stays empty.
    let seeded_mods = match &loader {
        ewo_render::screens::instances::InstanceLoader::Vanilla
        | ewo_render::screens::instances::InstanceLoader::Native => Vec::new(),
        ewo_render::screens::instances::InstanceLoader::Ewo { .. } => bundled::seed_instance_mods(),
    };
    let mut new_inst = Instance::new(
        form.name.clone(),
        version_meta,
        "just now".to_string(),
        seeded_mods,
    )
    .with_config(form.ram, 16)
    .with_loader(loader);
    // A fresh folder id — never the name, so a new instance can't inherit
    // a deleted same-named instance's folder.
    new_inst.id = persistence::new_instance_id(&form.name, instances);
    // Stamp the new world as "just played" so it leads the list in both
    // newest-first and recently-played sorts until the user launches
    // anything else. New instances are Pending until the download job
    // finishes.
    new_inst.last_played_at = screens::instances::current_unix_seconds();
    new_inst.status = ewo_render::screens::instances::InstanceStatus::Pending;
    log::info!(
        "instances: created \"{}\" ({} · {} · {} GB)",
        form.name,
        form.version,
        form.loader,
        form.ram
    );
    instances.insert(0, new_inst);
    // Selection is by underlying index → newly inserted is at 0; previously
    // selected indices shift +1, so update prefs.selected to track it.
    prefs.selected = prefs.selected.saturating_add(1);
    prefs.select(instances, 0);
    prefs.list_scroll = 0.0;
    prefs.created_at = Some(time);
    prefs.selected_at = Some(time);
    persistence::save_instances(instances);

    // Kick off the download job for the selected version. If the master
    // manifest hasn't loaded yet (very-first launch + offline), we just
    // log + leave the instance Pending; user can retry once the
    // manifest fetches. `loader_spec` was derived earlier (before `loader`
    // got moved into the Instance) and feeds the loader manifest URL
    // through so the job can fetch + merge it up front and include
    // loader-added libraries (EwoLoader fat jar + bundled mods) in the
    // progress bar's byte total.
    if let Some(manifest) = versions.manifest() {
        if let Some(entry) = manifest.entry(&form.version) {
            downloads.start(entry.clone(), loader_spec);
        } else {
            log::warn!(
                "instances: version {} not in master manifest — download skipped",
                form.version
            );
        }
    } else {
        log::warn!(
            "instances: master manifest not yet loaded — download deferred for {}",
            form.version
        );
    }
}
