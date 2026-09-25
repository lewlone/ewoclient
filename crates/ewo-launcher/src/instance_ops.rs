//! Instance-list operations: removing an instance (with selection
//! re-anchoring), mirroring prefs into the selected instance, and mapping its
//! loader to the download job's loader layer. Moved out of `main.rs` unchanged;
//! `remove_instance` is the pure half of `delete_instance`, so the selection
//! rules can be unit-tested.

use crate::{loaders, persistence};
use ewo_render::screens::{InstancePrefs, instances::Instance};

/// Remove an instance by underlying index. Adjusts `prefs.selected` so it
/// still points at a valid instance (or the last one, if the user deleted the
/// currently-selected one). Refuses to delete the last remaining instance —
/// there must always be at least one. Returns the name, or `None` if refused.
pub(crate) fn remove_instance(
    instances: &mut Vec<Instance>,
    prefs: &mut InstancePrefs,
    underlying_idx: usize,
    time: f32,
) -> Option<String> {
    if underlying_idx >= instances.len() || instances.len() <= 1 {
        return None;
    }
    let removed_name = instances[underlying_idx].name.clone();
    instances.remove(underlying_idx);

    // Re-anchor selection. If we removed something below the cursor,
    // shift back. If we removed the cursor itself, clamp to the new last
    // index.
    if prefs.selected > underlying_idx {
        prefs.selected -= 1;
    } else if prefs.selected == underlying_idx {
        prefs.selected = prefs.selected.min(instances.len().saturating_sub(1));
    }
    prefs.sync_from_instance(instances);
    prefs.detail_scroll = 0.0;
    prefs.selected_at = Some(time); // play the detail-panel fade for the new view
    prefs.delete_hover = None;
    prefs.list_hover = None;

    Some(removed_name)
}

/// `remove_instance` + persistence: logs the outcome and saves the list.
pub(crate) fn delete_instance(
    instances: &mut Vec<Instance>,
    prefs: &mut InstancePrefs,
    underlying_idx: usize,
    time: f32,
) {
    match remove_instance(instances, prefs, underlying_idx, time) {
        Some(removed_name) => {
            log::info!("delete: removed \"{}\"", removed_name);
            persistence::save_instances(instances);
        }
        None => {
            log::info!("delete: refused (idx={} len={})", underlying_idx, instances.len());
        }
    }
}

/// Mirror the prefs slider/dropdown values into the currently-selected
/// instance. Called whenever those widgets fire a change event so the
/// per-instance config follows the user's edits.
pub(crate) fn sync_instance_config(instances: &mut Vec<Instance>, prefs: &InstancePrefs) {
    if let Some(inst) = instances.get_mut(prefs.selected) {
        inst.ram = prefs.ram.value as u32;
        inst.render_distance = prefs.render_dist.value as u32;
        inst.java_runtime = prefs.java_runtime.selected;
    }
}

/// The download job's loader layer for an instance loader (`None` = vanilla).
pub(crate) fn loader_spec_for(
    loader: &ewo_render::screens::instances::InstanceLoader,
) -> Option<loaders::LoaderSpec> {
    match loader {
        ewo_render::screens::instances::InstanceLoader::Vanilla
        | ewo_render::screens::instances::InstanceLoader::Native => None,
        ewo_render::screens::instances::InstanceLoader::Ewo { manifest_url } => {
            Some(loaders::LoaderSpec {
                id: "ewo".to_string(),
                url: manifest_url.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loaders::LoaderSpec;
    use ewo_render::screens::instances::InstanceLoader;

    fn inst(name: &str) -> Instance { Instance::new(name.into(), "26.2".into(), "never".into(), Vec::new()) }
    fn three() -> Vec<Instance> { vec![inst("a"), inst("b"), inst("c")] }

    #[test]
    fn remove_instance_refuses_out_of_range_and_sole_instance() {
        let mut list = three();
        let mut prefs = InstancePrefs::default();
        assert_eq!(remove_instance(&mut list, &mut prefs, 3, 1.0), None);
        assert_eq!(list.len(), 3);
        let mut solo = vec![inst("solo")];
        assert_eq!(remove_instance(&mut solo, &mut prefs, 0, 1.0), None);
        assert_eq!(solo.len(), 1);
    }

    /// Deleting below the selection shifts it down by one; deleting the
    /// selected last row clamps it to the new last index; deleting above it
    /// leaves it untouched.
    #[test]
    fn remove_instance_reanchors_the_selection() {
        let mut prefs = InstancePrefs::default();
        prefs.selected = 2;
        prefs.detail_scroll = 40.0;
        let mut below = three();
        assert_eq!(remove_instance(&mut below, &mut prefs, 0, 5.0), Some("a".to_string()));
        assert_eq!((prefs.selected, prefs.detail_scroll, prefs.selected_at), (1, 0.0, Some(5.0)));

        prefs.selected = 2;
        let mut last = three();
        assert_eq!(remove_instance(&mut last, &mut prefs, 2, 5.0), Some("c".to_string()));
        assert_eq!((prefs.selected, last.len()), (1, 2));

        prefs.selected = 0;
        let mut above = three();
        assert_eq!(remove_instance(&mut above, &mut prefs, 2, 5.0), Some("c".to_string()));
        assert_eq!(prefs.selected, 0);
    }

    #[test]
    fn sync_instance_config_copies_prefs_into_selected() {
        let mut list = three();
        let mut prefs = InstancePrefs::default();
        prefs.selected = 1;
        prefs.ram.value = 12.0;
        prefs.render_dist.value = 24.0;
        prefs.java_runtime.selected = 2;
        sync_instance_config(&mut list, &prefs);
        assert_eq!((list[1].ram, list[1].render_distance, list[1].java_runtime), (12, 24, 2));
        assert_eq!((list[0].ram, list[2].ram), (8, 8));
    }

    #[test]
    fn loader_spec_for_maps_loader_to_download_spec() {
        assert_eq!(loader_spec_for(&InstanceLoader::Vanilla), None);
        assert_eq!(loader_spec_for(&InstanceLoader::Native), None);
        let url = "file:///loader/26.2.json".to_string();
        let spec = loader_spec_for(&InstanceLoader::Ewo { manifest_url: url.clone() });
        assert_eq!(spec, Some(LoaderSpec { id: "ewo".to_string(), url }));
    }
}
