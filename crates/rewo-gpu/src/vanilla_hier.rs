//! Vanilla entity-model part hierarchies — GENERATED, do not edit.
//!
//! Regenerate with `python tools/gen_vanilla_hierarchy.py` after a version
//! bump. Source of truth is the decompiled client jar (REWO_PLAN §11).
//!
//! OptiFine CEM `.jem` top-level parts map onto *vanilla* model parts and
//! inherit vanilla's parent hierarchy, not just the `.jem` nesting — a pack
//! states an animated part's translation relative to that vanilla parent. The
//! vex's `right_arm` is a child of `body` in `VexModel`, so FA writes
//! `right_arm.ty = 0.6` against the body; treating it as absolute put the arms
//! 18 px above the mob. Only models with real nesting are listed; flat models
//! (every part under the root) need no entry.

pub const VANILLA_HIERARCHY: &[(&str, &[(&str, &str)])] = &[
    ("abstract_equine", &[("head", "head_parts"), ("left_ear", "head"), ("mane", "head_parts"), ("right_ear", "head"), ("tail", "body"), ("upper_mouth", "head_parts")]),
    ("abstract_piglin", &[("left_ear", "head"), ("right_ear", "head")]),
    ("adult_armadillo", &[("head", "body"), ("head_cube", "head"), ("left_ear", "head"), ("left_ear_cube", "left_ear"), ("right_ear", "head"), ("right_ear_cube", "right_ear"), ("tail", "body")]),
    ("adult_axolotl", &[("head", "body"), ("left_front_leg", "body"), ("left_gills", "head"), ("left_hind_leg", "body"), ("right_front_leg", "body"), ("right_gills", "head"), ("right_hind_leg", "body"), ("tail", "body"), ("top_gills", "head")]),
    ("adult_bee", &[("back_legs", "bone"), ("body", "bone"), ("front_legs", "bone"), ("left_antenna", "body"), ("left_wing", "bone"), ("middle_legs", "bone"), ("right_antenna", "body"), ("right_wing", "bone"), ("stinger", "body")]),
    ("adult_camel", &[("head", "body"), ("hump", "body"), ("left_ear", "head"), ("right_ear", "head"), ("tail", "body")]),
    ("adult_chicken", &[("beak", "head"), ("red_thing", "head")]),
    ("adult_fox", &[("left_ear", "head"), ("nose", "head"), ("right_ear", "head"), ("tail", "body")]),
    ("adult_rabbit", &[("frontlegs", "body"), ("head", "body"), ("left_ear", "head"), ("left_front_leg", "frontlegs"), ("left_haunch", "left_hind_leg"), ("left_hind_leg", "backlegs"), ("right_ear", "head"), ("right_front_leg", "frontlegs"), ("right_haunch", "right_hind_leg"), ("right_hind_leg", "backlegs"), ("tail", "body")]),
    ("adult_strider", &[("left_bottom_bristle", "body"), ("left_middle_bristle", "body"), ("left_top_bristle", "body"), ("right_bottom_bristle", "body"), ("right_middle_bristle", "body"), ("right_top_bristle", "body")]),
    ("adult_wolf", &[("real_head", "head"), ("real_tail", "tail")]),
    ("allay", &[("body", "root"), ("head", "root"), ("left_arm", "body"), ("left_wing", "body"), ("right_arm", "body"), ("right_wing", "body")]),
    ("armor_stand_armor", &[("hat", "head")]),
    ("baby_armadillo", &[("head", "body"), ("head_cube", "head"), ("left_ear", "head_cube"), ("right_ear", "head_cube"), ("right_ear_cube", "tail"), ("tail", "body")]),
    ("baby_axolotl", &[("body", "root"), ("head", "body"), ("left_front_leg", "body"), ("left_gills", "head"), ("left_hind_leg", "body"), ("right_front_leg", "body"), ("right_gills", "head"), ("right_hind_leg", "body"), ("right_leg_r1", "right_hind_leg"), ("tail", "body"), ("top_gills", "head")]),
    ("baby_bee", &[("back_legs", "bone"), ("body", "bone"), ("front_legs", "bone"), ("left_wing", "bone"), ("middle_legs", "bone"), ("right_wing", "bone"), ("stinger", "body")]),
    ("baby_camel", &[("head", "body"), ("left_ear", "head"), ("right_ear", "head"), ("tail", "body")]),
    ("baby_dolphin", &[("back_fin", "body"), ("head", "body"), ("left_fin", "body"), ("nose", "head"), ("right_fin", "body"), ("tail", "body"), ("tail_fin", "tail")]),
    ("baby_donkey", &[("head", "head_parts"), ("head_parts", "body"), ("head_r1", "head"), ("left_chest", "body"), ("left_ear", "head"), ("left_front_leg", "body"), ("left_hind_leg", "body"), ("neck_r1", "head_parts"), ("right_chest", "body"), ("right_ear", "head"), ("right_front_leg", "body"), ("right_hind_leg", "body"), ("tail", "body"), ("tail_r1", "tail")]),
    ("baby_fox", &[("tail", "body")]),
    ("baby_goat", &[("HeadMain", "head"), ("left_ear", "head"), ("left_horn", "head"), ("right_ear", "head"), ("right_horn", "head")]),
    ("baby_hoglin", &[("left_ear", "head"), ("right_ear", "head")]),
    ("baby_horse", &[("head", "head_parts"), ("left_ear", "head"), ("right_ear", "head"), ("tail", "body")]),
    ("baby_piglin", &[("hat", "head"), ("left_ear", "head"), ("left_ear_r1", "left_ear"), ("right_ear", "head"), ("right_ear_r1", "right_ear")]),
    ("baby_rabbit", &[("body_r1", "body"), ("frontlegs", "body"), ("head", "body"), ("left_ear", "head"), ("left_front_leg", "frontlegs"), ("left_front_leg_r1", "left_front_leg"), ("left_haunch", "left_hind_leg"), ("left_hind_leg", "backlegs"), ("right_ear", "head"), ("right_front_leg", "frontlegs"), ("right_front_leg_r1", "right_front_leg"), ("right_haunch", "right_hind_leg"), ("right_hind_leg", "backlegs"), ("tail", "body"), ("tail_r1", "tail")]),
    ("baby_strider", &[("bristle0", "body"), ("bristle1", "body"), ("bristle2", "body")]),
    ("baby_villager", &[("hat", "head"), ("hat_rim", "head"), ("middlearm_r1", "arms"), ("nose", "head"), ("right_hand", "arms")]),
    ("baby_wolf", &[("left_ear", "head"), ("right_ear", "head"), ("tail_r1", "tail")]),
    ("baby_zombie", &[("hat", "head")]),
    ("baby_zombie_villager", &[("hat", "head"), ("hat_rim", "head"), ("nose", "head")]),
    ("bat", &[("feet", "body"), ("left_ear", "head"), ("left_wing", "body"), ("left_wing_tip", "left_wing"), ("right_ear", "head"), ("right_wing", "body"), ("right_wing_tip", "right_wing")]),
    ("bell", &[("bell_base", "bell_body")]),
    ("breeze", &[("eyes", "head"), ("head", "body"), ("rod_1", "rods"), ("rod_2", "rods"), ("rod_3", "rods"), ("rods", "body"), ("wind_bottom", "wind_body"), ("wind_mid", "wind_bottom"), ("wind_top", "wind_mid")]),
    ("copper_golem", &[("body_r1", "body"), ("head", "body"), ("left_arm", "body"), ("left_arm_r1", "left_arm"), ("left_leg_r1", "left_leg"), ("rightItem", "right_arm"), ("right_arm", "body"), ("right_arm_r1", "right_arm"), ("right_leg_r1", "right_leg")]),
    ("creaking", &[("body", "upper_body"), ("head", "upper_body"), ("left_arm", "upper_body"), ("left_leg", "root"), ("right_arm", "upper_body"), ("right_leg", "root"), ("upper_body", "root")]),
    ("dolphin", &[("back_fin", "body"), ("head", "body"), ("left_fin", "body"), ("nose", "head"), ("right_fin", "body"), ("tail", "body"), ("tail_fin", "tail")]),
    ("dragon_head", &[("jaw", "head")]),
    ("end_crystal", &[("cube", "inner_glass"), ("inner_glass", "outer_glass")]),
    ("ender_dragon", &[("jaw", "head"), ("left_front_foot", "left_front_leg_tip"), ("left_front_leg", "body"), ("left_front_leg_tip", "left_front_leg"), ("left_hind_foot", "left_hind_leg_tip"), ("left_hind_leg", "body"), ("left_hind_leg_tip", "left_hind_leg"), ("left_wing", "body"), ("left_wing_tip", "left_wing"), ("right_front_foot", "right_front_leg_tip"), ("right_front_leg", "body"), ("right_front_leg_tip", "right_front_leg"), ("right_hind_foot", "right_hind_leg_tip"), ("right_hind_leg", "body"), ("right_hind_leg_tip", "right_hind_leg"), ("right_wing", "body"), ("right_wing_tip", "right_wing")]),
    ("enderman", &[("hat", "head")]),
    ("evoker_fangs", &[("lower_jaw", "base"), ("upper_jaw", "base")]),
    ("frog", &[("body", "root"), ("croaking_body", "body"), ("eyes", "head"), ("head", "body"), ("left_arm", "body"), ("left_eye", "eyes"), ("left_foot", "left_leg"), ("left_hand", "left_arm"), ("left_leg", "root"), ("right_arm", "body"), ("right_eye", "eyes"), ("right_foot", "right_leg"), ("right_hand", "right_arm"), ("right_leg", "root"), ("tongue", "body")]),
    ("goat", &[("left_horn", "head"), ("nose", "head"), ("right_horn", "head")]),
    ("guardian", &[("eye", "head"), ("tail0", "head"), ("tail1", "tail0"), ("tail2", "tail1")]),
    ("happy_ghast", &[("inner_body", "body")]),
    ("hoglin", &[("left_ear", "head"), ("left_horn", "head"), ("mane", "body"), ("right_ear", "head"), ("right_horn", "head")]),
    ("humanoid", &[("hat", "head"), ("left_foot", "right_leg"), ("right_foot", "left_leg")]),
    ("illager", &[("hat", "head"), ("left_shoulder", "arms"), ("nose", "head")]),
    ("nautilus", &[("body", "root"), ("inner_mouth", "body"), ("lower_mouth", "body"), ("shell", "root"), ("upper_mouth", "body")]),
    ("nautilus_armor", &[("shell", "root")]),
    ("nautilus_saddle", &[("shell", "root")]),
    ("parrot", &[("beak1", "head"), ("beak2", "head"), ("feather", "head"), ("head2", "head")]),
    ("phantom", &[("head", "body"), ("left_wing_base", "body"), ("left_wing_tip", "left_wing_base"), ("right_wing_base", "body"), ("right_wing_tip", "right_wing_base"), ("tail_base", "body"), ("tail_tip", "tail_base")]),
    ("player", &[("left_pants", "left_leg"), ("left_sleeve", "left_arm"), ("right_sleeve", "right_arm")]),
    ("ravager", &[("head", "neck"), ("left_horn", "head"), ("mouth", "head"), ("right_horn", "head")]),
    ("salmon", &[("back_fin", "body_back"), ("top_back_fin", "body_back"), ("top_front_fin", "body_front")]),
    ("sniffer", &[("body", "bone"), ("head", "body"), ("left_ear", "head"), ("left_front_leg", "bone"), ("left_hind_leg", "bone"), ("left_mid_leg", "bone"), ("lower_beak", "head"), ("nose", "head"), ("right_ear", "head"), ("right_front_leg", "bone"), ("right_hind_leg", "bone"), ("right_mid_leg", "bone")]),
    ("snifflet", &[("body", "bone"), ("head", "body"), ("left_ear", "head"), ("left_front_leg", "bone"), ("left_hind_leg", "bone"), ("left_mid_leg", "bone"), ("lower_beak", "head"), ("nose", "head"), ("right_ear", "head"), ("right_front_leg", "bone"), ("right_hind_leg", "bone"), ("right_mid_leg", "bone")]),
    ("trident", &[("base", "pole"), ("left_spike", "pole"), ("middle_spike", "pole"), ("right_spike", "pole")]),
    ("vex", &[("body", "root"), ("head", "root"), ("left_arm", "body"), ("left_wing", "body"), ("right_arm", "body"), ("right_wing", "body")]),
    ("villager", &[("hat", "head"), ("hat_rim", "hat"), ("jacket", "body"), ("nose", "head")]),
    ("warden", &[("body", "bone"), ("head", "body"), ("left_arm", "body"), ("left_leg", "bone"), ("left_ribcage", "body"), ("left_tendril", "head"), ("right_arm", "body"), ("right_leg", "bone"), ("right_ribcage", "body"), ("right_tendril", "head")]),
    ("wind_charge", &[("wind", "bone"), ("wind_charge", "bone")]),
    ("witch", &[("hat", "head"), ("hat2", "hat"), ("hat3", "hat2"), ("hat4", "hat3")]),
    ("zombie_nautilus_coral", &[("blue_first", "blue_coral"), ("blue_second", "blue_coral"), ("pink_coral_second", "pink_coral"), ("red_coral_first", "red_coral"), ("red_coral_second", "red_coral"), ("yellow_coral_first", "yellow_coral"), ("yellow_coral_second", "yellow_coral")]),
    ("zombie_villager", &[("hat", "head"), ("hat_rim", "hat")]),
];

/// Vanilla child→parent pairs for `entity` (`""`/unknown → empty, i.e. every
/// top-level part sits at the model root, which is right for flat models).
pub fn parents_for(entity: &str) -> &'static [(&'static str, &'static str)] {
    VANILLA_HIERARCHY
        .iter()
        .find(|(k, _)| *k == entity)
        .map_or(&[], |(_, v)| *v)
}
