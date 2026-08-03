//! registries.json → the `minecraft:data_component_type` ids an item-stack
//! `DataComponentPatch` is keyed by.
//!
//! The patch encodes each entry's type as a raw registry id
//! (`DataComponentType.STREAM_CODEC = ByteBufCodecs.registry(DATA_COMPONENT_TYPE)`),
//! and those numbers move between versions — so they are resolved by name here
//! and never hard-coded, exactly like the packet ids (REWO_PLAN §0.0 gotcha 5).
//!
//! Every registered component's id is kept (M41), not just the handful with
//! a transcribed codec: `rewo_net::component_wire` is keyed by name and the
//! wire by id, so the mapping has to cover whatever the registry ships. Which
//! of them Rewo can actually *walk* is that module's table, and the two are
//! deliberately separate — a component with an id and no codec is the
//! fail-closed case, and it has to be nameable to be reported.

use std::path::Path;

use crate::read_json_file;

/// Registry ids of the data components Rewo can read out of a patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataComponentIds {
    /// `minecraft:swing_animation` — `SwingAnimation.STREAM_CODEC`
    /// (type idMapper + VarInt duration). Drives M19's swing length + arm rig.
    pub swing_animation: i32,
    /// `minecraft:damage` — `ByteBufCodecs.VAR_INT`. Not read for its value:
    /// it is the component a vanilla server most often patches onto a held
    /// weapon, and being able to *skip* it is what lets the walk reach a swing
    /// override that follows it.
    pub damage: i32,
    /// `minecraft:charged_projectiles` — `ItemStackTemplate.STREAM_CODEC`
    /// under `ByteBufCodecs.list(1024)`. Read for `CrossbowItem.isCharged`,
    /// which is `!getOrDefault(CHARGED_PROJECTILES, EMPTY).isEmpty()` and is
    /// the sole gate on `ArmPose::CrossbowHold` (M23).
    ///
    /// Unlike the other two this one *must* be walked rather than merely
    /// skipped-past: a crossbow is only ever charged by a patch, so before M23
    /// every charged crossbow made its stack unresolvable and suppressed the
    /// entity's whole combat pose.
    pub charged_projectiles: i32,
    /// The six components M41 reads a *value* out of, rather than merely
    /// walking past. Each is resolved by name for the same reason the three
    /// above are: the numbers move between versions.
    pub max_damage: i32,
    pub rarity: i32,
    pub unbreakable: i32,
    pub custom_name: i32,
    pub item_name: i32,
    pub lore: i32,
    pub enchantments: i32,
    /// A book's enchantments (M42). Rendered by the same `addToTooltip`, so
    /// it is read into the same list.
    pub stored_enchantments: i32,
    /// `minecraft:enchantment_glint_override` (M43) — a per-stack yes/no that
    /// **overrides** the enchantment test in both directions.
    pub enchantment_glint_override: i32,
    /// `minecraft:dyed_color` (M47) — `DyedItemColor`, whose stream codec is a
    /// bare `ByteBufCodecs.INT`. The value is an **RGB** with no alpha, which
    /// is why `DyedItemColor.getOrDefault` runs it through `ARGB.opaque`.
    pub dyed_color: i32,
    /// `minecraft:trim` (M48) — `ArmorTrim.STREAM_CODEC`, a pair of
    /// `ByteBufCodecs.holder`s (material then pattern).
    pub trim: i32,
    /// `minecraft:map_id` (M93f) — `CartographyTableMenu`'s map slot is
    /// `mayPlace = itemStack.has(DataComponents.MAP_ID)`, and its
    /// `quickMoveStack` asks the same question first.
    ///
    /// Read for its **presence only**, never its value: the id indexes the
    /// server's saved map data, which this client has no equivalent of. And
    /// presence is the whole answer, because no item's prototype carries
    /// MAP_ID — verified against the per-item component table — so a patch is
    /// the only way a stack can have one.
    pub map_id: i32,
    /// `minecraft:dye` and `minecraft:provides_banner_patterns` (M93g) — the
    /// component halves of the loom's two conjunctions. Resolved only so a
    /// **removal** can be spotted: the prototype answers the positive case.
    pub dye: i32,
    pub provides_banner_patterns: i32,
    /// `minecraft:bundle_contents` (M61) — `ItemStackTemplate.STREAM_CODEC`
    /// under `ByteBufCodecs.list()`.
    ///
    /// Read for its contents rather than merely walked, because a bundle's
    /// tooltip grid is drawn from them and *nothing else carries them*: a
    /// bundle is only ever filled by a patch, so the item prototype answers
    /// `BundleContents.EMPTY` for every bundle a server ever sends.
    ///
    /// Note the wire form has no `selectedItem`: `STREAM_CODEC` maps through
    /// the one-argument constructor, which fixes it at `-1`. A selection is
    /// client-side state on the open screen, not a synchronised value.
    pub bundle_contents: i32,
    /// `minecraft:container` (M63) — `ItemContainerContents.STREAM_CODEC`,
    /// which is `ItemStackTemplate.STREAM_CODEC.apply(ByteBufCodecs::optional)
    /// .apply(ByteBufCodecs.list(256))`.
    ///
    /// Read for its contents for the same reason `bundle_contents` is: a
    /// shulker box is only ever filled by a patch, so the item prototype
    /// answers `ItemContainerContents.EMPTY` for every one a server sends, and
    /// `addToTooltip`'s five `item.container.item_count` lines have nowhere
    /// else to come from.
    ///
    /// Note the slot is an **`Optional`** template, unlike a bundle's plain
    /// one — the list is indexed by slot number and gaps are real.
    pub container: i32,
    /// `minecraft:use_cooldown` (M79) — `UseCooldown(float seconds,
    /// Optional<Identifier> cooldownGroup)`.
    ///
    /// Read for the **group**, which is the key `ClientboundCooldownPacket`
    /// addresses:
    ///
    /// ```java
    /// public Identifier getCooldownGroup(final ItemStack item) {
    ///    UseCooldown useCooldown = item.get(DataComponents.USE_COOLDOWN);
    ///    Identifier defaultItemGroup = BuiltInRegistries.ITEM.getKey(item.getItem());
    ///    return useCooldown == null ? defaultItemGroup
    ///                               : useCooldown.cooldownGroup().orElse(defaultItemGroup);
    /// }
    /// ```
    ///
    /// The `float` is the *server's* cooldown length and is not read by the
    /// HUD at all — the packet carries the duration in ticks. Without this,
    /// every stack falls through to its item id, which is right for every item
    /// that does not set a group and silently wrong for one that does: the
    /// sweep would be drawn on the wrong hotbar slots, with no error anywhere.
    pub use_cooldown: i32,
}

/// Every `minecraft:data_component_type` the registry ships, by name (M41).
#[derive(Clone, Debug, Default)]
pub struct DataComponentRegistry {
    by_name: std::collections::HashMap<String, i32>,
    by_id: std::collections::HashMap<i32, String>,
}

impl DataComponentRegistry {
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        let entries = json
            .get("minecraft:data_component_type")
            .and_then(|r| r.get("entries"))
            .and_then(|e| e.as_object())
            .ok_or("registries.json: no minecraft:data_component_type registry")?;
        let mut by_name = std::collections::HashMap::new();
        let mut by_id = std::collections::HashMap::new();
        for (name, entry) in entries {
            let Some(id) = entry.get("protocol_id").and_then(|i| i.as_i64()) else {
                continue;
            };
            by_name.insert(name.clone(), id as i32);
            by_id.insert(id as i32, name.clone());
        }
        if by_name.is_empty() {
            return Err("registries.json: data_component_type registry is empty".into());
        }
        Ok(Self { by_name, by_id })
    }

    pub fn ids(&self) -> &std::collections::HashMap<String, i32> {
        &self.by_name
    }

    /// The registry name for a protocol id — used to *report* which component
    /// stopped a walk, which is the difference between a diagnosable failure
    /// and a mystery.
    pub fn name_of(&self, id: i32) -> Option<&str> {
        self.by_id.get(&id).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

impl DataComponentIds {
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        let entries = json
            .get("minecraft:data_component_type")
            .and_then(|r| r.get("entries"))
            .and_then(|e| e.as_object())
            .ok_or("registries.json: no minecraft:data_component_type registry")?;
        let id = |name: &str| -> Result<i32, String> {
            entries
                .get(name)
                .and_then(|e| e.get("protocol_id"))
                .and_then(|i| i.as_i64())
                .map(|i| i as i32)
                .ok_or_else(|| format!("registries.json: no data component {name}"))
        };
        let ids = Self {
            swing_animation: id("minecraft:swing_animation")?,
            damage: id("minecraft:damage")?,
            charged_projectiles: id("minecraft:charged_projectiles")?,
            max_damage: id("minecraft:max_damage")?,
            rarity: id("minecraft:rarity")?,
            unbreakable: id("minecraft:unbreakable")?,
            custom_name: id("minecraft:custom_name")?,
            item_name: id("minecraft:item_name")?,
            lore: id("minecraft:lore")?,
            enchantments: id("minecraft:enchantments")?,
            stored_enchantments: id("minecraft:stored_enchantments")?,
            enchantment_glint_override: id("minecraft:enchantment_glint_override")?,
            dyed_color: id("minecraft:dyed_color")?,
            trim: id("minecraft:trim")?,
            map_id: id("minecraft:map_id")?,
            dye: id("minecraft:dye")?,
            provides_banner_patterns: id("minecraft:provides_banner_patterns")?,
            bundle_contents: id("minecraft:bundle_contents")?,
            container: id("minecraft:container")?,
            use_cooldown: id("minecraft:use_cooldown")?,
        };
        log::info!(
            "rewo-data: data components — swing_animation={} damage={} charged_projectiles={}",
            ids.swing_animation,
            ids.damage,
            ids.charged_projectiles
        );
        Ok(ids)
    }
}
