//! registries.json → `minecraft:number_format_type` ids (M65).
//!
//! `set_objective` and `set_score` both carry an optional `NumberFormat`,
//! whose stream codec is
//! `ByteBufCodecs.registry(Registries.NUMBER_FORMAT_TYPE).dispatch(...)` — a
//! **raw** registry id (an `idMapper`, so no `id + 1` / inline convention),
//! then a body whose *shape depends on which type the id names*:
//!
//! | type      | body                                   |
//! |-----------|----------------------------------------|
//! | `blank`   | `StreamCodec.unit` — **zero bytes**    |
//! | `styled`  | one network-NBT tag (a `Style`)        |
//! | `fixed`   | one network-NBT tag (a `Component`)    |
//!
//! So an id we cannot name is not skippable — it is the `DataComponentPatch`
//! problem from M41 a second time, and the honest answer is a decode error
//! rather than a guess at a length.
//!
//! The ids are resolved **by name** from the report rather than hard-coded,
//! for the reason REWO_PLAN §11 gives for packet ids: `number_format_type` is
//! a built-in registry, so a version bump that inserts a fourth type ahead of
//! the others renumbers all three silently. A blank score would then read as
//! a styled one, consume a tag that is not there, and take the rest of the
//! packet with it. Failing loud at load is the alternative.

use std::path::Path;

use crate::read_json_file;

/// The three `NumberFormatType` protocol ids.
///
/// `Copy` because it is three integers and every decode site wants it by
/// value; `PlaySession` keeps its own copy the way it does `ParticleTypes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumberFormatTypeIds {
    pub blank: i32,
    pub styled: i32,
    pub fixed: i32,
}

impl NumberFormatTypeIds {
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        let entries = json
            .get("minecraft:number_format_type")
            .and_then(|r| r.get("entries"))
            .and_then(|e| e.as_object())
            .ok_or("registries.json: no minecraft:number_format_type registry")?;
        let id_of = |name: &str| -> Result<i32, String> {
            entries
                .get(name)
                .and_then(|e| e.get("protocol_id"))
                .and_then(|i| i.as_i64())
                .map(|i| i as i32)
                .ok_or_else(|| format!("registries.json: no number format type {name}"))
        };
        let ids = Self {
            blank: id_of("minecraft:blank")?,
            styled: id_of("minecraft:styled")?,
            fixed: id_of("minecraft:fixed")?,
        };
        log::info!("rewo-data: number format types {ids:?}");
        Ok(ids)
    }
}
