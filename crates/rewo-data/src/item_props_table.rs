//! Per-item stack size and equippable slot — GENERATED, do not edit.
//!
//! Regenerate with `python tools/gen_item_props.py` after a version bump.
//!
//! Source: the datagen per-item component report for 26.2. Of 1537
//! items, 295 differ from `Item.DEFAULT_MAX_STACK_SIZE` = 64
//! (246 at 1, 49 at 16), 84 carry `minecraft:equippable`
//! (44 body, 8 chest, 7 feet, 16 head, 7 legs, 1 offhand, 1 saddle), 84 carry `minecraft:max_damage` and 43
//! distinct equipment assets are named. Items with
//! none of the three are not listed.
//!
//! Both feed the container click arithmetic. A wrong cap or a wrongly-allowed
//! armour placement predicts a wrong slot, the server's `HashedStack.matches`
//! fails, and the container resynchronises — which looks like a click that
//! bounced.

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

/// Every item that is not (default stack size, not equippable), sorted by name
/// so a binary search finds it. A `None` size means the default.
pub const ITEM_PROPS: &[(
    &str,
    Option<i32>,
    Option<EquipSlot>,
    Option<i32>,
    Option<&str>,
)] = &[
    ("minecraft:acacia_boat", Some(1), None, None, None),
    ("minecraft:acacia_chest_boat", Some(1), None, None, None),
    ("minecraft:acacia_hanging_sign", Some(16), None, None, None),
    ("minecraft:acacia_sign", Some(16), None, None, None),
    ("minecraft:armor_stand", Some(16), None, None, None),
    ("minecraft:axolotl_bucket", Some(1), None, None, None),
    ("minecraft:bamboo_chest_raft", Some(1), None, None, None),
    ("minecraft:bamboo_hanging_sign", Some(16), None, None, None),
    ("minecraft:bamboo_raft", Some(1), None, None, None),
    ("minecraft:bamboo_sign", Some(16), None, None, None),
    ("minecraft:beetroot_soup", Some(1), None, None, None),
    ("minecraft:birch_boat", Some(1), None, None, None),
    ("minecraft:birch_chest_boat", Some(1), None, None, None),
    ("minecraft:birch_hanging_sign", Some(16), None, None, None),
    ("minecraft:birch_sign", Some(16), None, None, None),
    ("minecraft:black_banner", Some(16), None, None, None),
    ("minecraft:black_bed", Some(1), None, None, None),
    ("minecraft:black_bundle", Some(1), None, None, None),
    ("minecraft:black_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:black_carpet")),
    ("minecraft:black_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:black_harness")),
    ("minecraft:black_shulker_box", Some(1), None, None, None),
    ("minecraft:blue_banner", Some(16), None, None, None),
    ("minecraft:blue_bed", Some(1), None, None, None),
    ("minecraft:blue_bundle", Some(1), None, None, None),
    ("minecraft:blue_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:blue_carpet")),
    ("minecraft:blue_egg", Some(16), None, None, None),
    ("minecraft:blue_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:blue_harness")),
    ("minecraft:blue_shulker_box", Some(1), None, None, None),
    ("minecraft:bordure_indented_banner_pattern", Some(1), None, None, None),
    ("minecraft:bow", Some(1), None, Some(384), None),
    ("minecraft:brown_banner", Some(16), None, None, None),
    ("minecraft:brown_bed", Some(1), None, None, None),
    ("minecraft:brown_bundle", Some(1), None, None, None),
    ("minecraft:brown_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:brown_carpet")),
    ("minecraft:brown_egg", Some(16), None, None, None),
    ("minecraft:brown_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:brown_harness")),
    ("minecraft:brown_shulker_box", Some(1), None, None, None),
    ("minecraft:brush", Some(1), None, Some(64), None),
    ("minecraft:bucket", Some(16), None, None, None),
    ("minecraft:bundle", Some(1), None, None, None),
    ("minecraft:cake", Some(1), None, None, None),
    ("minecraft:carrot_on_a_stick", Some(1), None, Some(25), None),
    ("minecraft:carved_pumpkin", None, Some(EquipSlot::Head), None, None),
    ("minecraft:chainmail_boots", Some(1), Some(EquipSlot::Feet), Some(195), Some("minecraft:chainmail")),
    ("minecraft:chainmail_chestplate", Some(1), Some(EquipSlot::Chest), Some(240), Some("minecraft:chainmail")),
    ("minecraft:chainmail_helmet", Some(1), Some(EquipSlot::Head), Some(165), Some("minecraft:chainmail")),
    ("minecraft:chainmail_leggings", Some(1), Some(EquipSlot::Legs), Some(225), Some("minecraft:chainmail")),
    ("minecraft:cherry_boat", Some(1), None, None, None),
    ("minecraft:cherry_chest_boat", Some(1), None, None, None),
    ("minecraft:cherry_hanging_sign", Some(16), None, None, None),
    ("minecraft:cherry_sign", Some(16), None, None, None),
    ("minecraft:chest_minecart", Some(1), None, None, None),
    ("minecraft:cod_bucket", Some(1), None, None, None),
    ("minecraft:command_block_minecart", Some(1), None, None, None),
    ("minecraft:copper_axe", Some(1), None, Some(190), None),
    ("minecraft:copper_boots", Some(1), Some(EquipSlot::Feet), Some(143), Some("minecraft:copper")),
    ("minecraft:copper_chestplate", Some(1), Some(EquipSlot::Chest), Some(176), Some("minecraft:copper")),
    ("minecraft:copper_helmet", Some(1), Some(EquipSlot::Head), Some(121), Some("minecraft:copper")),
    ("minecraft:copper_hoe", Some(1), None, Some(190), None),
    ("minecraft:copper_horse_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:copper")),
    ("minecraft:copper_leggings", Some(1), Some(EquipSlot::Legs), Some(165), Some("minecraft:copper")),
    ("minecraft:copper_nautilus_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:copper")),
    ("minecraft:copper_pickaxe", Some(1), None, Some(190), None),
    ("minecraft:copper_shovel", Some(1), None, Some(190), None),
    ("minecraft:copper_spear", Some(1), None, Some(190), None),
    ("minecraft:copper_sword", Some(1), None, Some(190), None),
    ("minecraft:creeper_banner_pattern", Some(1), None, None, None),
    ("minecraft:creeper_head", None, Some(EquipSlot::Head), None, None),
    ("minecraft:crimson_hanging_sign", Some(16), None, None, None),
    ("minecraft:crimson_sign", Some(16), None, None, None),
    ("minecraft:crossbow", Some(1), None, Some(465), None),
    ("minecraft:cyan_banner", Some(16), None, None, None),
    ("minecraft:cyan_bed", Some(1), None, None, None),
    ("minecraft:cyan_bundle", Some(1), None, None, None),
    ("minecraft:cyan_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:cyan_carpet")),
    ("minecraft:cyan_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:cyan_harness")),
    ("minecraft:cyan_shulker_box", Some(1), None, None, None),
    ("minecraft:dark_oak_boat", Some(1), None, None, None),
    ("minecraft:dark_oak_chest_boat", Some(1), None, None, None),
    ("minecraft:dark_oak_hanging_sign", Some(16), None, None, None),
    ("minecraft:dark_oak_sign", Some(16), None, None, None),
    ("minecraft:debug_stick", Some(1), None, None, None),
    ("minecraft:diamond_axe", Some(1), None, Some(1561), None),
    ("minecraft:diamond_boots", Some(1), Some(EquipSlot::Feet), Some(429), Some("minecraft:diamond")),
    ("minecraft:diamond_chestplate", Some(1), Some(EquipSlot::Chest), Some(528), Some("minecraft:diamond")),
    ("minecraft:diamond_helmet", Some(1), Some(EquipSlot::Head), Some(363), Some("minecraft:diamond")),
    ("minecraft:diamond_hoe", Some(1), None, Some(1561), None),
    ("minecraft:diamond_horse_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:diamond")),
    ("minecraft:diamond_leggings", Some(1), Some(EquipSlot::Legs), Some(495), Some("minecraft:diamond")),
    ("minecraft:diamond_nautilus_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:diamond")),
    ("minecraft:diamond_pickaxe", Some(1), None, Some(1561), None),
    ("minecraft:diamond_shovel", Some(1), None, Some(1561), None),
    ("minecraft:diamond_spear", Some(1), None, Some(1561), None),
    ("minecraft:diamond_sword", Some(1), None, Some(1561), None),
    ("minecraft:dragon_head", None, Some(EquipSlot::Head), None, None),
    ("minecraft:egg", Some(16), None, None, None),
    ("minecraft:elytra", Some(1), Some(EquipSlot::Chest), Some(432), Some("minecraft:elytra")),
    ("minecraft:enchanted_book", Some(1), None, None, None),
    ("minecraft:ender_pearl", Some(16), None, None, None),
    ("minecraft:field_masoned_banner_pattern", Some(1), None, None, None),
    ("minecraft:fishing_rod", Some(1), None, Some(64), None),
    ("minecraft:flint_and_steel", Some(1), None, Some(64), None),
    ("minecraft:flow_banner_pattern", Some(1), None, None, None),
    ("minecraft:flower_banner_pattern", Some(1), None, None, None),
    ("minecraft:furnace_minecart", Some(1), None, None, None),
    ("minecraft:globe_banner_pattern", Some(1), None, None, None),
    ("minecraft:goat_horn", Some(1), None, None, None),
    ("minecraft:golden_axe", Some(1), None, Some(32), None),
    ("minecraft:golden_boots", Some(1), Some(EquipSlot::Feet), Some(91), Some("minecraft:gold")),
    ("minecraft:golden_chestplate", Some(1), Some(EquipSlot::Chest), Some(112), Some("minecraft:gold")),
    ("minecraft:golden_helmet", Some(1), Some(EquipSlot::Head), Some(77), Some("minecraft:gold")),
    ("minecraft:golden_hoe", Some(1), None, Some(32), None),
    ("minecraft:golden_horse_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:gold")),
    ("minecraft:golden_leggings", Some(1), Some(EquipSlot::Legs), Some(105), Some("minecraft:gold")),
    ("minecraft:golden_nautilus_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:gold")),
    ("minecraft:golden_pickaxe", Some(1), None, Some(32), None),
    ("minecraft:golden_shovel", Some(1), None, Some(32), None),
    ("minecraft:golden_spear", Some(1), None, Some(32), None),
    ("minecraft:golden_sword", Some(1), None, Some(32), None),
    ("minecraft:gray_banner", Some(16), None, None, None),
    ("minecraft:gray_bed", Some(1), None, None, None),
    ("minecraft:gray_bundle", Some(1), None, None, None),
    ("minecraft:gray_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:gray_carpet")),
    ("minecraft:gray_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:gray_harness")),
    ("minecraft:gray_shulker_box", Some(1), None, None, None),
    ("minecraft:green_banner", Some(16), None, None, None),
    ("minecraft:green_bed", Some(1), None, None, None),
    ("minecraft:green_bundle", Some(1), None, None, None),
    ("minecraft:green_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:green_carpet")),
    ("minecraft:green_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:green_harness")),
    ("minecraft:green_shulker_box", Some(1), None, None, None),
    ("minecraft:guster_banner_pattern", Some(1), None, None, None),
    ("minecraft:honey_bottle", Some(16), None, None, None),
    ("minecraft:hopper_minecart", Some(1), None, None, None),
    ("minecraft:iron_axe", Some(1), None, Some(250), None),
    ("minecraft:iron_boots", Some(1), Some(EquipSlot::Feet), Some(195), Some("minecraft:iron")),
    ("minecraft:iron_chestplate", Some(1), Some(EquipSlot::Chest), Some(240), Some("minecraft:iron")),
    ("minecraft:iron_helmet", Some(1), Some(EquipSlot::Head), Some(165), Some("minecraft:iron")),
    ("minecraft:iron_hoe", Some(1), None, Some(250), None),
    ("minecraft:iron_horse_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:iron")),
    ("minecraft:iron_leggings", Some(1), Some(EquipSlot::Legs), Some(225), Some("minecraft:iron")),
    ("minecraft:iron_nautilus_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:iron")),
    ("minecraft:iron_pickaxe", Some(1), None, Some(250), None),
    ("minecraft:iron_shovel", Some(1), None, Some(250), None),
    ("minecraft:iron_spear", Some(1), None, Some(250), None),
    ("minecraft:iron_sword", Some(1), None, Some(250), None),
    ("minecraft:jungle_boat", Some(1), None, None, None),
    ("minecraft:jungle_chest_boat", Some(1), None, None, None),
    ("minecraft:jungle_hanging_sign", Some(16), None, None, None),
    ("minecraft:jungle_sign", Some(16), None, None, None),
    ("minecraft:knowledge_book", Some(1), None, None, None),
    ("minecraft:lava_bucket", Some(1), None, None, None),
    ("minecraft:leather_boots", Some(1), Some(EquipSlot::Feet), Some(65), Some("minecraft:leather")),
    ("minecraft:leather_chestplate", Some(1), Some(EquipSlot::Chest), Some(80), Some("minecraft:leather")),
    ("minecraft:leather_helmet", Some(1), Some(EquipSlot::Head), Some(55), Some("minecraft:leather")),
    ("minecraft:leather_horse_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:leather")),
    ("minecraft:leather_leggings", Some(1), Some(EquipSlot::Legs), Some(75), Some("minecraft:leather")),
    ("minecraft:light_blue_banner", Some(16), None, None, None),
    ("minecraft:light_blue_bed", Some(1), None, None, None),
    ("minecraft:light_blue_bundle", Some(1), None, None, None),
    ("minecraft:light_blue_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:light_blue_carpet")),
    ("minecraft:light_blue_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:light_blue_harness")),
    ("minecraft:light_blue_shulker_box", Some(1), None, None, None),
    ("minecraft:light_gray_banner", Some(16), None, None, None),
    ("minecraft:light_gray_bed", Some(1), None, None, None),
    ("minecraft:light_gray_bundle", Some(1), None, None, None),
    ("minecraft:light_gray_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:light_gray_carpet")),
    ("minecraft:light_gray_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:light_gray_harness")),
    ("minecraft:light_gray_shulker_box", Some(1), None, None, None),
    ("minecraft:lime_banner", Some(16), None, None, None),
    ("minecraft:lime_bed", Some(1), None, None, None),
    ("minecraft:lime_bundle", Some(1), None, None, None),
    ("minecraft:lime_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:lime_carpet")),
    ("minecraft:lime_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:lime_harness")),
    ("minecraft:lime_shulker_box", Some(1), None, None, None),
    ("minecraft:lingering_potion", Some(1), None, None, None),
    ("minecraft:mace", Some(1), None, Some(500), None),
    ("minecraft:magenta_banner", Some(16), None, None, None),
    ("minecraft:magenta_bed", Some(1), None, None, None),
    ("minecraft:magenta_bundle", Some(1), None, None, None),
    ("minecraft:magenta_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:magenta_carpet")),
    ("minecraft:magenta_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:magenta_harness")),
    ("minecraft:magenta_shulker_box", Some(1), None, None, None),
    ("minecraft:mangrove_boat", Some(1), None, None, None),
    ("minecraft:mangrove_chest_boat", Some(1), None, None, None),
    ("minecraft:mangrove_hanging_sign", Some(16), None, None, None),
    ("minecraft:mangrove_sign", Some(16), None, None, None),
    ("minecraft:milk_bucket", Some(1), None, None, None),
    ("minecraft:minecart", Some(1), None, None, None),
    ("minecraft:mojang_banner_pattern", Some(1), None, None, None),
    ("minecraft:mushroom_stew", Some(1), None, None, None),
    ("minecraft:music_disc_11", Some(1), None, None, None),
    ("minecraft:music_disc_13", Some(1), None, None, None),
    ("minecraft:music_disc_5", Some(1), None, None, None),
    ("minecraft:music_disc_blocks", Some(1), None, None, None),
    ("minecraft:music_disc_bounce", Some(1), None, None, None),
    ("minecraft:music_disc_cat", Some(1), None, None, None),
    ("minecraft:music_disc_chirp", Some(1), None, None, None),
    ("minecraft:music_disc_creator", Some(1), None, None, None),
    ("minecraft:music_disc_creator_music_box", Some(1), None, None, None),
    ("minecraft:music_disc_far", Some(1), None, None, None),
    ("minecraft:music_disc_lava_chicken", Some(1), None, None, None),
    ("minecraft:music_disc_mall", Some(1), None, None, None),
    ("minecraft:music_disc_mellohi", Some(1), None, None, None),
    ("minecraft:music_disc_otherside", Some(1), None, None, None),
    ("minecraft:music_disc_pigstep", Some(1), None, None, None),
    ("minecraft:music_disc_precipice", Some(1), None, None, None),
    ("minecraft:music_disc_relic", Some(1), None, None, None),
    ("minecraft:music_disc_stal", Some(1), None, None, None),
    ("minecraft:music_disc_strad", Some(1), None, None, None),
    ("minecraft:music_disc_tears", Some(1), None, None, None),
    ("minecraft:music_disc_wait", Some(1), None, None, None),
    ("minecraft:music_disc_ward", Some(1), None, None, None),
    ("minecraft:netherite_axe", Some(1), None, Some(2031), None),
    ("minecraft:netherite_boots", Some(1), Some(EquipSlot::Feet), Some(481), Some("minecraft:netherite")),
    ("minecraft:netherite_chestplate", Some(1), Some(EquipSlot::Chest), Some(592), Some("minecraft:netherite")),
    ("minecraft:netherite_helmet", Some(1), Some(EquipSlot::Head), Some(407), Some("minecraft:netherite")),
    ("minecraft:netherite_hoe", Some(1), None, Some(2031), None),
    ("minecraft:netherite_horse_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:netherite")),
    ("minecraft:netherite_leggings", Some(1), Some(EquipSlot::Legs), Some(555), Some("minecraft:netherite")),
    ("minecraft:netherite_nautilus_armor", Some(1), Some(EquipSlot::Body), None, Some("minecraft:netherite")),
    ("minecraft:netherite_pickaxe", Some(1), None, Some(2031), None),
    ("minecraft:netherite_shovel", Some(1), None, Some(2031), None),
    ("minecraft:netherite_spear", Some(1), None, Some(2031), None),
    ("minecraft:netherite_sword", Some(1), None, Some(2031), None),
    ("minecraft:oak_boat", Some(1), None, None, None),
    ("minecraft:oak_chest_boat", Some(1), None, None, None),
    ("minecraft:oak_hanging_sign", Some(16), None, None, None),
    ("minecraft:oak_sign", Some(16), None, None, None),
    ("minecraft:orange_banner", Some(16), None, None, None),
    ("minecraft:orange_bed", Some(1), None, None, None),
    ("minecraft:orange_bundle", Some(1), None, None, None),
    ("minecraft:orange_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:orange_carpet")),
    ("minecraft:orange_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:orange_harness")),
    ("minecraft:orange_shulker_box", Some(1), None, None, None),
    ("minecraft:pale_oak_boat", Some(1), None, None, None),
    ("minecraft:pale_oak_chest_boat", Some(1), None, None, None),
    ("minecraft:pale_oak_hanging_sign", Some(16), None, None, None),
    ("minecraft:pale_oak_sign", Some(16), None, None, None),
    ("minecraft:piglin_banner_pattern", Some(1), None, None, None),
    ("minecraft:piglin_head", None, Some(EquipSlot::Head), None, None),
    ("minecraft:pink_banner", Some(16), None, None, None),
    ("minecraft:pink_bed", Some(1), None, None, None),
    ("minecraft:pink_bundle", Some(1), None, None, None),
    ("minecraft:pink_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:pink_carpet")),
    ("minecraft:pink_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:pink_harness")),
    ("minecraft:pink_shulker_box", Some(1), None, None, None),
    ("minecraft:player_head", None, Some(EquipSlot::Head), None, None),
    ("minecraft:potion", Some(1), None, None, None),
    ("minecraft:powder_snow_bucket", Some(1), None, None, None),
    ("minecraft:pufferfish_bucket", Some(1), None, None, None),
    ("minecraft:purple_banner", Some(16), None, None, None),
    ("minecraft:purple_bed", Some(1), None, None, None),
    ("minecraft:purple_bundle", Some(1), None, None, None),
    ("minecraft:purple_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:purple_carpet")),
    ("minecraft:purple_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:purple_harness")),
    ("minecraft:purple_shulker_box", Some(1), None, None, None),
    ("minecraft:rabbit_stew", Some(1), None, None, None),
    ("minecraft:red_banner", Some(16), None, None, None),
    ("minecraft:red_bed", Some(1), None, None, None),
    ("minecraft:red_bundle", Some(1), None, None, None),
    ("minecraft:red_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:red_carpet")),
    ("minecraft:red_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:red_harness")),
    ("minecraft:red_shulker_box", Some(1), None, None, None),
    ("minecraft:saddle", Some(1), Some(EquipSlot::Saddle), None, Some("minecraft:saddle")),
    ("minecraft:salmon_bucket", Some(1), None, None, None),
    ("minecraft:shears", Some(1), None, Some(238), None),
    ("minecraft:shield", Some(1), Some(EquipSlot::Offhand), Some(336), None),
    ("minecraft:shulker_box", Some(1), None, None, None),
    ("minecraft:skeleton_skull", None, Some(EquipSlot::Head), None, None),
    ("minecraft:skull_banner_pattern", Some(1), None, None, None),
    ("minecraft:snowball", Some(16), None, None, None),
    ("minecraft:splash_potion", Some(1), None, None, None),
    ("minecraft:spruce_boat", Some(1), None, None, None),
    ("minecraft:spruce_chest_boat", Some(1), None, None, None),
    ("minecraft:spruce_hanging_sign", Some(16), None, None, None),
    ("minecraft:spruce_sign", Some(16), None, None, None),
    ("minecraft:spyglass", Some(1), None, None, None),
    ("minecraft:stone_axe", Some(1), None, Some(131), None),
    ("minecraft:stone_hoe", Some(1), None, Some(131), None),
    ("minecraft:stone_pickaxe", Some(1), None, Some(131), None),
    ("minecraft:stone_shovel", Some(1), None, Some(131), None),
    ("minecraft:stone_spear", Some(1), None, Some(131), None),
    ("minecraft:stone_sword", Some(1), None, Some(131), None),
    ("minecraft:sulfur_cube_bucket", Some(1), None, None, None),
    ("minecraft:suspicious_stew", Some(1), None, None, None),
    ("minecraft:tadpole_bucket", Some(1), None, None, None),
    ("minecraft:tnt_minecart", Some(1), None, None, None),
    ("minecraft:totem_of_undying", Some(1), None, None, None),
    ("minecraft:trident", Some(1), None, Some(250), None),
    ("minecraft:tropical_fish_bucket", Some(1), None, None, None),
    ("minecraft:turtle_helmet", Some(1), Some(EquipSlot::Head), Some(275), Some("minecraft:turtle_scute")),
    ("minecraft:warped_fungus_on_a_stick", Some(1), None, Some(100), None),
    ("minecraft:warped_hanging_sign", Some(16), None, None, None),
    ("minecraft:warped_sign", Some(16), None, None, None),
    ("minecraft:water_bucket", Some(1), None, None, None),
    ("minecraft:white_banner", Some(16), None, None, None),
    ("minecraft:white_bed", Some(1), None, None, None),
    ("minecraft:white_bundle", Some(1), None, None, None),
    ("minecraft:white_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:white_carpet")),
    ("minecraft:white_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:white_harness")),
    ("minecraft:white_shulker_box", Some(1), None, None, None),
    ("minecraft:wither_skeleton_skull", None, Some(EquipSlot::Head), None, None),
    ("minecraft:wolf_armor", Some(1), Some(EquipSlot::Body), Some(64), Some("minecraft:armadillo_scute")),
    ("minecraft:wooden_axe", Some(1), None, Some(59), None),
    ("minecraft:wooden_hoe", Some(1), None, Some(59), None),
    ("minecraft:wooden_pickaxe", Some(1), None, Some(59), None),
    ("minecraft:wooden_shovel", Some(1), None, Some(59), None),
    ("minecraft:wooden_spear", Some(1), None, Some(59), None),
    ("minecraft:wooden_sword", Some(1), None, Some(59), None),
    ("minecraft:writable_book", Some(1), None, None, None),
    ("minecraft:written_book", Some(16), None, None, None),
    ("minecraft:yellow_banner", Some(16), None, None, None),
    ("minecraft:yellow_bed", Some(1), None, None, None),
    ("minecraft:yellow_bundle", Some(1), None, None, None),
    ("minecraft:yellow_carpet", None, Some(EquipSlot::Body), None, Some("minecraft:yellow_carpet")),
    ("minecraft:yellow_harness", Some(1), Some(EquipSlot::Body), None, Some("minecraft:yellow_harness")),
    ("minecraft:yellow_shulker_box", Some(1), None, None, None),
    ("minecraft:zombie_head", None, Some(EquipSlot::Head), None, None),
];

type Props = (
    Option<i32>,
    Option<EquipSlot>,
    Option<i32>,
    Option<&'static str>,
);

fn lookup(name: &str) -> Option<Props> {
    ITEM_PROPS
        .binary_search_by(|(n, _, _, _, _)| (*n).cmp(name))
        .ok()
        .map(|i| {
            (
                ITEM_PROPS[i].1,
                ITEM_PROPS[i].2,
                ITEM_PROPS[i].3,
                ITEM_PROPS[i].4,
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
    lookup(name).and_then(|(_, _, _, a)| a)
}

/// `stack.getMaxDamage()` — the denominator of a durability bar, or `None` for
/// an item that cannot be damaged.
///
/// The **numerator** is `minecraft:damage`, which does travel on the wire as a
/// patch; this does not, because a pickaxe's 1561 is the same on every
/// pickaxe. A patch that overrides `max_damage` wins over this, which is why
/// the caller takes the patch's value first.
pub fn max_damage(name: &str) -> Option<i32> {
    lookup(name).and_then(|(_, _, d, _)| d)
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
    lookup(name).and_then(|(s, _, _, _)| s).unwrap_or(DEFAULT_MAX_STACK)
}

/// The slot an item can be equipped into, or `None` if it carries no
/// `minecraft:equippable`.
///
/// `LivingEntity.isEquippableInSlot` treats that absence as **main hand only**,
/// so an item without the component is refused by every armour slot — which is
/// why this returns an `Option` rather than defaulting to anything.
pub fn equip_slot(name: &str) -> Option<EquipSlot> {
    lookup(name).and_then(|(_, q, _, _)| q)
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
