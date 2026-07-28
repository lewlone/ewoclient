//! Per-item stack size, equippable slot, durability and rarity — GENERATED,
//! do not edit.
//!
//! Regenerate with `python tools/gen_item_props.py` after a version bump.
//!
//! Source: the datagen per-item component report for 26.2. Of 1537
//! items, 295 differ from `Item.DEFAULT_MAX_STACK_SIZE` = 64
//! (246 at 1, 49 at 16), 84 carry `minecraft:equippable`
//! (44 body, 8 chest, 7 feet, 16 head, 7 legs, 1 offhand, 1 saddle), 84 carry `minecraft:max_damage`, 43
//! distinct equipment assets are named, and 115 differ from
//! `Rarity.COMMON` (78 uncommon, 18 rare, 19 epic). Items with none
//! of these are not listed.
//!
//! The first two feed the container click arithmetic. A wrong cap or a
//! wrongly-allowed armour placement predicts a wrong slot, the server's
//! `HashedStack.matches` fails, and the container resynchronises — which looks
//! like a click that bounced. The rarity is purely visible, and is here for
//! the same reason: it lives in the prototype, so a client that reads only the
//! component patch paints all 115 of these white.

/// `EquipmentSlot`, restricted to the members an item can name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EquipSlot {
    Head,
    Chest,
    Legs,
    Feet,
    Mainhand,
    Offhand,
    Body,
    Saddle,
}

/// `Item.DEFAULT_MAX_STACK_SIZE`, read from the decompile rather than assumed,
/// because the table below is a delta from it.
pub const DEFAULT_MAX_STACK: i32 = 64;

/// `Item.ABSOLUTE_MAX_STACK_SIZE` — the ceiling `stacksTo` asserts against.
pub const ABSOLUTE_MAX_STACK: i32 = 99;

/// `Rarity.COMMON`'s id — the value
/// `ItemStack.getRarity`'s `getOrDefault(DataComponents.RARITY, ...)` falls
/// back to, and the one the table below is a delta from.
pub const DEFAULT_RARITY: i32 = 0;

/// Every item that differs from the defaults in at least one column, sorted by
/// name so a binary search finds it. A `None` in any column means the default.
pub const ITEM_PROPS: &[(
    &str,
    Option<i32>,
    Option<EquipSlot>,
    Option<i32>,
    Option<&str>,
    Option<i32>,
)] = &[
    ("minecraft:acacia_boat", Some(1), None, None, None, None),
    ("minecraft:acacia_chest_boat", Some(1), None, None, None, None),
    ("minecraft:acacia_hanging_sign", Some(16), None, None, None, None),
    ("minecraft:acacia_sign", Some(16), None, None, None, None),
    ("minecraft:angler_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:archer_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:armor_stand", Some(16), None, None, None, None),
    ("minecraft:arms_up_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:axolotl_bucket", Some(1), None, None, None, None),
    ("minecraft:bamboo_chest_raft", Some(1), None, None, None, None),
    ("minecraft:bamboo_hanging_sign", Some(16), None, None, None, None),
    ("minecraft:bamboo_raft", Some(1), None, None, None, None),
    ("minecraft:bamboo_sign", Some(16), None, None, None, None),
    ("minecraft:barrier", None, None, None, None, Some(3)),
    ("minecraft:beacon", None, None, None, None, Some(2)),
    ("minecraft:beetroot_soup", Some(1), None, None, None, None),
    ("minecraft:birch_boat", Some(1), None, None, None, None),
    ("minecraft:birch_chest_boat", Some(1), None, None, None, None),
    ("minecraft:birch_hanging_sign", Some(16), None, None, None, None),
    ("minecraft:birch_sign", Some(16), None, None, None, None),
    ("minecraft:black_banner", Some(16), None, None, None, None),
    ("minecraft:black_bed", Some(1), None, None, None, None),
    ("minecraft:black_bundle", Some(1), None, None, None, None),
    ("minecraft:black_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:black_carpet"), None),
    ("minecraft:black_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:black_harness"), None),
    ("minecraft:black_shulker_box", Some(1), None, None, None, None),
    ("minecraft:blade_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:blue_banner", Some(16), None, None, None, None),
    ("minecraft:blue_bed", Some(1), None, None, None, None),
    ("minecraft:blue_bundle", Some(1), None, None, None, None),
    ("minecraft:blue_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:blue_carpet"), None),
    ("minecraft:blue_egg", Some(16), None, None, None, None),
    ("minecraft:blue_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:blue_harness"), None),
    ("minecraft:blue_shulker_box", Some(1), None, None, None, None),
    ("minecraft:bolt_armor_trim_smithing_template", None, None, None, None, Some(1)),
    ("minecraft:bordure_indented_banner_pattern", Some(1), None, None, None, None),
    ("minecraft:bow", Some(1), None, Some(384), None, None),
    ("minecraft:brewer_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:brown_banner", Some(16), None, None, None, None),
    ("minecraft:brown_bed", Some(1), None, None, None, None),
    ("minecraft:brown_bundle", Some(1), None, None, None, None),
    ("minecraft:brown_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:brown_carpet"), None),
    ("minecraft:brown_egg", Some(16), None, None, None, None),
    ("minecraft:brown_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:brown_harness"), None),
    ("minecraft:brown_shulker_box", Some(1), None, None, None, None),
    ("minecraft:brush", Some(1), None, Some(64), None, None),
    ("minecraft:bucket", Some(16), None, None, None, None),
    ("minecraft:bundle", Some(1), None, None, None, None),
    ("minecraft:burn_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:cake", Some(1), None, None, None, None),
    ("minecraft:carrot_on_a_stick", Some(1), None, Some(25), None, None),
    ("minecraft:carved_pumpkin", None, Some(EquipSlot::Head), None, None, None),
    ("minecraft:chain_command_block", None, None, None, None, Some(3)),
    ("minecraft:chainmail_boots", Some(1), Some(EquipSlot::Feet), Some(195), Some("minecraft:chainmail"), Some(1)),
    ("minecraft:chainmail_chestplate", Some(1), Some(EquipSlot::Chest), Some(240), Some("minecraft:chainmail"), Some(1)),
    ("minecraft:chainmail_helmet", Some(1), Some(EquipSlot::Head), Some(165), Some("minecraft:chainmail"), Some(1)),
    ("minecraft:chainmail_leggings", Some(1), Some(EquipSlot::Legs), Some(225), Some("minecraft:chainmail"), Some(1)),
    ("minecraft:cherry_boat", Some(1), None, None, None, None),
    ("minecraft:cherry_chest_boat", Some(1), None, None, None, None),
    ("minecraft:cherry_hanging_sign", Some(16), None, None, None, None),
    ("minecraft:cherry_sign", Some(16), None, None, None, None),
    ("minecraft:chest_minecart", Some(1), None, None, None, None),
    ("minecraft:coast_armor_trim_smithing_template", None, None, None, None, Some(1)),
    ("minecraft:cod_bucket", Some(1), None, None, None, None),
    ("minecraft:command_block", None, None, None, None, Some(3)),
    ("minecraft:command_block_minecart", Some(1), None, None, None, Some(3)),
    ("minecraft:conduit", None, None, None, None, Some(1)),
    ("minecraft:copper_axe", Some(1), None, Some(190), None, None),
    ("minecraft:copper_boots", Some(1), Some(EquipSlot::Feet), Some(143), Some("minecraft:copper"), None),
    ("minecraft:copper_chestplate", Some(1), Some(EquipSlot::Chest), Some(176), Some("minecraft:copper"), None),
    ("minecraft:copper_helmet", Some(1), Some(EquipSlot::Head), Some(121), Some("minecraft:copper"), None),
    ("minecraft:copper_hoe", Some(1), None, Some(190), None, None),
    ("minecraft:copper_horse_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:copper"), None),
    ("minecraft:copper_leggings", Some(1), Some(EquipSlot::Legs), Some(165), Some("minecraft:copper"), None),
    ("minecraft:copper_nautilus_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:copper"), None),
    ("minecraft:copper_pickaxe", Some(1), None, Some(190), None, None),
    ("minecraft:copper_shovel", Some(1), None, Some(190), None, None),
    ("minecraft:copper_spear", Some(1), None, Some(190), None, None),
    ("minecraft:copper_sword", Some(1), None, Some(190), None, None),
    ("minecraft:creeper_banner_pattern", Some(1), None, None, None, Some(1)),
    ("minecraft:creeper_head", None, Some(EquipSlot::Head), None, None, Some(1)),
    ("minecraft:crimson_hanging_sign", Some(16), None, None, None, None),
    ("minecraft:crimson_sign", Some(16), None, None, None, None),
    ("minecraft:crossbow", Some(1), None, Some(465), None, None),
    ("minecraft:cyan_banner", Some(16), None, None, None, None),
    ("minecraft:cyan_bed", Some(1), None, None, None, None),
    ("minecraft:cyan_bundle", Some(1), None, None, None, None),
    ("minecraft:cyan_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:cyan_carpet"), None),
    ("minecraft:cyan_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:cyan_harness"), None),
    ("minecraft:cyan_shulker_box", Some(1), None, None, None, None),
    ("minecraft:danger_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:dark_oak_boat", Some(1), None, None, None, None),
    ("minecraft:dark_oak_chest_boat", Some(1), None, None, None, None),
    ("minecraft:dark_oak_hanging_sign", Some(16), None, None, None, None),
    ("minecraft:dark_oak_sign", Some(16), None, None, None, None),
    ("minecraft:debug_stick", Some(1), None, None, None, Some(3)),
    ("minecraft:diamond_axe", Some(1), None, Some(1561), None, None),
    ("minecraft:diamond_boots", Some(1), Some(EquipSlot::Feet), Some(429), Some("minecraft:diamond"), None),
    ("minecraft:diamond_chestplate", Some(1), Some(EquipSlot::Chest), Some(528), Some("minecraft:diamond"), None),
    ("minecraft:diamond_helmet", Some(1), Some(EquipSlot::Head), Some(363), Some("minecraft:diamond"), None),
    ("minecraft:diamond_hoe", Some(1), None, Some(1561), None, None),
    ("minecraft:diamond_horse_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:diamond"), None),
    ("minecraft:diamond_leggings", Some(1), Some(EquipSlot::Legs), Some(495), Some("minecraft:diamond"), None),
    ("minecraft:diamond_nautilus_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:diamond"), None),
    ("minecraft:diamond_pickaxe", Some(1), None, Some(1561), None, None),
    ("minecraft:diamond_shovel", Some(1), None, Some(1561), None, None),
    ("minecraft:diamond_spear", Some(1), None, Some(1561), None, None),
    ("minecraft:diamond_sword", Some(1), None, Some(1561), None, None),
    ("minecraft:disc_fragment_5", None, None, None, None, Some(1)),
    ("minecraft:dragon_breath", None, None, None, None, Some(1)),
    ("minecraft:dragon_egg", None, None, None, None, Some(3)),
    ("minecraft:dragon_head", None, Some(EquipSlot::Head), None, None, Some(3)),
    ("minecraft:dune_armor_trim_smithing_template", None, None, None, None, Some(1)),
    ("minecraft:echo_shard", None, None, None, None, Some(1)),
    ("minecraft:egg", Some(16), None, None, None, None),
    ("minecraft:elytra", Some(1), Some(EquipSlot::Chest), Some(432), Some("minecraft:elytra"), Some(3)),
    ("minecraft:enchanted_book", Some(1), None, None, None, Some(2)),
    ("minecraft:enchanted_golden_apple", None, None, None, None, Some(2)),
    ("minecraft:ender_pearl", Some(16), None, None, None, None),
    ("minecraft:experience_bottle", None, None, None, None, Some(1)),
    ("minecraft:explorer_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:eye_armor_trim_smithing_template", None, None, None, None, Some(2)),
    ("minecraft:field_masoned_banner_pattern", Some(1), None, None, None, None),
    ("minecraft:fishing_rod", Some(1), None, Some(64), None, None),
    ("minecraft:flint_and_steel", Some(1), None, Some(64), None, None),
    ("minecraft:flow_armor_trim_smithing_template", None, None, None, None, Some(1)),
    ("minecraft:flow_banner_pattern", Some(1), None, None, None, Some(2)),
    ("minecraft:flow_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:flower_banner_pattern", Some(1), None, None, None, None),
    ("minecraft:friend_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:furnace_minecart", Some(1), None, None, None, None),
    ("minecraft:globe_banner_pattern", Some(1), None, None, None, None),
    ("minecraft:goat_horn", Some(1), None, None, None, Some(1)),
    ("minecraft:golden_axe", Some(1), None, Some(32), None, None),
    ("minecraft:golden_boots", Some(1), Some(EquipSlot::Feet), Some(91), Some("minecraft:gold"), None),
    ("minecraft:golden_chestplate", Some(1), Some(EquipSlot::Chest), Some(112), Some("minecraft:gold"), None),
    ("minecraft:golden_helmet", Some(1), Some(EquipSlot::Head), Some(77), Some("minecraft:gold"), None),
    ("minecraft:golden_hoe", Some(1), None, Some(32), None, None),
    ("minecraft:golden_horse_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:gold"), None),
    ("minecraft:golden_leggings", Some(1), Some(EquipSlot::Legs), Some(105), Some("minecraft:gold"), None),
    ("minecraft:golden_nautilus_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:gold"), None),
    ("minecraft:golden_pickaxe", Some(1), None, Some(32), None, None),
    ("minecraft:golden_shovel", Some(1), None, Some(32), None, None),
    ("minecraft:golden_spear", Some(1), None, Some(32), None, None),
    ("minecraft:golden_sword", Some(1), None, Some(32), None, None),
    ("minecraft:gray_banner", Some(16), None, None, None, None),
    ("minecraft:gray_bed", Some(1), None, None, None, None),
    ("minecraft:gray_bundle", Some(1), None, None, None, None),
    ("minecraft:gray_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:gray_carpet"), None),
    ("minecraft:gray_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:gray_harness"), None),
    ("minecraft:gray_shulker_box", Some(1), None, None, None, None),
    ("minecraft:green_banner", Some(16), None, None, None, None),
    ("minecraft:green_bed", Some(1), None, None, None, None),
    ("minecraft:green_bundle", Some(1), None, None, None, None),
    ("minecraft:green_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:green_carpet"), None),
    ("minecraft:green_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:green_harness"), None),
    ("minecraft:green_shulker_box", Some(1), None, None, None, None),
    ("minecraft:guster_banner_pattern", Some(1), None, None, None, Some(2)),
    ("minecraft:guster_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:heart_of_the_sea", None, None, None, None, Some(1)),
    ("minecraft:heart_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:heartbreak_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:heavy_core", None, None, None, None, Some(3)),
    ("minecraft:honey_bottle", Some(16), None, None, None, None),
    ("minecraft:hopper_minecart", Some(1), None, None, None, None),
    ("minecraft:host_armor_trim_smithing_template", None, None, None, None, Some(1)),
    ("minecraft:howl_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:iron_axe", Some(1), None, Some(250), None, None),
    ("minecraft:iron_boots", Some(1), Some(EquipSlot::Feet), Some(195), Some("minecraft:iron"), None),
    ("minecraft:iron_chestplate", Some(1), Some(EquipSlot::Chest), Some(240), Some("minecraft:iron"), None),
    ("minecraft:iron_helmet", Some(1), Some(EquipSlot::Head), Some(165), Some("minecraft:iron"), None),
    ("minecraft:iron_hoe", Some(1), None, Some(250), None, None),
    ("minecraft:iron_horse_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:iron"), None),
    ("minecraft:iron_leggings", Some(1), Some(EquipSlot::Legs), Some(225), Some("minecraft:iron"), None),
    ("minecraft:iron_nautilus_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:iron"), None),
    ("minecraft:iron_pickaxe", Some(1), None, Some(250), None, None),
    ("minecraft:iron_shovel", Some(1), None, Some(250), None, None),
    ("minecraft:iron_spear", Some(1), None, Some(250), None, None),
    ("minecraft:iron_sword", Some(1), None, Some(250), None, None),
    ("minecraft:jigsaw", None, None, None, None, Some(3)),
    ("minecraft:jungle_boat", Some(1), None, None, None, None),
    ("minecraft:jungle_chest_boat", Some(1), None, None, None, None),
    ("minecraft:jungle_hanging_sign", Some(16), None, None, None, None),
    ("minecraft:jungle_sign", Some(16), None, None, None, None),
    ("minecraft:knowledge_book", Some(1), None, None, None, Some(3)),
    ("minecraft:lava_bucket", Some(1), None, None, None, None),
    ("minecraft:leather_boots", Some(1), Some(EquipSlot::Feet), Some(65), Some("minecraft:leather"), None),
    ("minecraft:leather_chestplate", Some(1), Some(EquipSlot::Chest), Some(80), Some("minecraft:leather"), None),
    ("minecraft:leather_helmet", Some(1), Some(EquipSlot::Head), Some(55), Some("minecraft:leather"), None),
    ("minecraft:leather_horse_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:leather"), None),
    ("minecraft:leather_leggings", Some(1), Some(EquipSlot::Legs), Some(75), Some("minecraft:leather"), None),
    ("minecraft:light", None, None, None, None, Some(3)),
    ("minecraft:light_blue_banner", Some(16), None, None, None, None),
    ("minecraft:light_blue_bed", Some(1), None, None, None, None),
    ("minecraft:light_blue_bundle", Some(1), None, None, None, None),
    ("minecraft:light_blue_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:light_blue_carpet"), None),
    ("minecraft:light_blue_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:light_blue_harness"), None),
    ("minecraft:light_blue_shulker_box", Some(1), None, None, None, None),
    ("minecraft:light_gray_banner", Some(16), None, None, None, None),
    ("minecraft:light_gray_bed", Some(1), None, None, None, None),
    ("minecraft:light_gray_bundle", Some(1), None, None, None, None),
    ("minecraft:light_gray_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:light_gray_carpet"), None),
    ("minecraft:light_gray_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:light_gray_harness"), None),
    ("minecraft:light_gray_shulker_box", Some(1), None, None, None, None),
    ("minecraft:lime_banner", Some(16), None, None, None, None),
    ("minecraft:lime_bed", Some(1), None, None, None, None),
    ("minecraft:lime_bundle", Some(1), None, None, None, None),
    ("minecraft:lime_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:lime_carpet"), None),
    ("minecraft:lime_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:lime_harness"), None),
    ("minecraft:lime_shulker_box", Some(1), None, None, None, None),
    ("minecraft:lingering_potion", Some(1), None, None, None, None),
    ("minecraft:mace", Some(1), None, Some(500), None, Some(3)),
    ("minecraft:magenta_banner", Some(16), None, None, None, None),
    ("minecraft:magenta_bed", Some(1), None, None, None, None),
    ("minecraft:magenta_bundle", Some(1), None, None, None, None),
    ("minecraft:magenta_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:magenta_carpet"), None),
    ("minecraft:magenta_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:magenta_harness"), None),
    ("minecraft:magenta_shulker_box", Some(1), None, None, None, None),
    ("minecraft:mangrove_boat", Some(1), None, None, None, None),
    ("minecraft:mangrove_chest_boat", Some(1), None, None, None, None),
    ("minecraft:mangrove_hanging_sign", Some(16), None, None, None, None),
    ("minecraft:mangrove_sign", Some(16), None, None, None, None),
    ("minecraft:milk_bucket", Some(1), None, None, None, None),
    ("minecraft:minecart", Some(1), None, None, None, None),
    ("minecraft:miner_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:mojang_banner_pattern", Some(1), None, None, None, Some(2)),
    ("minecraft:mourner_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:mushroom_stew", Some(1), None, None, None, None),
    ("minecraft:music_disc_11", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_13", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_5", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_blocks", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_bounce", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_cat", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_chirp", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_creator", Some(1), None, None, None, Some(2)),
    ("minecraft:music_disc_creator_music_box", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_far", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_lava_chicken", Some(1), None, None, None, Some(2)),
    ("minecraft:music_disc_mall", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_mellohi", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_otherside", Some(1), None, None, None, Some(2)),
    ("minecraft:music_disc_pigstep", Some(1), None, None, None, Some(2)),
    ("minecraft:music_disc_precipice", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_relic", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_stal", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_strad", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_tears", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_wait", Some(1), None, None, None, Some(1)),
    ("minecraft:music_disc_ward", Some(1), None, None, None, Some(1)),
    ("minecraft:nautilus_shell", None, None, None, None, Some(1)),
    ("minecraft:nether_star", None, None, None, None, Some(2)),
    ("minecraft:netherite_axe", Some(1), None, Some(2031), None, None),
    ("minecraft:netherite_boots", Some(1), Some(EquipSlot::Feet), Some(481), Some("minecraft:netherite"), None),
    ("minecraft:netherite_chestplate", Some(1), Some(EquipSlot::Chest), Some(592), Some("minecraft:netherite"), None),
    ("minecraft:netherite_helmet", Some(1), Some(EquipSlot::Head), Some(407), Some("minecraft:netherite"), None),
    ("minecraft:netherite_hoe", Some(1), None, Some(2031), None, None),
    ("minecraft:netherite_horse_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:netherite"), None),
    ("minecraft:netherite_leggings", Some(1), Some(EquipSlot::Legs), Some(555), Some("minecraft:netherite"), None),
    ("minecraft:netherite_nautilus_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:netherite"), None),
    ("minecraft:netherite_pickaxe", Some(1), None, Some(2031), None, None),
    ("minecraft:netherite_shovel", Some(1), None, Some(2031), None, None),
    ("minecraft:netherite_spear", Some(1), None, Some(2031), None, None),
    ("minecraft:netherite_sword", Some(1), None, Some(2031), None, None),
    ("minecraft:netherite_upgrade_smithing_template", None, None, None, None, Some(1)),
    ("minecraft:oak_boat", Some(1), None, None, None, None),
    ("minecraft:oak_chest_boat", Some(1), None, None, None, None),
    ("minecraft:oak_hanging_sign", Some(16), None, None, None, None),
    ("minecraft:oak_sign", Some(16), None, None, None, None),
    ("minecraft:ominous_bottle", None, None, None, None, Some(1)),
    ("minecraft:orange_banner", Some(16), None, None, None, None),
    ("minecraft:orange_bed", Some(1), None, None, None, None),
    ("minecraft:orange_bundle", Some(1), None, None, None, None),
    ("minecraft:orange_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:orange_carpet"), None),
    ("minecraft:orange_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:orange_harness"), None),
    ("minecraft:orange_shulker_box", Some(1), None, None, None, None),
    ("minecraft:pale_oak_boat", Some(1), None, None, None, None),
    ("minecraft:pale_oak_chest_boat", Some(1), None, None, None, None),
    ("minecraft:pale_oak_hanging_sign", Some(16), None, None, None, None),
    ("minecraft:pale_oak_sign", Some(16), None, None, None, None),
    ("minecraft:piglin_banner_pattern", Some(1), None, None, None, Some(1)),
    ("minecraft:piglin_head", None, Some(EquipSlot::Head), None, None, Some(1)),
    ("minecraft:pink_banner", Some(16), None, None, None, None),
    ("minecraft:pink_bed", Some(1), None, None, None, None),
    ("minecraft:pink_bundle", Some(1), None, None, None, None),
    ("minecraft:pink_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:pink_carpet"), None),
    ("minecraft:pink_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:pink_harness"), None),
    ("minecraft:pink_shulker_box", Some(1), None, None, None, None),
    ("minecraft:player_head", None, Some(EquipSlot::Head), None, None, Some(1)),
    ("minecraft:plenty_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:potion", Some(1), None, None, None, None),
    ("minecraft:powder_snow_bucket", Some(1), None, None, None, None),
    ("minecraft:prize_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:pufferfish_bucket", Some(1), None, None, None, None),
    ("minecraft:purple_banner", Some(16), None, None, None, None),
    ("minecraft:purple_bed", Some(1), None, None, None, None),
    ("minecraft:purple_bundle", Some(1), None, None, None, None),
    ("minecraft:purple_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:purple_carpet"), None),
    ("minecraft:purple_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:purple_harness"), None),
    ("minecraft:purple_shulker_box", Some(1), None, None, None, None),
    ("minecraft:rabbit_stew", Some(1), None, None, None, None),
    ("minecraft:raiser_armor_trim_smithing_template", None, None, None, None, Some(1)),
    ("minecraft:recovery_compass", None, None, None, None, Some(1)),
    ("minecraft:red_banner", Some(16), None, None, None, None),
    ("minecraft:red_bed", Some(1), None, None, None, None),
    ("minecraft:red_bundle", Some(1), None, None, None, None),
    ("minecraft:red_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:red_carpet"), None),
    ("minecraft:red_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:red_harness"), None),
    ("minecraft:red_shulker_box", Some(1), None, None, None, None),
    ("minecraft:repeating_command_block", None, None, None, None, Some(3)),
    ("minecraft:rib_armor_trim_smithing_template", None, None, None, None, Some(1)),
    ("minecraft:saddle", Some(1), Some(EquipSlot::Saddle), None, Some("minecraft:saddle"), None),
    ("minecraft:salmon_bucket", Some(1), None, None, None, None),
    ("minecraft:scrape_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:sentry_armor_trim_smithing_template", None, None, None, None, Some(1)),
    ("minecraft:shaper_armor_trim_smithing_template", None, None, None, None, Some(1)),
    ("minecraft:sheaf_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:shears", Some(1), None, Some(238), None, None),
    ("minecraft:shelter_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:shield", Some(1), Some(EquipSlot::Offhand), Some(336), None, None),
    ("minecraft:shulker_box", Some(1), None, None, None, None),
    ("minecraft:silence_armor_trim_smithing_template", None, None, None, None, Some(3)),
    ("minecraft:skeleton_skull", None, Some(EquipSlot::Head), None, None, Some(1)),
    ("minecraft:skull_banner_pattern", Some(1), None, None, None, Some(2)),
    ("minecraft:skull_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:sniffer_egg", None, None, None, None, Some(1)),
    ("minecraft:snort_pottery_sherd", None, None, None, None, Some(1)),
    ("minecraft:snout_armor_trim_smithing_template", None, None, None, None, Some(1)),
    ("minecraft:snowball", Some(16), None, None, None, None),
    ("minecraft:spire_armor_trim_smithing_template", None, None, None, None, Some(2)),
    ("minecraft:splash_potion", Some(1), None, None, None, None),
    ("minecraft:spruce_boat", Some(1), None, None, None, None),
    ("minecraft:spruce_chest_boat", Some(1), None, None, None, None),
    ("minecraft:spruce_hanging_sign", Some(16), None, None, None, None),
    ("minecraft:spruce_sign", Some(16), None, None, None, None),
    ("minecraft:spyglass", Some(1), None, None, None, None),
    ("minecraft:stone_axe", Some(1), None, Some(131), None, None),
    ("minecraft:stone_hoe", Some(1), None, Some(131), None, None),
    ("minecraft:stone_pickaxe", Some(1), None, Some(131), None, None),
    ("minecraft:stone_shovel", Some(1), None, Some(131), None, None),
    ("minecraft:stone_spear", Some(1), None, Some(131), None, None),
    ("minecraft:stone_sword", Some(1), None, Some(131), None, None),
    ("minecraft:structure_block", None, None, None, None, Some(3)),
    ("minecraft:structure_void", None, None, None, None, Some(3)),
    ("minecraft:sulfur_cube_bucket", Some(1), None, None, None, None),
    ("minecraft:suspicious_stew", Some(1), None, None, None, None),
    ("minecraft:tadpole_bucket", Some(1), None, None, None, None),
    ("minecraft:test_block", None, None, None, None, Some(3)),
    ("minecraft:test_instance_block", None, None, None, None, Some(3)),
    ("minecraft:tide_armor_trim_smithing_template", None, None, None, None, Some(1)),
    ("minecraft:tnt_minecart", Some(1), None, None, None, None),
    ("minecraft:totem_of_undying", Some(1), None, None, None, Some(1)),
    ("minecraft:trident", Some(1), None, Some(250), None, Some(2)),
    ("minecraft:tropical_fish_bucket", Some(1), None, None, None, None),
    ("minecraft:turtle_helmet", Some(1), Some(EquipSlot::Head), Some(275), Some("minecraft:turtle_scute"), None),
    ("minecraft:vex_armor_trim_smithing_template", None, None, None, None, Some(2)),
    ("minecraft:ward_armor_trim_smithing_template", None, None, None, None, Some(2)),
    ("minecraft:warped_fungus_on_a_stick", Some(1), None, Some(100), None, None),
    ("minecraft:warped_hanging_sign", Some(16), None, None, None, None),
    ("minecraft:warped_sign", Some(16), None, None, None, None),
    ("minecraft:water_bucket", Some(1), None, None, None, None),
    ("minecraft:wayfinder_armor_trim_smithing_template", None, None, None, None, Some(1)),
    ("minecraft:white_banner", Some(16), None, None, None, None),
    ("minecraft:white_bed", Some(1), None, None, None, None),
    ("minecraft:white_bundle", Some(1), None, None, None, None),
    ("minecraft:white_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:white_carpet"), None),
    ("minecraft:white_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:white_harness"), None),
    ("minecraft:white_shulker_box", Some(1), None, None, None, None),
    ("minecraft:wild_armor_trim_smithing_template", None, None, None, None, Some(1)),
    ("minecraft:wither_skeleton_skull", None, Some(EquipSlot::Head), None, None, Some(2)),
    ("minecraft:wolf_armor", Some(1), Some(EquipSlot::Body), Some(64), Some("minecraft:armadillo_scute"), None),
    ("minecraft:wooden_axe", Some(1), None, Some(59), None, None),
    ("minecraft:wooden_hoe", Some(1), None, Some(59), None, None),
    ("minecraft:wooden_pickaxe", Some(1), None, Some(59), None, None),
    ("minecraft:wooden_shovel", Some(1), None, Some(59), None, None),
    ("minecraft:wooden_spear", Some(1), None, Some(59), None, None),
    ("minecraft:wooden_sword", Some(1), None, Some(59), None, None),
    ("minecraft:writable_book", Some(1), None, None, None, None),
    ("minecraft:written_book", Some(16), None, None, None, None),
    ("minecraft:yellow_banner", Some(16), None, None, None, None),
    ("minecraft:yellow_bed", Some(1), None, None, None, None),
    ("minecraft:yellow_bundle", Some(1), None, None, None, None),
    ("minecraft:yellow_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:yellow_carpet"), None),
    ("minecraft:yellow_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:yellow_harness"), None),
    ("minecraft:yellow_shulker_box", Some(1), None, None, None, None),
    ("minecraft:zombie_head", None, Some(EquipSlot::Head), None, None, Some(1)),
];

type Props = (
    Option<i32>,
    Option<EquipSlot>,
    Option<i32>,
    Option<&'static str>,
    Option<i32>,
);

fn lookup(name: &str) -> Option<Props> {
    ITEM_PROPS
        .binary_search_by(|(n, _, _, _, _, _)| (*n).cmp(name))
        .ok()
        .map(|i| {
            (
                ITEM_PROPS[i].1,
                ITEM_PROPS[i].2,
                ITEM_PROPS[i].3,
                ITEM_PROPS[i].4,
                ITEM_PROPS[i].5,
            )
        })
}

/// `Equippable.assetId()` — which `assets/minecraft/equipment/<asset>.json`
/// describes this item's armour layers (M46).
///
/// `None` covers two cases the caller must not confuse: an item that is not
/// equippable at all, and one that is worn but names no armour model (a carved
/// pumpkin). Both render no armour, which is why they share a return.
pub fn equip_asset(name: &str) -> Option<&'static str> {
    lookup(name).and_then(|(_, _, _, a, _)| a)
}

/// `getOrDefault(DataComponents.RARITY, Rarity.COMMON)`
/// — the **prototype** half of `ItemStack.getRarity`.
///
/// This is the one the wire cannot supply. A `minecraft:rarity` entry in the
/// patch overrides it (a plugin may), so the caller takes the patch first;
/// with no patch entry — which is the ordinary case for every stack in the
/// game — this is the answer, and defaulting to
/// `COMMON` instead paints all 115
/// non-common items white.
///
/// The **enchantment promotion** is not here: it depends on the stack, not the
/// item. See `ItemStack.getRarity`'s switch.
pub fn rarity(name: &str) -> i32 {
    lookup(name)
        .and_then(|(_, _, _, _, r)| r)
        .unwrap_or(DEFAULT_RARITY)
}

/// `stack.getMaxDamage()` — the denominator of a durability bar, or `None` for
/// an item that cannot be damaged.
///
/// The **numerator** is `minecraft:damage`, which does travel on the wire as a
/// patch; this does not, because a pickaxe's 1561 is the same on every
/// pickaxe. A patch that overrides `max_damage` wins over this, which is why
/// the caller takes the patch's value first.
pub fn max_damage(name: &str) -> Option<i32> {
    lookup(name).and_then(|(_, _, d, _, _)| d)
}

/// `stack.getMaxStackSize()` for an item name.
///
/// An item absent from the table takes [`DEFAULT_MAX_STACK`], which is correct
/// **only for a name that came out of the item registry** — the table lists
/// every real item that differs, so absence means "the default", not
/// "unknown". A name this build has never heard of is a different question,
/// and the caller must have failed to resolve it long before reaching here:
/// `Items::name` returns `None`, and the click path declines to predict rather
/// than guessing a cap.
pub fn max_stack_size(name: &str) -> i32 {
    lookup(name).and_then(|(s, _, _, _, _)| s).unwrap_or(DEFAULT_MAX_STACK)
}

/// The slot an item can be equipped into, or `None` if it carries no
/// `minecraft:equippable`.
///
/// `LivingEntity.isEquippableInSlot` treats that absence as **main hand only**,
/// so an item without the component is refused by every armour slot — which is
/// why this returns an `Option` rather than defaulting to anything.
pub fn equip_slot(name: &str) -> Option<EquipSlot> {
    lookup(name).and_then(|(_, q, _, _, _)| q)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The binary search is only correct if the generator emitted the table
    /// sorted.
    #[test]
    fn the_table_is_sorted() {
        assert!(ITEM_PROPS.windows(2).all(|w| w[0].0 < w[1].0));
    }

    /// One item per rarity bucket, pinned by hand from the report. A music
    /// disc is the case that made this column necessary: the wire says
    /// nothing about it, so a client reading only the patch draws its name
    /// white where vanilla draws it yellow.
    #[test]
    fn the_rarity_buckets_are_present() {
        assert_eq!(rarity("minecraft:dirt"), DEFAULT_RARITY);
        assert_eq!(rarity("minecraft:music_disc_13"), 1);
        assert_eq!(rarity("minecraft:enchanted_golden_apple"), 2);
        assert_eq!(rarity("minecraft:elytra"), 3);
    }

    /// A durability bar needs both halves, and only one of them is on the
    /// wire.
    #[test]
    fn damageable_items_carry_a_maximum() {
        assert_eq!(max_damage("minecraft:diamond_pickaxe"), Some(1561));
        assert_eq!(max_damage("minecraft:dirt"), None);
        assert_eq!(
            equip_asset("minecraft:diamond_chestplate"),
            Some("minecraft:diamond")
        );
        assert_eq!(equip_asset("minecraft:dirt"), None);
    }

    /// One item per stack-size bucket, pinned by hand from the report, so a
    /// regenerated table that collapsed to one number fails here.
    #[test]
    fn the_three_stack_buckets_are_present() {
        assert_eq!(max_stack_size("minecraft:dirt"), 64);
        assert_eq!(max_stack_size("minecraft:ender_pearl"), 16);
        assert_eq!(max_stack_size("minecraft:diamond_sword"), 1);
    }

    /// A registry item outside the table takes the default.
    #[test]
    fn an_item_outside_the_table_takes_the_default() {
        assert_eq!(max_stack_size("minecraft:stone"), DEFAULT_MAX_STACK);
    }

    /// The armour rule's inputs: a helmet names `head`, a shield names
    /// `offhand`, and dirt names nothing at all — which is what makes every
    /// armour slot refuse it.
    #[test]
    fn equippable_slots_are_read() {
        assert_eq!(equip_slot("minecraft:diamond_helmet"), Some(EquipSlot::Head));
        assert_eq!(equip_slot("minecraft:shield"), Some(EquipSlot::Offhand));
        assert_eq!(equip_slot("minecraft:dirt"), None);
    }

    /// A carved pumpkin is equippable **and** stacks to the default — it is in
    /// the table for the second reason only, which is the row a size-only
    /// table would have dropped.
    #[test]
    fn an_equippable_item_at_the_default_size_is_still_listed() {
        assert_eq!(max_stack_size("minecraft:carved_pumpkin"), DEFAULT_MAX_STACK);
        assert_eq!(equip_slot("minecraft:carved_pumpkin"), Some(EquipSlot::Head));
    }
}
