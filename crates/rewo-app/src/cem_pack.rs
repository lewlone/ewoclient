//! Loading an OptiFine-style CEM resource pack into per-kind model overrides.

use rewo_gpu::mobs::EntityModelKind;

/// Parse a resource-pack's CEM `.jem` models into per-kind model overrides
/// (M9). Files whose entity name doesn't map to a known model kind, or that
/// fail to parse, are skipped with a notice.
pub(crate) fn load_cem_overrides(
    path: &std::path::Path,
) -> Result<std::collections::HashMap<EntityModelKind, rewo_gpu::mobs::Model>, String> {
    let pack = rewo_data::cem::load_pack(path)?;
    let mut out = std::collections::HashMap::new();
    for file in &pack.files {
        let kind = rewo_gpu::mobs::kind_for_entity_name(&format!("minecraft:{}", file.entity));
        if kind == EntityModelKind::Capsule {
            continue; // no matching model kind (variant/collar/… files)
        }
        match rewo_gpu::cem::model_from_jem_for(&file.entity, &file.jem, &pack.jpms) {
            Ok(model) => {
                out.entry(kind).or_insert(model);
            }
            Err(e) => log::warn!("cem: {} skipped: {e}", file.entity),
        }
    }
    log::info!("cem: {} model(s) from the pack mapped to kinds", out.len());
    Ok(out)
}
