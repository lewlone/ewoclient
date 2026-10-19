//! The local half of a right-click (M229): `Minecraft.startUseItem` and the
//! client's own run of `MultiPlayerGameMode.performUseItemOn`.
//!
//! Vanilla always sends the packets, and the LOCAL result of each step only
//! decides what is tried next:
//!
//! ```text
//! for hand in [MAIN_HAND, OFF_HAND]:
//!     BLOCK hit:  useItemOn(hand, hit)          -- packet sent regardless
//!                 Success / Fail -> done; Pass -> fall through
//!     stack not empty: useItem(hand)            -- packet sent regardless
//!                 Success -> done; otherwise the next hand
//! ```
//!
//! `performUseItemOn` on the client is
//!
//! ```text
//! suppress = isSecondaryUseActive() && (main or off hand not empty)
//! if !suppress:
//!     r = block.useItemOn(stack, ...)      -- TRY_WITH_EMPTY_HAND by default
//!     if r consumes: return r
//!     if r is TRY_WITH_EMPTY_HAND and hand == MAIN_HAND:
//!         r = block.useWithoutItem(...)    -- PASS by default
//!         if r consumes: return r
//! if stack not empty: return stack.useOn(...)   -- PASS by default
//! return PASS
//! ```
//!
//! Which class's override answers comes from the decompile
//! ([`rewo_data::click_use_table`], `tools/gen_click_use.py`); the rules are
//! here, one per class, each the class's client-side return transcribed.
//!
//! **Recorded approximations** (each errs toward ending the click, because
//! a wrongly-passed click sends a second packet the server acts on — eating
//! after opening a chest — while a wrongly-ended one only fails to use the
//! item):
//! - `canUseGameMasterBlocks()` (command, structure, jigsaw, test blocks) is
//!   taken as creative: the permission level is not on the wire.
//! - `ChiseledBookShelfBlock` and `ShelfBlock` hit slots are not resolved
//!   (every face counts as a slot); `BellBlock.onHit`'s face test is not run.
//! - A cauldron's `CauldronInteraction` map is reduced to the buckets, the
//!   bottle, potions and (filled with water) dyed items, banners and shulker
//!   boxes; a potion is not checked for being water.
//! - `RedStoneWireBlock`'s shape toggle is taken to change the state whenever
//!   the wire is a dot or a cross.
//! - `PotionItem.useOn` (mud) is taken for any potion on dirt, coarse dirt or
//!   rooted dirt; the potion is not checked for being water.
//! - An item's own `use` (a class override) counts as a success, which stops
//!   the hand loop even where vanilla's would pass (an empty bucket aimed at
//!   no fluid does not then raise an off-hand shield).

use rewo_data::click_use_table::{
    ALWAYS_EDIBLE, BLOCK_ITEM_ON_CLASS, BLOCK_WITHOUT_ITEM_CLASS, COMPOSTABLES, FOOD,
    ITEM_CLICK_CLASS, ITEM_USE_CLASS,
};

/// `InteractionHand`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Main,
    Off,
}

/// The part of `InteractionResult` the click loop reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// `Success` or `Consume`: the click is spent.
    Consumed,
    /// `Fail`: also spent; nothing else is tried.
    Fail,
    /// `Pass`: the loop goes on (to `useItem`, or to the next hand).
    Pass,
}

/// `useItemOn`'s three answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ItemOn {
    Consumed,
    TryEmptyHand,
    Pass,
}

/// The clicked block.
pub struct Clicked<'a> {
    /// Registry name (`minecraft:oak_door`).
    pub block: &'a str,
    /// The clicked state's property value by key (`"true"`, `"3"`).
    pub prop: &'a dyn Fn(&str) -> Option<String>,
    /// `hitResult.getDirection()` as `Direction.get3DDataValue` (0 down, 1 up).
    pub face: u8,
}

/// The player doing the clicking.
pub struct Clicker<'a> {
    /// Item registry names; `None` = an empty hand.
    pub main: Option<&'a str>,
    pub off: Option<&'a str>,
    /// `isSecondaryUseActive()` — sneaking.
    pub sneaking: bool,
    /// `abilities.instabuild`.
    pub creative: bool,
    /// `abilities.mayBuild` — survival or creative, not adventure.
    pub may_build: bool,
    /// `FoodData.foodLevel`.
    pub food: i32,
    /// `stack.is(tag)` for an item tag (`"minecraft:candles"`).
    pub item_tag: &'a dyn Fn(&str, &str) -> bool,
    /// `RecipePropertySet.CAMPFIRE_INPUT.test(stack)`.
    pub campfire_input: &'a dyn Fn(&str) -> bool,
    /// Whether a block of this registry name exists (`potted_<plant>`).
    pub block_exists: &'a dyn Fn(&str) -> bool,
}

impl Clicker<'_> {
    fn held(&self, hand: Hand) -> Option<&str> {
        match hand {
            Hand::Main => self.main,
            Hand::Off => self.off,
        }
    }

    /// `Player.canEat(false)` with `abilities.invulnerable` taken as creative:
    /// `invulnerable || foodData.needsFood()`.
    fn can_eat(&self) -> bool {
        self.creative || self.food < 20
    }

    fn is(&self, item: Option<&str>, tag: &str) -> bool {
        item.is_some_and(|i| (self.item_tag)(i, tag))
    }
}

fn lookup<'t>(table: &'t [(&'t str, &'t str)], key: &str) -> Option<&'t str> {
    table
        .binary_search_by(|(k, _)| (*k).cmp(key))
        .ok()
        .map(|i| table[i].1)
}

/// The class whose `useItemOn` answers for this block (`None`: the default,
/// `TRY_WITH_EMPTY_HAND`).
pub fn block_item_on_class(block: &str) -> Option<&'static str> {
    lookup(BLOCK_ITEM_ON_CLASS, block)
}

/// The class whose `useWithoutItem` answers for this block (`None`: the
/// default, `PASS`). Separate from [`block_item_on_class`]: a cauldron
/// overrides only `useItemOn`, a hanging sign `useItemOn` while its
/// `useWithoutItem` is `SignBlock`'s.
pub fn block_without_item_class(block: &str) -> Option<&'static str> {
    lookup(BLOCK_WITHOUT_ITEM_CLASS, block)
}

/// The class whose `useOn` answers for this item; an item named after a block
/// (the colour and weathering collections) is a `BlockItem`.
pub fn item_use_on_class(item: &str, block_exists: &dyn Fn(&str) -> bool) -> Option<&'static str> {
    lookup(ITEM_CLICK_CLASS, item).or_else(|| block_exists(item).then_some("BlockItem"))
}

fn is_true(c: &Clicked, key: &str) -> bool {
    (c.prop)(key).as_deref() == Some("true")
}

fn int_prop(c: &Clicked, key: &str) -> i32 {
    (c.prop)(key).and_then(|v| v.parse().ok()).unwrap_or(0)
}

fn short(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

/// `block.useItemOn(stack, ...)` for the clicked block's class.
fn block_item_on(class: Option<&str>, c: &Clicked, p: &Clicker, hand: Hand) -> ItemOn {
    use ItemOn::*;
    let held = p.held(hand);
    let is = |name: &str| held == Some(name);
    match class {
        Some("AbstractCauldronBlock") => {
            let filled_bucket = ["water_bucket", "lava_bucket", "powder_snow_bucket"]
                .iter()
                .any(|b| held.map(short) == Some(*b));
            let hit = match short(c.block) {
                "cauldron" => filled_bucket || is("minecraft:potion"),
                "water_cauldron" => {
                    filled_bucket
                        || is("minecraft:bucket")
                        || is("minecraft:glass_bottle")
                        || is("minecraft:potion")
                        || p.is(held, "minecraft:cauldron_can_remove_dye")
                        || held.is_some_and(|h| {
                            h.ends_with("_banner")
                                || (h.ends_with("_shulker_box") && h != "minecraft:shulker_box")
                        })
                }
                _ => filled_bucket || is("minecraft:bucket"),
            };
            if hit {
                Consumed
            } else {
                TryEmptyHand
            }
        }
        Some("BeehiveBlock") => {
            if int_prop(c, "honey_level") >= 5
                && (is("minecraft:shears") || is("minecraft:glass_bottle"))
            {
                Consumed
            } else {
                TryEmptyHand
            }
        }
        Some("CakeBlock") => {
            if p.is(held, "minecraft:candles") && int_prop(c, "bites") == 0 {
                Consumed
            } else {
                TryEmptyHand
            }
        }
        Some("CampfireBlock") => {
            if held.is_some_and(|h| (p.campfire_input)(h)) {
                Consumed
            } else {
                TryEmptyHand
            }
        }
        Some("CandleBlock") => {
            if held.is_none() && p.may_build && is_true(c, "lit") {
                Consumed
            } else {
                TryEmptyHand
            }
        }
        Some("CandleCakeBlock") => {
            if is("minecraft:flint_and_steel") || is("minecraft:fire_charge") {
                Pass
            } else if held.is_none() && is_true(c, "lit") {
                Consumed
            } else {
                TryEmptyHand
            }
        }
        Some("CeilingHangingSignBlock") | Some("WallHangingSignBlock")
            if held.is_some_and(|h| h.ends_with("_hanging_sign")) =>
        {
            // `shouldTryToChainAnotherHangingSign`: placing another below.
            Pass
        }
        // `SignBlock.useItemOn`'s client arm: CONSUME or SUCCESS, both spent.
        Some("SignBlock") | Some("CeilingHangingSignBlock") | Some("WallHangingSignBlock") => {
            Consumed
        }
        Some("ChiseledBookShelfBlock") => {
            if p.is(held, "minecraft:bookshelf_books") {
                Consumed
            } else {
                TryEmptyHand
            }
        }
        Some("ComposterBlock") => {
            if int_prop(c, "level") < 8 && held.is_some_and(|h| COMPOSTABLES.contains(&h)) {
                Consumed
            } else {
                TryEmptyHand
            }
        }
        Some("CopperGolemStatueBlock") => {
            if p.is(held, "minecraft:axes") {
                Pass
            } else {
                Consumed
            }
        }
        Some("WeatheringCopperGolemStatueBlock") => {
            if p.is(held, "minecraft:axes") {
                // Only the unaffected statue comes alive.
                let aged = ["exposed_", "weathered_", "oxidized_"]
                    .iter()
                    .any(|a| short(c.block).contains(a));
                if aged {
                    Pass
                } else {
                    Consumed
                }
            } else if is("minecraft:honeycomb") {
                Pass
            } else {
                Consumed
            }
        }
        // `DecoratedPotBlock.useItemOn`'s client arm returns SUCCESS.
        Some("DecoratedPotBlock") => Consumed,
        Some("FlowerPotBlock") => {
            let pottable =
                held.is_some_and(|h| (p.block_exists)(&format!("minecraft:potted_{}", short(h))));
            if pottable {
                Consumed
            } else {
                TryEmptyHand
            }
        }
        Some("JukeboxBlock") => {
            if !is_true(c, "has_record")
                && held.is_some_and(|h| h.starts_with("minecraft:music_disc_"))
            {
                Consumed
            } else {
                TryEmptyHand
            }
        }
        Some("LecternBlock") => {
            if is_true(c, "has_book") {
                TryEmptyHand
            } else if p.is(held, "minecraft:lectern_books") {
                Consumed
            } else if held.is_none() && hand == Hand::Main {
                Pass
            } else {
                TryEmptyHand
            }
        }
        Some("NoteBlock") => {
            if p.is(held, "minecraft:noteblock_top_instruments") && c.face == 1 {
                Pass
            } else {
                TryEmptyHand
            }
        }
        Some("PumpkinBlock") => {
            if is("minecraft:shears") {
                Consumed
            } else {
                TryEmptyHand
            }
        }
        Some("RedStoneOreBlock") => {
            // PASS only for a block item that can be placed (taken as any).
            if held.is_some_and(|h| item_use_on_class(h, p.block_exists) == Some("BlockItem")) {
                Pass
            } else {
                Consumed
            }
        }
        Some("RespawnAnchorBlock") => {
            let chargeable = int_prop(c, "charge") < 4;
            if is("minecraft:glowstone") && chargeable {
                Consumed
            } else if hand == Hand::Main && p.off == Some("minecraft:glowstone") && chargeable {
                Pass
            } else {
                TryEmptyHand
            }
        }
        Some("ShelfBlock") => {
            if hand == Hand::Off || p.main.is_none() {
                Pass
            } else {
                Consumed
            }
        }
        Some("SweetBerryBushBlock") => {
            if int_prop(c, "age") != 3 && is("minecraft:bone_meal") {
                Pass
            } else {
                TryEmptyHand
            }
        }
        Some("TntBlock") => {
            if is("minecraft:flint_and_steel") || is("minecraft:fire_charge") {
                Consumed
            } else {
                TryEmptyHand
            }
        }
        _ => TryEmptyHand,
    }
}

/// `block.useWithoutItem(...)` for the clicked block's class: whether it
/// consumes.
fn block_without_item(class: Option<&str>, c: &Clicked, p: &Clicker) -> bool {
    let creative_op = p.creative;
    match class {
        None | Some("FenceBlock") | Some("MovingPistonBlock") => false,
        // `BellBlock.onHit`'s face test is not run: taken as a ring.
        Some("CakeBlock") => p.can_eat() || p.main.is_none(),
        Some("CandleCakeBlock") => p.can_eat(),
        Some("CaveVinesBlock") | Some("CaveVinesPlantBlock") => is_true(c, "berries"),
        Some("CommandBlock")
        | Some("JigsawBlock")
        | Some("StructureBlock")
        | Some("TestBlock")
        | Some("TestInstanceBlock") => creative_op,
        Some("ComparatorBlock") | Some("RepeaterBlock") | Some("DaylightDetectorBlock") => {
            p.may_build
        }
        Some("ComposterBlock") => int_prop(c, "level") == 8,
        Some("DoorBlock") | Some("TrapDoorBlock") => {
            // `BlockSetType.canOpenByHand()`: false for iron alone.
            !matches!(short(c.block), "iron_door" | "iron_trapdoor")
        }
        Some("JukeboxBlock") => is_true(c, "has_record"),
        Some("RedStoneWireBlock") => {
            let sides: Vec<String> = ["north", "east", "south", "west"]
                .iter()
                .map(|k| (c.prop)(k).unwrap_or_default())
                .collect();
            let dot = sides.iter().all(|s| s == "none");
            let cross = sides.iter().all(|s| s != "none");
            p.may_build && (dot || cross)
        }
        Some("RespawnAnchorBlock") => int_prop(c, "charge") > 0,
        Some("SweetBerryBushBlock") => int_prop(c, "age") > 1,
        // Every other override returns SUCCESS / CONSUME on the client:
        // containers, beds, buttons, levers, gates, the lectern, the flower
        // pot, signs, the light block, the dragon egg, the note block...
        Some(_) => true,
    }
}

/// `stack.useOn(context)` for the item's class.
fn item_use_on(item: &str, c: &Clicked, p: &Clicker) -> Outcome {
    match item_use_on_class(item, p.block_exists) {
        None => Outcome::Pass,
        // `PlaceOnWaterBlockItem.useOn` returns PASS (it places from `use`).
        Some("PlaceOnWaterBlockItem") => Outcome::Pass,
        Some("EnderEyeItem") => {
            if c.block == "minecraft:end_portal_frame" && !is_true(c, "eye") {
                Outcome::Consumed
            } else {
                Outcome::Pass
            }
        }
        Some("PotionItem") => {
            let dirt = matches!(short(c.block), "dirt" | "coarse_dirt" | "rooted_dirt");
            if dirt && c.face != 0 {
                Outcome::Consumed
            } else {
                Outcome::Pass
            }
        }
        // `BlockItem.useOn`: placed (SUCCESS) or not (FAIL) — either way the
        // click is spent (an unplaceable consumable eats inside `useOn`).
        // Every other override spends it too, or has no `use` to fall to.
        Some(_) => Outcome::Consumed,
    }
}

/// `MultiPlayerGameMode.performUseItemOn`'s local result for one hand.
pub fn use_item_on(c: &Clicked, p: &Clicker, hand: Hand) -> Outcome {
    let suppress = p.sneaking && (p.main.is_some() || p.off.is_some());
    if !suppress {
        match block_item_on(block_item_on_class(c.block), c, p, hand) {
            ItemOn::Consumed => return Outcome::Consumed,
            ItemOn::TryEmptyHand
                if hand == Hand::Main
                    && block_without_item(block_without_item_class(c.block), c, p) =>
            {
                return Outcome::Consumed;
            }
            _ => {}
        }
    }
    match p.held(hand) {
        Some(item) => item_use_on(item, c, p),
        None => Outcome::Pass,
    }
}

/// What the item's own `use` would answer locally, as far as the click loop
/// needs (whether the next hand is tried). `use_duration` is
/// `getUseDuration`; `armor_slot` whether the item's `equippable` slot is a
/// player armour slot (`Equippable.swapWithEquipmentSlot`).
pub fn use_item(item: &str, p: &Clicker, use_duration: i32, armor_slot: bool) -> Outcome {
    if lookup(ITEM_USE_CLASS, item).is_some() {
        return Outcome::Consumed;
    }
    // `Item.use`: a consumable starts if `Consumable.canConsume` (food needs
    // `canEat(canAlwaysEat)`), else FAIL; then a swappable equippable swaps;
    // then `BLOCKS_ATTACKS` / `KINETIC_WEAPON` start using.
    if use_duration > 0 {
        let food_refused = FOOD.contains(&item) && !ALWAYS_EDIBLE.contains(&item) && !p.can_eat();
        return if food_refused {
            Outcome::Fail
        } else {
            Outcome::Consumed
        };
    }
    if armor_slot {
        return Outcome::Consumed;
    }
    Outcome::Pass
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Env {
        props: Vec<(&'static str, &'static str)>,
        tags: Vec<(&'static str, &'static str)>,
    }

    fn clicked<'a>(block: &'a str, prop: &'a dyn Fn(&str) -> Option<String>) -> Clicked<'a> {
        Clicked {
            block,
            prop,
            face: 1,
        }
    }

    fn run(
        block: &str,
        env: &Env,
        main: Option<&str>,
        off: Option<&str>,
        sneaking: bool,
        food: i32,
    ) -> Outcome {
        let prop = |k: &str| {
            env.props
                .iter()
                .find(|(pk, _)| *pk == k)
                .map(|(_, v)| v.to_string())
        };
        let tag = |i: &str, t: &str| env.tags.iter().any(|(ti, tt)| *ti == i && *tt == t);
        let campfire = |i: &str| i == "minecraft:beef";
        let exists = |b: &str| {
            matches!(
                b,
                "minecraft:stone"
                    | "minecraft:oak_door"
                    | "minecraft:potted_poppy"
                    | "minecraft:poppy"
            )
        };
        let p = Clicker {
            main,
            off,
            sneaking,
            creative: false,
            may_build: true,
            food,
            item_tag: &tag,
            campfire_input: &campfire,
            block_exists: &exists,
        };
        use_item_on(&clicked(block, &prop), &p, Hand::Main)
    }

    fn env() -> Env {
        Env {
            props: Vec::new(),
            tags: Vec::new(),
        }
    }

    /// Terrain has no override: food in hand passes to `useItem` (eating
    /// while looking at the ground), a block item places.
    #[test]
    fn terrain_passes_food_and_ends_a_placement() {
        assert_eq!(
            run(
                "minecraft:grass_block",
                &env(),
                Some("minecraft:apple"),
                None,
                false,
                10
            ),
            Outcome::Pass
        );
        assert_eq!(
            run(
                "minecraft:grass_block",
                &env(),
                Some("minecraft:stone"),
                None,
                false,
                10
            ),
            Outcome::Consumed
        );
        assert_eq!(
            run(
                "minecraft:grass_block",
                &env(),
                Some("minecraft:bucket"),
                None,
                false,
                10
            ),
            Outcome::Pass
        );
        assert_eq!(
            run("minecraft:grass_block", &env(), None, None, false, 10),
            Outcome::Pass
        );
    }

    /// A container opens whatever is held, unless sneaking with something in
    /// a hand (then the item's own `useOn` answers).
    #[test]
    fn a_container_takes_the_click_unless_sneaking() {
        assert_eq!(
            run(
                "minecraft:chest",
                &env(),
                Some("minecraft:apple"),
                None,
                false,
                10
            ),
            Outcome::Consumed
        );
        assert_eq!(
            run(
                "minecraft:chest",
                &env(),
                Some("minecraft:apple"),
                None,
                true,
                10
            ),
            Outcome::Pass
        );
        assert_eq!(
            run("minecraft:chest", &env(), None, None, true, 10),
            Outcome::Consumed
        );
        assert_eq!(
            run(
                "minecraft:crafting_table",
                &env(),
                Some("minecraft:bow"),
                None,
                false,
                10
            ),
            Outcome::Consumed
        );
    }

    /// `canOpenByHand`: a wooden door opens, an iron one passes the click on.
    #[test]
    fn iron_doors_do_not_open_by_hand() {
        assert_eq!(
            run(
                "minecraft:oak_door",
                &env(),
                Some("minecraft:apple"),
                None,
                false,
                10
            ),
            Outcome::Consumed
        );
        assert_eq!(
            run(
                "minecraft:iron_door",
                &env(),
                Some("minecraft:apple"),
                None,
                false,
                10
            ),
            Outcome::Pass
        );
        assert_eq!(
            run("minecraft:iron_trapdoor", &env(), None, None, false, 10),
            Outcome::Pass
        );
        assert_eq!(
            run(
                "minecraft:oak_fence",
                &env(),
                Some("minecraft:apple"),
                None,
                false,
                10
            ),
            Outcome::Pass
        );
    }

    /// State-dependent: a jukebox takes a disc or gives one back, and is
    /// otherwise a pass; a cake is eaten only by a hungry player.
    #[test]
    fn state_decides_the_jukebox_and_the_cake() {
        let empty = env();
        let full = Env {
            props: vec![("has_record", "true")],
            tags: Vec::new(),
        };
        assert_eq!(
            run(
                "minecraft:jukebox",
                &empty,
                Some("minecraft:music_disc_cat"),
                None,
                false,
                10
            ),
            Outcome::Consumed
        );
        assert_eq!(
            run(
                "minecraft:jukebox",
                &empty,
                Some("minecraft:apple"),
                None,
                false,
                10
            ),
            Outcome::Pass
        );
        assert_eq!(
            run(
                "minecraft:jukebox",
                &full,
                Some("minecraft:apple"),
                None,
                false,
                10
            ),
            Outcome::Consumed
        );
        assert_eq!(
            run(
                "minecraft:cake",
                &empty,
                Some("minecraft:apple"),
                None,
                false,
                10
            ),
            Outcome::Consumed
        );
        assert_eq!(
            run(
                "minecraft:cake",
                &empty,
                Some("minecraft:apple"),
                None,
                false,
                20
            ),
            Outcome::Pass
        );
        assert_eq!(
            run("minecraft:cake", &empty, None, None, false, 20),
            Outcome::Consumed
        );
    }

    /// Item-dependent: a cauldron takes a filled bucket, a campfire its food,
    /// a flower pot its plant; a lectern without a book passes an empty
    /// hand and spends anything else (`useWithoutItem`'s CONSUME).
    #[test]
    fn the_held_item_decides_cauldrons_campfires_pots_and_lecterns() {
        let e = env();
        assert_eq!(
            run(
                "minecraft:cauldron",
                &e,
                Some("minecraft:water_bucket"),
                None,
                false,
                10
            ),
            Outcome::Consumed
        );
        assert_eq!(
            run(
                "minecraft:cauldron",
                &e,
                Some("minecraft:apple"),
                None,
                false,
                10
            ),
            Outcome::Pass
        );
        assert_eq!(
            run(
                "minecraft:campfire",
                &e,
                Some("minecraft:beef"),
                None,
                false,
                10
            ),
            Outcome::Consumed
        );
        assert_eq!(
            run(
                "minecraft:campfire",
                &e,
                Some("minecraft:apple"),
                None,
                false,
                10
            ),
            Outcome::Pass
        );
        assert_eq!(
            run(
                "minecraft:flower_pot",
                &e,
                Some("minecraft:poppy"),
                None,
                false,
                10
            ),
            Outcome::Consumed
        );
        assert_eq!(
            run(
                "minecraft:flower_pot",
                &e,
                Some("minecraft:apple"),
                None,
                false,
                10
            ),
            Outcome::Consumed
        );
        assert_eq!(
            run("minecraft:lectern", &e, None, None, false, 10),
            Outcome::Pass
        );
        assert_eq!(
            run(
                "minecraft:lectern",
                &e,
                Some("minecraft:apple"),
                None,
                false,
                10
            ),
            Outcome::Consumed
        );
    }

    /// The item side: an ender eye on an empty portal frame, a lily pad that
    /// places from `use` (so its `useOn` passes).
    #[test]
    fn item_use_on_rules() {
        let e = env();
        assert_eq!(
            run(
                "minecraft:end_portal_frame",
                &e,
                Some("minecraft:ender_eye"),
                None,
                false,
                10
            ),
            Outcome::Consumed
        );
        assert_eq!(
            run(
                "minecraft:stone",
                &e,
                Some("minecraft:ender_eye"),
                None,
                false,
                10
            ),
            Outcome::Pass
        );
        assert_eq!(
            run(
                "minecraft:stone",
                &e,
                Some("minecraft:lily_pad"),
                None,
                false,
                10
            ),
            Outcome::Pass
        );
        assert_eq!(
            run(
                "minecraft:stone",
                &e,
                Some("minecraft:diamond_hoe"),
                None,
                false,
                10
            ),
            Outcome::Consumed
        );
    }

    /// `use_item`: a class override is a success; food refuses a full
    /// player unless it is always edible; armour swaps; a sword passes.
    #[test]
    fn use_item_rules() {
        let tag = |_: &str, _: &str| false;
        let no = |_: &str| false;
        let p = |food| Clicker {
            main: None,
            off: None,
            sneaking: false,
            creative: false,
            may_build: true,
            food,
            item_tag: &tag,
            campfire_input: &no,
            block_exists: &no,
        };
        assert_eq!(
            use_item("minecraft:bow", &p(20), 72000, false),
            Outcome::Consumed
        );
        assert_eq!(
            use_item("minecraft:bucket", &p(20), 0, false),
            Outcome::Consumed
        );
        assert_eq!(
            use_item("minecraft:apple", &p(10), 32, false),
            Outcome::Consumed
        );
        assert_eq!(
            use_item("minecraft:apple", &p(20), 32, false),
            Outcome::Fail
        );
        assert_eq!(
            use_item("minecraft:golden_apple", &p(20), 32, false),
            Outcome::Consumed
        );
        assert_eq!(
            use_item("minecraft:potion", &p(20), 32, false),
            Outcome::Consumed
        );
        assert_eq!(
            use_item("minecraft:shield", &p(20), 72000, false),
            Outcome::Consumed
        );
        assert_eq!(
            use_item("minecraft:iron_helmet", &p(20), 0, true),
            Outcome::Consumed
        );
        assert_eq!(
            use_item("minecraft:diamond_sword", &p(20), 0, false),
            Outcome::Pass
        );
    }

    /// Every overriding class in this version was reviewed against its
    /// decompiled body (the rules above). A class a later version adds must
    /// fail here, not fall into a default arm unread.
    #[test]
    fn every_override_class_was_reviewed() {
        use rewo_data::click_use_table::{
            BLOCK_ITEM_ON_CLASSES, BLOCK_WITHOUT_ITEM_CLASSES, ITEM_USE_ON_CLASSES,
        };
        const ITEM_ON: &[&str] = &[
            "AbstractCauldronBlock",
            "BeehiveBlock",
            "CakeBlock",
            "CampfireBlock",
            "CandleBlock",
            "CandleCakeBlock",
            "CeilingHangingSignBlock",
            "ChiseledBookShelfBlock",
            "ComposterBlock",
            "CopperGolemStatueBlock",
            "DecoratedPotBlock",
            "FlowerPotBlock",
            "JukeboxBlock",
            "LecternBlock",
            "NoteBlock",
            "PumpkinBlock",
            "RedStoneOreBlock",
            "RespawnAnchorBlock",
            "ShelfBlock",
            "SignBlock",
            "SweetBerryBushBlock",
            "TntBlock",
            "WallHangingSignBlock",
            "WeatheringCopperGolemStatueBlock",
        ];
        const WITHOUT_ITEM: &[&str] = &[
            "AbstractFurnaceBlock",
            "AnvilBlock",
            "BarrelBlock",
            "BeaconBlock",
            "BedBlock",
            "BellBlock",
            "BrewingStandBlock",
            "ButtonBlock",
            "CakeBlock",
            "CandleCakeBlock",
            "CartographyTableBlock",
            "CaveVinesBlock",
            "CaveVinesPlantBlock",
            "ChestBlock",
            "ChiseledBookShelfBlock",
            "CommandBlock",
            "ComparatorBlock",
            "ComposterBlock",
            "CrafterBlock",
            "CraftingTableBlock",
            "DaylightDetectorBlock",
            "DecoratedPotBlock",
            "DispenserBlock",
            "DoorBlock",
            "DragonEggBlock",
            "EnchantingTableBlock",
            "EnderChestBlock",
            "FenceBlock",
            "FenceGateBlock",
            "FlowerPotBlock",
            "GrindstoneBlock",
            "HopperBlock",
            "JigsawBlock",
            "JukeboxBlock",
            "LecternBlock",
            "LeverBlock",
            "LightBlock",
            "LoomBlock",
            "MovingPistonBlock",
            "NoteBlock",
            "RedStoneWireBlock",
            "RepeaterBlock",
            "RespawnAnchorBlock",
            "ShulkerBoxBlock",
            "SignBlock",
            "SmithingTableBlock",
            "StonecutterBlock",
            "StructureBlock",
            "SweetBerryBushBlock",
            "TestBlock",
            "TestInstanceBlock",
            "TrapDoorBlock",
        ];
        const USE_ON: &[&str] = &[
            "ArmorStandItem",
            "AxeItem",
            "BlockItem",
            "BoneMealItem",
            "BrushItem",
            "CompassItem",
            "DebugStickItem",
            "EndCrystalItem",
            "EnderEyeItem",
            "FireChargeItem",
            "FireworkRocketItem",
            "FlintAndSteelItem",
            "HangingEntityItem",
            "HoeItem",
            "HoneycombItem",
            "LeadItem",
            "MapItem",
            "MinecartItem",
            "PlaceOnWaterBlockItem",
            "PotionItem",
            "ShearsItem",
            "ShovelItem",
            "SolidBucketItem",
            "SpawnEggItem",
        ];
        assert_eq!(
            BLOCK_ITEM_ON_CLASSES, ITEM_ON,
            "useItemOn overrides changed"
        );
        assert_eq!(
            BLOCK_WITHOUT_ITEM_CLASSES, WITHOUT_ITEM,
            "useWithoutItem overrides changed"
        );
        assert_eq!(ITEM_USE_ON_CLASSES, USE_ON, "useOn overrides changed");
    }

    /// The generated tables are sorted (the lookups binary-search them) and
    /// every class they name has a rule or is one of the always-consuming
    /// overrides.
    #[test]
    fn the_tables_are_sorted() {
        for t in [
            BLOCK_ITEM_ON_CLASS,
            BLOCK_WITHOUT_ITEM_CLASS,
            ITEM_CLICK_CLASS,
            ITEM_USE_CLASS,
        ] {
            assert!(
                t.windows(2).all(|w| w[0].0 < w[1].0),
                "unsorted or duplicate keys"
            );
        }
    }
}
