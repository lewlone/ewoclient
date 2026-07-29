//! Walking a `DataComponentPatch`'s values — the shape of every component
//! codec Rewo transcribes, and what it reads out of them (M41).
//!
//! # Why a shape table rather than a function per component
//!
//! 26.2 registers **111** data components, 104 of them network-synchronised,
//! and the patch encodes each one's value with that component's own stream
//! codec and **no length prefix**. So a component whose codec is not
//! transcribed cannot be skipped — the reader parks mid-value and every stack
//! after it in the packet is parsed out of garbage. That is why M19's decoder
//! knew three codecs and treated the rest as fatal.
//!
//! Nearly all 104 are built from a dozen primitives by the same handful of
//! combinators (`composite`, `list`, `map`, `optional`, `holder`), so the
//! codecs are written here as **data** — a [`Shape`] tree per component — and
//! one interpreter walks them. A new component is a table row, not a function,
//! and the coverage is a number the gate can read rather than a claim.
//!
//! # What is *not* here
//!
//! The interpreter walks a value and reports its **byte span**; it does not
//! build a typed model of every component. Only the handful the client
//! actually reads ([`ComponentValue`]) are interpreted. The rest are walked
//! for two reasons that matter on their own:
//!
//! 1. the reader stays aligned, so one enchanted sword no longer costs the
//!    whole rest of the packet, and
//! 2. the raw bytes give an **exact** answer to
//!    `ItemStack.isSameItemSameComponents`, which M35 could only approximate
//!    by "either side carries components at all".

use rewo_proto::nbt::Nbt;
use rewo_proto::reader::PacketReader;

/// A stream codec, as data.
///
/// The variants are `ByteBufCodecs`' own combinators. Anything expressible
/// here is walkable without a hand-written function.
#[derive(Clone, Copy, Debug)]
pub enum Shape {
    /// `Unit.STREAM_CODEC` — **zero bytes**. `unbreakable` is a marker, and
    /// reading even one byte for it desynchronises everything after.
    Unit,
    VarInt,
    /// `ByteBufCodecs.INT` — four bytes big-endian, *not* a var-int. The two
    /// are interchangeable for small positive numbers and not otherwise, and
    /// `dyed_color` is a packed RGB that is routinely negative.
    Int,
    Long,
    Float,
    Bool,
    Byte,
    /// `ByteBufCodecs.STRING_UTF8` — var-int length then bytes.
    Str,
    /// One network NBT tag. Every `fromCodec*` component reduces to this,
    /// which is what makes the chat components (`custom_name`, `item_name`,
    /// `lore`) walkable without transcribing their codecs at all.
    NbtTag,
    /// `ByteBufCodecs.holderRegistry` — a **raw** registry id.
    HolderRegistry,
    /// `ByteBufCodecs.holder` — `id + 1`, where **0 means an inline value**
    /// follows in the given shape. The off-by-one is the whole difference from
    /// [`Shape::HolderRegistry`], and reading one as the other shifts every id
    /// by one and then desynchronises on the first direct holder.
    Holder(&'static Shape),
    /// `ByteBufCodecs.holderSet` — a var-int, **and it is not a count**: the
    /// value written is `count + 1`, and a literal `0` means a *tag name*
    /// follows as a string instead of any entries at all. So `0` is one
    /// string, `1` is the empty set, and `n` is `n - 1` raw registry ids.
    HolderSet,
    /// `ByteBufCodecs.registry(...).dispatch(...)` — a var-int selecting one
    /// of several payload shapes.
    ///
    /// The selector is a **registry id**, so the order of the variants here is
    /// the registry's order and not the source file's. Getting it wrong reads
    /// the wrong payload and desynchronises, which is why each dispatch names
    /// the registry it indexes.
    Dispatch(&'static [Shape]),
    /// A 128-bit UUID, two big-endian longs.
    Uuid,
    /// Eight bytes — `ByteBufCodecs.DOUBLE`.
    Double,
    /// `ByteBufCodecs.optional` — a bool, then the value if true.
    Optional(&'static Shape),
    /// A var-int count then that many values.
    List(&'static Shape),
    /// A var-int count then that many key/value pairs.
    Map(&'static Shape, &'static Shape),
    /// `StreamCodec.composite` — fields in order.
    Tuple(&'static [Shape]),
    /// `ByteBufCodecs.either` — a bool selecting which of two shapes follows.
    /// **True is the *left*** alternative, which is the opposite of the
    /// intuition that a flag means "the special case".
    Either(&'static Shape, &'static Shape),
    /// `ItemStackTemplate.STREAM_CODEC` — item id, count, and a nested patch.
    /// Recursive, and bounded by the walk's depth limit.
    ItemStackTemplate,
    /// `TypedDataComponent.STREAM_CODEC` — a component type id, then that
    /// component's value under **its own** codec.
    ///
    /// This is the patch's own rule appearing a second time, in a place that is
    /// easy to miss: a `BlockPredicate` inside `can_place_on` carries a
    /// `DataComponentExactPredicate`, which is a list of these, so an adventure
    /// predicate can name *any* component at all. The consequence is the
    /// patch's too — an untranscribed component here has no length, so the walk
    /// stops rather than skipping. Recursive, and bounded by the depth limit.
    TypedComponent,
}

/// Shorthand for the shapes that appear inside others.
const S_STR: Shape = Shape::Str;
const S_VARINT: Shape = Shape::VarInt;
const S_INT: Shape = Shape::Int;
const S_FLOAT: Shape = Shape::Float;
const S_BOOL: Shape = Shape::Bool;
const S_NBT: Shape = Shape::NbtTag;
const S_HOLDER_REG: Shape = Shape::HolderRegistry;

/// `MaterialAssetGroup.STREAM_CODEC` — a base asset plus per-material
/// overrides.
const TRIM_ASSETS: Shape = Shape::Tuple(&[S_STR, Shape::Map(&S_STR, &S_STR)]);
/// `TrimMaterial.DIRECT_STREAM_CODEC`.
const TRIM_MATERIAL_DIRECT: Shape = Shape::Tuple(&[TRIM_ASSETS, S_NBT]);
/// `TrimPattern.DIRECT_STREAM_CODEC` — asset id, description, decal flag.
const TRIM_PATTERN_DIRECT: Shape = Shape::Tuple(&[S_STR, S_NBT, S_BOOL]);

/// `BannerPatternLayers.Layer.STREAM_CODEC` — a pattern holder and a dye.
const BANNER_LAYER: Shape = Shape::Tuple(&[
    Shape::Holder(&Shape::Tuple(&[S_STR, S_STR])),
    S_VARINT,
]);

/// `MobEffectInstance.STREAM_CODEC` — the effect, then its details, which
/// nest through `hiddenEffect`.
const MOB_EFFECT_DETAILS: Shape = Shape::Tuple(&[
    S_VARINT, // amplifier
    S_VARINT, // duration
    S_BOOL,   // ambient
    S_BOOL,   // showParticles
    S_BOOL,   // showIcon
    Shape::Optional(&Shape::NbtTag), // hiddenEffect, as a tag rather than a recursion
]);
const MOB_EFFECT_INSTANCE: Shape = Shape::Tuple(&[S_HOLDER_REG, MOB_EFFECT_DETAILS]);

/// `PotionContents.STREAM_CODEC`.
const POTION_CONTENTS: Shape = Shape::Tuple(&[
    Shape::Optional(&S_HOLDER_REG),          // potion
    Shape::Optional(&S_INT),                 // customColor
    Shape::List(&MOB_EFFECT_INSTANCE),       // customEffects
    Shape::Optional(&S_STR),                 // customName
]);

/// `CustomModelData.STREAM_CODEC` — four parallel lists.
const CUSTOM_MODEL_DATA: Shape = Shape::Tuple(&[
    Shape::List(&S_FLOAT),
    Shape::List(&S_BOOL),
    Shape::List(&S_STR),
    Shape::List(&S_INT),
]);

/// `SoundEvent.DIRECT_STREAM_CODEC` — a sound id and an optional fixed range.
const SOUND_DIRECT: Shape = Shape::Tuple(&[S_STR, Shape::Optional(&S_FLOAT)]);
/// `SoundEvent.STREAM_CODEC` — that behind a registry holder.
const SOUND: Shape = Shape::Holder(&SOUND_DIRECT);
const OPT_SOUND: Shape = Shape::Optional(&SOUND);

/// `Filterable.streamCodec(inner)` — the raw value, then an optional filtered
/// one. A book's pages are each two values, not one.
const FILTERABLE_STR: Shape = Shape::Tuple(&[S_STR, Shape::Optional(&S_STR)]);
const FILTERABLE_NBT: Shape = Shape::Tuple(&[S_NBT, Shape::Optional(&S_NBT)]);

/// `Tool.Rule.STREAM_CODEC`.
const TOOL_RULE: Shape = Shape::Tuple(&[
    Shape::HolderSet,
    Shape::Optional(&S_FLOAT),
    Shape::Optional(&S_BOOL),
]);

/// `FireworkExplosion.STREAM_CODEC` — shape id, two colour lists, two flags.
const FIREWORK_EXPLOSION: Shape = Shape::Tuple(&[
    S_VARINT,
    Shape::List(&S_INT),
    Shape::List(&S_INT),
    S_BOOL,
    S_BOOL,
]);

/// `GlobalPos.STREAM_CODEC` — a dimension key and a packed block position.
const GLOBAL_POS: Shape = Shape::Tuple(&[S_STR, Shape::Long]);

/// `ConsumeEffect.STREAM_CODEC`, dispatched on `minecraft:consume_effect_type`
/// in **registry id order**: apply_effects 0, remove_effects 1,
/// clear_all_effects 2, teleport_randomly 3, play_sound 4.
const CONSUME_EFFECT: Shape = Shape::Dispatch(&[
    Shape::Tuple(&[Shape::List(&MOB_EFFECT_INSTANCE), S_FLOAT]),
    Shape::HolderSet,
    Shape::Unit,
    Shape::Float,
    SOUND,
]);

/// `ResolvableProfile.STREAM_CODEC` — an unpacked profile and a skin patch.
///
/// `ByteBufCodecs.either` writes **true for the left**, which here is a full
/// `GameProfile` (uuid, name, and a property list); false is the partial form.
const GAME_PROFILE: Shape = Shape::Tuple(&[
    Shape::Uuid,
    S_STR,
    Shape::List(&Shape::Tuple(&[S_STR, S_STR, Shape::Optional(&S_STR)])),
]);
const PARTIAL_PROFILE: Shape = Shape::Tuple(&[
    Shape::Optional(&S_STR),
    Shape::Optional(&Shape::Uuid),
    Shape::List(&Shape::Tuple(&[S_STR, S_STR, Shape::Optional(&S_STR)])),
]);

/// `ItemAttributeModifiers.Entry.STREAM_CODEC` — attribute, modifier, slot
/// group, and a display dispatched on its own **id-mapped** type
/// (default 0, hidden 1, override 2), not a registry.
const ATTRIBUTE_ENTRY: Shape = Shape::Tuple(&[
    S_HOLDER_REG,
    // `AttributeModifier.STREAM_CODEC` — id, a **double**, and an operation.
    Shape::Tuple(&[S_STR, Shape::Double, S_VARINT]),
    S_VARINT, // EquipmentSlotGroup
    Shape::Dispatch(&[Shape::Unit, Shape::Unit, S_NBT]),
]);

/// `TypedEntityData.streamCodec(typeCodec)` — a registry id then a compound
/// tag. `ByteBufCodecs.COMPOUND_TAG` is `tagCodec` with a cast, so it is the
/// ordinary network tag [`Shape::NbtTag`] already reads.
const TYPED_ENTITY_DATA: Shape = Shape::Tuple(&[S_HOLDER_REG, S_NBT]);

/// `StatePropertiesPredicate.ValueMatcher.STREAM_CODEC` — `either(Exact,
/// Ranged)`, so **true is the plain string** and false is the min/max pair.
/// The two branches differ in length, which is the whole reason the flag has to
/// be read the right way round here.
const VALUE_MATCHER: Shape = Shape::Either(
    &S_STR,
    &Shape::Tuple(&[Shape::Optional(&S_STR), Shape::Optional(&S_STR)]),
);
/// `StatePropertiesPredicate.STREAM_CODEC` — a list of (name, matcher).
const STATE_PROPERTIES_PREDICATE: Shape = Shape::List(&Shape::Tuple(&[S_STR, VALUE_MATCHER]));

/// `DataComponentMatchers.STREAM_CODEC` — an exact half and a partial half.
///
/// The partial half looks like it needs the `data_component_predicate_type`
/// registry's dispatch table and does not: every `Type` builds its
/// `singleStreamCodec` as `ByteBufCodecs.fromCodecWithRegistries(codec)`, which
/// serialises through NBT — so whichever predicate type is selected, the
/// payload on the wire is **one tag**. The selector itself is
/// `either(registry(DATA_COMPONENT_PREDICATE_TYPE), registry(DATA_COMPONENT_TYPE))`,
/// two branches that are both a raw registry var-int, so this walks correctly
/// whichever way the flag reads.
const DATA_COMPONENT_MATCHERS: Shape = Shape::Tuple(&[
    // `DataComponentExactPredicate` — `TypedDataComponent.list()`.
    Shape::List(&Shape::TypedComponent),
    // `DataComponentPredicate` — `SINGLE_STREAM_CODEC.list(64)`.
    Shape::List(&Shape::Tuple(&[Shape::Either(&S_VARINT, &S_VARINT), S_NBT])),
]);

/// `BlockPredicate.STREAM_CODEC` (the *advancements* one — there are three
/// classes with this name in 26.2, and only `advancements.predicates` has a
/// stream codec).
const BLOCK_PREDICATE: Shape = Shape::Tuple(&[
    Shape::Optional(&Shape::HolderSet),           // blocks
    Shape::Optional(&STATE_PROPERTIES_PREDICATE), // properties
    Shape::Optional(&S_NBT),                      // nbt
    DATA_COMPONENT_MATCHERS,                      // components
]);

/// `Equippable.STREAM_CODEC`.
///
/// Eleven fields, five of them bare bools in a row — a miscount inside that run
/// is invisible in the shape and desynchronises by exactly as many bytes as it
/// is wrong by, so the order below is the record's, verbatim.
const EQUIPPABLE: Shape = Shape::Tuple(&[
    S_VARINT,                           // slot — `EquipmentSlot`, an idMapper
    SOUND,                              // equipSound
    Shape::Optional(&S_STR),            // assetId — a ResourceKey, i.e. an Identifier
    Shape::Optional(&S_STR),            // cameraOverlay
    Shape::Optional(&Shape::HolderSet), // allowedEntities
    S_BOOL,                             // dispensable
    S_BOOL,                             // swappable
    S_BOOL,                             // damageOnHurt
    S_BOOL,                             // equipOnInteract
    S_BOOL,                             // canBeSheared
    SOUND,                              // shearingSound
]);

/// `BlocksAttacks.DamageReduction.STREAM_CODEC`.
const DAMAGE_REDUCTION: Shape = Shape::Tuple(&[
    S_FLOAT,                            // horizontalBlockingAngle
    Shape::Optional(&Shape::HolderSet), // type
    S_FLOAT,                            // base
    S_FLOAT,                            // factor
]);
/// `BlocksAttacks.STREAM_CODEC`. `ItemDamageFunction` is inlined as its three
/// floats — it has a codec of its own but no optionality, so it adds no bytes
/// of its own to mark.
const BLOCKS_ATTACKS: Shape = Shape::Tuple(&[
    S_FLOAT,                                    // blockDelaySeconds
    S_FLOAT,                                    // disableCooldownScale
    Shape::List(&DAMAGE_REDUCTION),             // damageReductions
    Shape::Tuple(&[S_FLOAT, S_FLOAT, S_FLOAT]), // itemDamage
    Shape::Optional(&Shape::HolderSet),         // bypassedBy
    OPT_SOUND,                                  // blockSound
    OPT_SOUND,                                  // disableSound
]);

/// `KineticWeapon.Condition.STREAM_CODEC` — ticks then two speeds.
const KINETIC_CONDITION: Shape = Shape::Tuple(&[S_VARINT, S_FLOAT, S_FLOAT]);
/// `KineticWeapon.STREAM_CODEC` — three optional conditions in a row, each of
/// which is a bool that may or may not be followed by nine bytes.
const KINETIC_WEAPON: Shape = Shape::Tuple(&[
    S_VARINT,                            // contactCooldownTicks
    S_VARINT,                            // delayTicks
    Shape::Optional(&KINETIC_CONDITION), // dismountConditions
    Shape::Optional(&KINETIC_CONDITION), // knockbackConditions
    Shape::Optional(&KINETIC_CONDITION), // damageConditions
    S_FLOAT,                             // forwardMovement
    S_FLOAT,                             // damageMultiplier
    OPT_SOUND,                           // sound
    OPT_SOUND,                           // hitSound
]);

/// `JukeboxPlayable.STREAM_CODEC` is `JukeboxSong.STREAM_CODEC` with no wrapper
/// of its own, and that is `ByteBufCodecs.holder` — so a datapack song arrives
/// inline rather than as an id, and the description inside it is a chat
/// component, i.e. one tag.
const JUKEBOX_SONG: Shape = Shape::Holder(&Shape::Tuple(&[
    SOUND,    // soundEvent
    S_NBT,    // description
    S_FLOAT,  // lengthInSeconds
    S_VARINT, // comparatorOutput
]));

/// `BeehiveBlockEntity.Occupant.STREAM_CODEC`.
const BEE_OCCUPANT: Shape = Shape::Tuple(&[
    TYPED_ENTITY_DATA, // entityData
    S_VARINT,          // ticksInHive
    S_VARINT,          // minTicksInHive
]);

/// One row of the table: a component's registry name and the shape of its
/// value.
pub struct ComponentCodec {
    pub name: &'static str,
    pub shape: Shape,
}

const fn c(name: &'static str, shape: Shape) -> ComponentCodec {
    ComponentCodec { name, shape }
}

/// Every component codec Rewo transcribes.
///
/// Ordered by the group they came from rather than alphabetically, so a
/// missing sibling is visible. A component **absent from this table is not
/// walkable**, and reaching one is fatal to the enclosing packet — which is
/// the fail-closed behaviour, not a bug.
pub static CODECS: &[ComponentCodec] = &[
    // -- primitives ---------------------------------------------------------
    c("minecraft:max_stack_size", Shape::VarInt),
    c("minecraft:max_damage", Shape::VarInt),
    c("minecraft:damage", Shape::VarInt),
    c("minecraft:repair_cost", Shape::VarInt),
    c("minecraft:additional_trade_cost", Shape::VarInt),
    c("minecraft:ominous_bottle_amplifier", Shape::VarInt),
    c("minecraft:map_id", Shape::VarInt),
    c("minecraft:minimum_attack_charge", Shape::Float),
    c("minecraft:potion_duration_scale", Shape::Float),
    c("minecraft:enchantment_glint_override", Shape::Bool),
    // `Unit` is zero bytes — the component's presence *is* the value.
    c("minecraft:unbreakable", Shape::Unit),
    c("minecraft:creative_slot_lock", Shape::Unit),
    c("minecraft:glider", Shape::Unit),
    // -- identifiers --------------------------------------------------------
    c("minecraft:item_model", Shape::Str),
    c("minecraft:tooltip_style", Shape::Str),
    c("minecraft:note_block_sound", Shape::Str),
    // -- chat components, which are one NBT tag each ------------------------
    c("minecraft:custom_name", Shape::NbtTag),
    c("minecraft:item_name", Shape::NbtTag),
    c("minecraft:lore", Shape::List(&S_NBT)),
    // -- id-mapped enums, all a bare var-int --------------------------------
    c("minecraft:rarity", Shape::VarInt),
    c("minecraft:dye", Shape::VarInt),
    c("minecraft:base_color", Shape::VarInt),
    c("minecraft:map_post_processing", Shape::VarInt),
    c("minecraft:sheep/color", Shape::VarInt),
    c("minecraft:shulker/color", Shape::VarInt),
    c("minecraft:wolf/collar", Shape::VarInt),
    c("minecraft:cat/collar", Shape::VarInt),
    c("minecraft:tropical_fish/base_color", Shape::VarInt),
    c("minecraft:tropical_fish/pattern_color", Shape::VarInt),
    c("minecraft:tropical_fish/pattern", Shape::VarInt),
    c("minecraft:salmon/size", Shape::VarInt),
    c("minecraft:fox/variant", Shape::VarInt),
    c("minecraft:parrot/variant", Shape::VarInt),
    c("minecraft:mooshroom/variant", Shape::VarInt),
    c("minecraft:rabbit/variant", Shape::VarInt),
    c("minecraft:horse/variant", Shape::VarInt),
    c("minecraft:llama/variant", Shape::VarInt),
    c("minecraft:axolotl/variant", Shape::VarInt),
    c("minecraft:villager/variant", Shape::VarInt),
    // -- registry holders ---------------------------------------------------
    c("minecraft:damage_type", Shape::HolderRegistry),
    c("minecraft:wolf/variant", Shape::HolderRegistry),
    c("minecraft:wolf/sound_variant", Shape::HolderRegistry),
    c("minecraft:pig/variant", Shape::HolderRegistry),
    c("minecraft:pig/sound_variant", Shape::HolderRegistry),
    c("minecraft:cow/variant", Shape::HolderRegistry),
    c("minecraft:cow/sound_variant", Shape::HolderRegistry),
    c("minecraft:chicken/variant", Shape::HolderRegistry),
    c("minecraft:chicken/sound_variant", Shape::HolderRegistry),
    c("minecraft:cat/variant", Shape::HolderRegistry),
    c("minecraft:cat/sound_variant", Shape::HolderRegistry),
    c("minecraft:frog/variant", Shape::HolderRegistry),
    c("minecraft:painting/variant", Shape::HolderRegistry),
    c("minecraft:zombie_nautilus/variant", Shape::HolderRegistry),
    c("minecraft:provides_trim_material", Shape::HolderRegistry),
    // -- composites ---------------------------------------------------------
    // `map(Enchantment.STREAM_CODEC, VAR_INT)` — the key is a *raw* registry
    // id, so an enchanted item is a list of (id, level).
    c(
        "minecraft:enchantments",
        Shape::Map(&S_HOLDER_REG, &S_VARINT),
    ),
    c(
        "minecraft:stored_enchantments",
        Shape::Map(&S_HOLDER_REG, &S_VARINT),
    ),
    c("minecraft:dyed_color", Shape::Int),
    c("minecraft:map_color", Shape::Int),
    c("minecraft:custom_model_data", CUSTOM_MODEL_DATA),
    c(
        "minecraft:block_state",
        Shape::Map(&S_STR, &S_STR),
    ),
    c(
        "minecraft:trim",
        Shape::Tuple(&[
            Shape::Holder(&TRIM_MATERIAL_DIRECT),
            Shape::Holder(&TRIM_PATTERN_DIRECT),
        ]),
    ),
    c("minecraft:banner_patterns", Shape::List(&BANNER_LAYER)),
    c("minecraft:pot_decorations", Shape::List(&S_VARINT)),
    c("minecraft:potion_contents", POTION_CONTENTS),
    c(
        "minecraft:charged_projectiles",
        Shape::List(&Shape::ItemStackTemplate),
    ),
    c(
        "minecraft:bundle_contents",
        Shape::List(&Shape::ItemStackTemplate),
    ),
    c(
        "minecraft:container",
        Shape::List(&Shape::Optional(&Shape::ItemStackTemplate)),
    ),
    // `composite(type idMapper, VarInt duration)`.
    c("minecraft:swing_animation", Shape::Tuple(&[S_VARINT, S_VARINT])),
    // `composite(BOOL hideTooltip, collection of component type ids)`.
    c(
        "minecraft:tooltip_display",
        Shape::Tuple(&[S_BOOL, Shape::List(&S_VARINT)]),
    ),
    // -- M41b: the rest of what composes out of the shapes above ------------
    c("minecraft:enchantable", Shape::VarInt),
    c("minecraft:weapon", Shape::Tuple(&[S_VARINT, S_FLOAT])),
    c("minecraft:use_effects", Shape::Tuple(&[S_BOOL, S_BOOL, S_FLOAT])),
    c("minecraft:food", Shape::Tuple(&[S_VARINT, S_FLOAT, S_BOOL])),
    c(
        "minecraft:attack_range",
        Shape::Tuple(&[S_FLOAT, S_FLOAT, S_FLOAT, S_FLOAT, S_FLOAT, S_FLOAT]),
    ),
    c(
        "minecraft:use_cooldown",
        Shape::Tuple(&[S_FLOAT, Shape::Optional(&S_STR)]),
    ),
    c("minecraft:use_remainder", Shape::ItemStackTemplate),
    c("minecraft:sulfur_cube_content", Shape::ItemStackTemplate),
    c("minecraft:damage_resistant", Shape::HolderSet),
    c("minecraft:repairable", Shape::HolderSet),
    c("minecraft:provides_banner_patterns", Shape::HolderSet),
    c("minecraft:break_sound", SOUND),
    c(
        "minecraft:piercing_weapon",
        Shape::Tuple(&[S_BOOL, S_BOOL, OPT_SOUND, OPT_SOUND]),
    ),
    c(
        "minecraft:lodestone_tracker",
        Shape::Tuple(&[Shape::Optional(&GLOBAL_POS), S_BOOL]),
    ),
    c("minecraft:firework_explosion", FIREWORK_EXPLOSION),
    c(
        "minecraft:fireworks",
        Shape::Tuple(&[S_VARINT, Shape::List(&FIREWORK_EXPLOSION)]),
    ),
    c(
        "minecraft:suspicious_stew_effects",
        Shape::List(&Shape::Tuple(&[S_HOLDER_REG, S_VARINT])),
    ),
    c(
        "minecraft:writable_book_content",
        Shape::List(&FILTERABLE_STR),
    ),
    c(
        "minecraft:written_book_content",
        Shape::Tuple(&[
            FILTERABLE_STR,
            S_STR,
            S_VARINT,
            Shape::List(&FILTERABLE_NBT),
            S_BOOL,
        ]),
    ),
    c(
        "minecraft:tool",
        Shape::Tuple(&[Shape::List(&TOOL_RULE), S_FLOAT, S_VARINT, S_BOOL]),
    ),
    // `TypedEntityData.streamCodec(...)` — a type id then a compound tag.
    c("minecraft:entity_data", TYPED_ENTITY_DATA),
    c("minecraft:block_entity_data", TYPED_ENTITY_DATA),
    // `CustomData.STREAM_CODEC` is a bare compound tag.
    c("minecraft:bucket_entity_data", Shape::NbtTag),
    c(
        "minecraft:consumable",
        Shape::Tuple(&[
            S_FLOAT,
            S_VARINT, // ItemUseAnimation, an idMapper
            SOUND,
            S_BOOL,
            Shape::List(&CONSUME_EFFECT),
        ]),
    ),
    c("minecraft:death_protection", Shape::List(&CONSUME_EFFECT)),
    // `ResolvableProfile.STREAM_CODEC` = either(GameProfile, Partial) then a
    // `PlayerSkin.Patch` of three optional texture ids and an optional model
    // flag. A player head carries one of these.
    c(
        "minecraft:profile",
        Shape::Tuple(&[
            Shape::Either(&GAME_PROFILE, &PARTIAL_PROFILE),
            Shape::Tuple(&[
                Shape::Optional(&S_STR),
                Shape::Optional(&S_STR),
                Shape::Optional(&S_STR),
                Shape::Optional(&S_BOOL),
            ]),
        ]),
    ),
    c("minecraft:instrument", Shape::HolderRegistry),
    c(
        "minecraft:attribute_modifiers",
        Shape::List(&ATTRIBUTE_ENTRY),
    ),
    // -- M52e: the last seven network-synchronised codecs --------------------
    //
    // With these the table covers **every** component 26.2 registers with a
    // `.networkSynchronized(...)`. The fourteen names that were missing after
    // M41 split exactly in half: these seven, and seven that a server can never
    // send because they are `persistent`-only. That is a property the tests
    // below assert against the decompiled register list rather than a count
    // anyone has to keep in their head.
    //
    // `AdventureModePredicate.STREAM_CODEC` is `BlockPredicate.list()`, and the
    // predicate reaches back into this table through `Shape::TypedComponent` —
    // the only component whose codec is not a closed tree.
    c("minecraft:can_place_on", Shape::List(&BLOCK_PREDICATE)),
    c("minecraft:can_break", Shape::List(&BLOCK_PREDICATE)),
    c("minecraft:equippable", EQUIPPABLE),
    c("minecraft:blocks_attacks", BLOCKS_ATTACKS),
    c("minecraft:kinetic_weapon", KINETIC_WEAPON),
    c("minecraft:jukebox_playable", JUKEBOX_SONG),
    c("minecraft:bees", Shape::List(&BEE_OCCUPANT)),
];

/// A value the client actually reads out of a patch.
///
/// Everything else is walked and discarded — its bytes still contribute to the
/// [`fingerprint`](StackComponents::fingerprint), so two stacks differing only
/// in an uninterpreted component still compare unequal.
#[derive(Clone, Debug, PartialEq)]
pub enum ComponentValue {
    Damage(i32),
    MaxDamage(i32),
    /// A chat component reduced to its plain text (see [`nbt_text`]).
    Text(String),
    Lore(Vec<String>),
    Rarity(i32),
    /// `(enchantment registry id, level)` pairs.
    Enchantments(Vec<(i32, i32)>),
    /// A marker component, present with no value.
    Marker,
    /// Walked correctly; the value is not interpreted.
    Opaque,
}

/// Extract a chat component's plain text from its NBT form.
///
/// A `Component` serialises as a string for the literal case, or a compound
/// with `text` / `translate` plus an `extra` list of children. This is
/// deliberately *not* a text renderer: it concatenates the literal parts and
/// falls back to the translation key, because a tooltip that shows
/// `item.minecraft.diamond_sword` is wrong in a visible way, while inventing a
/// translation would be wrong in an invisible one.
pub fn nbt_text(tag: &Nbt) -> String {
    fn walk(tag: &Nbt, out: &mut String) {
        match tag {
            Nbt::String(s) => out.push_str(s),
            Nbt::Compound(_) => {
                if let Some(Nbt::String(s)) = tag.get("text") {
                    out.push_str(s);
                } else if let Some(Nbt::String(s)) = tag.get("translate") {
                    out.push_str(s);
                }
                if let Some(Nbt::List(children)) = tag.get("extra") {
                    for child in children {
                        walk(child, out);
                    }
                }
            }
            // A list at the top level is the legacy "siblings" form.
            Nbt::List(children) => {
                for child in children {
                    walk(child, out);
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    walk(tag, &mut out);
    out
}

/// The outcome of walking one value.
pub enum WalkOutcome {
    /// The value was consumed. Carries what was read, if it is interpreted.
    Walked(ComponentValue),
    /// The reader is parked mid-value; the enclosing packet is finished.
    Stuck,
}

/// How many levels of **recursion** a walk may take before it gives up.
///
/// A shulker box holds stacks that hold shulker boxes, and a `can_place_on`
/// predicate can match a block entity whose components include another
/// `can_place_on`. Vanilla bounds neither by its own rules; the wire does not
/// either, so a hostile server could nest until the stack overflowed. The limit
/// reports [`WalkOutcome::Stuck`], which is the same fail-closed answer an
/// unknown component gets.
///
/// **Only the recursive shapes count against it** — [`Shape::ItemStackTemplate`],
/// [`Shape::TypedComponent`] and [`walk_patch_opaque`]. The static combinators
/// deliberately pass `depth` through unchanged, because a [`Shape`] tree is a
/// `const` and so cannot be self-referential: its depth is fixed at compile
/// time and needs no runtime bound.
///
/// M41 charged every combinator, which was free only because nothing then was
/// deep enough to notice. `can_place_on` is: five combinators get you to the
/// [`Shape::TypedComponent`] inside its `DataComponentExactPredicate`, leaving
/// three for whatever component that names — and `profile`, `potion_contents`,
/// `written_book_content` and anything holding an `ItemStackTemplate` all need
/// more. Under the old accounting a **legitimate** adventure predicate reported
/// `Stuck`, which is not a safe default here: it is fail-*closed*, so it costs
/// the rest of the packet rather than one field.
pub const MAX_DEPTH: u32 = 8;

/// Walk one value of the given shape, interpreting nothing.
///
/// `Err(())` is a truncated body — distinct from [`WalkOutcome::Stuck`], which
/// means the bytes were there and the codec was not.
#[allow(clippy::result_unit_err)]
pub fn walk(r: &mut PacketReader, shape: &Shape, depth: u32) -> Result<bool, ()> {
    if depth > MAX_DEPTH {
        return Ok(false);
    }
    match shape {
        Shape::Unit => {}
        Shape::VarInt => {
            r.varint().map_err(|_| ())?;
        }
        Shape::Int => {
            r.i32().map_err(|_| ())?;
        }
        Shape::Long => {
            r.i64().map_err(|_| ())?;
        }
        Shape::Double => {
            r.i64().map_err(|_| ())?;
        }
        Shape::Float => {
            r.f32().map_err(|_| ())?;
        }
        Shape::Bool => {
            r.u8().map_err(|_| ())?;
        }
        Shape::Byte => {
            r.u8().map_err(|_| ())?;
        }
        Shape::Str => {
            // `STRING_UTF8`'s default cap is 32767 chars.
            r.string(32767).map_err(|_| ())?;
        }
        Shape::NbtTag => {
            Nbt::read_network(r).map_err(|_| ())?;
        }
        Shape::HolderRegistry => {
            r.varint().map_err(|_| ())?;
        }
        Shape::Uuid => {
            r.i64().map_err(|_| ())?;
            r.i64().map_err(|_| ())?;
        }
        Shape::Dispatch(variants) => {
            let which = r.varint().map_err(|_| ())?;
            let Some(v) = usize::try_from(which).ok().and_then(|i| variants.get(i)) else {
                // A selector outside the shapes transcribed here. Fail closed
                // rather than skipping: the payload's length is unknown.
                return Ok(false);
            };
            return walk(r, v, depth);
        }
        Shape::HolderSet => {
            // `VarInt.read(input) - 1`, and **zero is not an empty set** — it
            // is the tag form, whose name follows as a string.
            let n = r.varint().map_err(|_| ())?;
            if n == 0 {
                r.string(32767).map_err(|_| ())?;
            } else {
                if !(1..=65536).contains(&n) {
                    return Err(());
                }
                for _ in 0..n - 1 {
                    r.varint().map_err(|_| ())?;
                }
            }
        }
        Shape::Holder(inline) => {
            // `id + 1`, and **0 means the value follows inline**.
            if r.varint().map_err(|_| ())? == 0 {
                return walk(r, inline, depth);
            }
        }
        Shape::Optional(inner) => {
            if r.u8().map_err(|_| ())? != 0 {
                return walk(r, inner, depth);
            }
        }
        Shape::List(inner) => {
            let n = bounded_count(r)?;
            for _ in 0..n {
                if !walk(r, inner, depth)? {
                    return Ok(false);
                }
            }
        }
        Shape::Map(k, v) => {
            let n = bounded_count(r)?;
            for _ in 0..n {
                if !walk(r, k, depth)? || !walk(r, v, depth)? {
                    return Ok(false);
                }
            }
        }
        Shape::Tuple(fields) => {
            for f in *fields {
                if !walk(r, f, depth)? {
                    return Ok(false);
                }
            }
        }
        Shape::Either(left, right) => {
            // `ByteBufCodecs.either` writes **true for the left** alternative.
            let is_left = r.u8().map_err(|_| ())? != 0;
            return walk(r, if is_left { left } else { right }, depth);
        }
        Shape::TypedComponent => {
            // The patch's own rule: a type id, then that component's value
            // under its own codec. An id with no shape has no length either, so
            // this is the same fail-closed stop `walk_patch_opaque` takes.
            let ty = r.varint().map_err(|_| ())?;
            let Some(inner) = shape_for_id(ty) else {
                report_unwalkable(ty);
                return Ok(false);
            };
            return walk(r, inner, depth + 1);
        }
        Shape::ItemStackTemplate => return walk_item_template(r, depth),
    }
    Ok(true)
}

/// A collection length, rejected rather than trusted.
///
/// Every `ByteBufCodecs` collection codec carries a max size and throws past
/// it. The exact limits differ per component (256 for a container, 1024 for
/// projectiles); this uses one conservative ceiling, because the point is to
/// refuse a length that would make the walk allocate or spin, not to reproduce
/// each limit.
pub(crate) fn bounded_count(r: &mut PacketReader) -> Result<i32, ()> {
    let n = r.varint().map_err(|_| ())?;
    if !(0..=65536).contains(&n) {
        return Err(());
    }
    Ok(n)
}

/// One element of an `ItemStackTemplate` list, kept instead of walked past
/// (M61).
///
/// This is the *whole* of what the wire says about a nested stack that is not
/// itself another patch: `ItemStackTemplate` is `(Holder<Item>, int count,
/// DataComponentPatch)` and nothing else. The patch is reduced to
/// [`Self::patched`] rather than kept, because keeping it would mean deciding
/// how deep to keep it, and the caller that needs a bundle's grid needs the
/// count and nothing below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemTemplate {
    /// `Item.STREAM_CODEC` is `holderRegistry(ITEM)` — a **raw** registry id,
    /// not `holder`'s `id + 1`.
    pub item_id: i32,
    /// `ByteBufCodecs.VAR_INT`. A template's count is constructor-checked to
    /// be non-zero on the *encoding* side, but nothing on the wire enforces
    /// that, so this is reported exactly as sent.
    pub count: i32,
    /// Whether the element's own `DataComponentPatch` carried any entry,
    /// added or removed.
    ///
    /// The same one bit [`crate::item_stack::WireStack::patched`] is, and for
    /// the same reason: knowing *which* components a nested stack carries
    /// means interpreting them, and this reader deliberately walks them
    /// opaquely. It is enough to answer "does this element have components at
    /// all", which is what an exact-equality test needs — and it is **not**
    /// enough for `BundleContents.getWeight`, which asks whether the element
    /// is itself a bundle or holds bees. That answer needs the nested patch's
    /// contents and is not available here.
    pub patched: bool,
}

/// `ItemStackTemplate.STREAM_CODEC` — item, count, and a nested patch.
///
/// The capturing half of [`walk_item_template`]. Both go through this one
/// body, so a captured element and a walked one consume **the same bytes by
/// construction** rather than by two implementations agreeing. That matters
/// more here than anywhere else in this module: the patch has no length
/// prefix, so a capture path that read one byte differently would leave the
/// reader parked and garbage every stack after it in the packet.
///
/// `Ok(None)` is [`WalkOutcome::Stuck`] — the element's patch named a
/// component with no transcribed codec, or the depth limit was hit. The reader
/// is then parked mid-value and the enclosing packet is finished, exactly as
/// it would be for the walk.
#[allow(clippy::result_unit_err)]
pub fn read_item_template(r: &mut PacketReader, depth: u32) -> Result<Option<ItemTemplate>, ()> {
    let item_id = r.varint().map_err(|_| ())?;
    let count = r.varint().map_err(|_| ())?;
    Ok(
        walk_patch_counted(r, depth + 1)?.map(|(added, removed)| ItemTemplate {
            item_id,
            count,
            patched: added > 0 || removed > 0,
        }),
    )
}

/// `ByteBufCodecs.list()` of [`Shape::ItemStackTemplate`] — the shape of
/// `BundleContents.STREAM_CODEC` and of `container` minus its optionals.
///
/// Bounded by [`bounded_count`], which is the same ceiling [`Shape::List`]
/// applies, so a list this captures is exactly a list the generic walk would
/// have accepted. Vanilla's `list()` with no argument caps at `Integer.MAX_VALUE`;
/// trusting that would let one var-int ask for a two-billion-element `Vec`.
#[allow(clippy::result_unit_err)]
pub fn read_item_template_list(
    r: &mut PacketReader,
    depth: u32,
) -> Result<Option<Vec<ItemTemplate>>, ()> {
    let n = bounded_count(r)?;
    let mut out = Vec::new();
    for _ in 0..n {
        match read_item_template(r, depth)? {
            Some(item) => out.push(item),
            None => return Ok(None),
        }
    }
    Ok(Some(out))
}

fn walk_item_template(r: &mut PacketReader, depth: u32) -> Result<bool, ()> {
    Ok(read_item_template(r, depth)?.is_some())
}

/// Walk a nested `DataComponentPatch` without interpreting it.
///
/// The outer patch is walked by [`crate::item_stack`], which needs the values;
/// a patch inside a shulker box inside a stack does not, so it shares this
/// simpler path.
pub fn walk_patch_opaque(r: &mut PacketReader, depth: u32) -> Result<bool, ()> {
    Ok(walk_patch_counted(r, depth)?.is_some())
}

/// [`walk_patch_opaque`], reporting the `(added, removed)` entry counts.
///
/// Split out so [`read_item_template`] can say whether an element carried
/// components without walking the patch a second time — the two counts are the
/// patch's first two var-ints and are read on the way past regardless, so the
/// only thing that was ever missing was returning them.
///
/// `Ok(None)` is [`WalkOutcome::Stuck`]; it covers both the depth limit and an
/// entry with no transcribed codec, because a caller can do nothing different
/// about either.
///
/// **The guard below is belt-and-braces, not the bound.** Mutation-testing M61
/// found that deleting it changes no observable behaviour: every path out of a
/// patch entry goes through [`walk`], which applies the same limit one level
/// down, so `walk`'s guard alone bounds the recursion. It is kept because this
/// function is `pub` and a future direct caller would otherwise be relying on
/// a bound it cannot see — but a witness that only kills *this* copy is
/// testing nothing, which is why
/// `a_bundle_chain_captures_while_shallow_and_stops_once_past_the_limit` is
/// graded against [`MAX_DEPTH`]'s value rather than against the guard.
#[allow(clippy::result_unit_err)]
pub fn walk_patch_counted(r: &mut PacketReader, depth: u32) -> Result<Option<(i32, i32)>, ()> {
    walk_patch_with(r, depth, &mut |_, _| None)
}

/// The shared body of every patch walk, capturing or not (M63).
///
/// `capture` is offered each **added** entry's type id before `shape_for_id`
/// sees it. Returning `Some(result)` means the closure consumed that entry's
/// value itself; returning `None` leaves it to the generic walk.
///
/// One body rather than two, for the reason [`read_item_template`] gives: the
/// patch has no length prefix, so a capturing reader that consumed one byte
/// differently from the walking one would park the reader mid-value and turn
/// everything after it in the packet into garbage. Sharing the loop makes the
/// two agree **by construction** instead of by two implementations happening to
/// match — and a capture is then only correct if it reads exactly what that
/// component's [`Shape`] would, which is what
/// `a_named_container_slot_consumes_exactly_what_walking_it_does` grades.
#[allow(clippy::result_unit_err)]
pub fn walk_patch_with(
    r: &mut PacketReader,
    depth: u32,
    capture: &mut dyn FnMut(i32, &mut PacketReader) -> Option<Result<bool, ()>>,
) -> Result<Option<(i32, i32)>, ()> {
    if depth > MAX_DEPTH {
        return Ok(None);
    }
    let added = bounded_count(r)?;
    let removed = bounded_count(r)?;
    for _ in 0..added {
        let ty = r.varint().map_err(|_| ())?;
        if let Some(taken) = capture(ty, r) {
            if !taken? {
                return Ok(None);
            }
            continue;
        }
        match shape_for_id(ty) {
            Some(shape) => {
                if !walk(r, shape, depth + 1)? {
                    return Ok(None);
                }
            }
            None => return Ok(None),
        }
    }
    for _ in 0..removed {
        r.varint().map_err(|_| ())?;
    }
    Ok(Some((added, removed)))
}

/// One slot of `minecraft:container`, kept rather than walked past (M63).
///
/// `ItemContainerContents.STREAM_CODEC` is
/// `ItemStackTemplate.STREAM_CODEC.apply(ByteBufCodecs::optional).apply(list(256))`
/// — so a slot is an `Optional<ItemStackTemplate>`, and this is the present
/// case. It carries one field more than [`ItemTemplate`] because the tooltip
/// needs one thing a bundle's grid does not: `addToTooltip` renders
/// `item.container.item_count` from `itemStack.getHoverName()`, and a hover
/// name can only come from the *nested* patch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerSlot {
    /// `Item.STREAM_CODEC` is `holderRegistry(ITEM)` — a **raw** registry id.
    pub item_id: i32,
    pub count: i32,
    /// The nested `minecraft:custom_name`, reduced to plain text.
    ///
    /// **Only `custom_name`.** `getHoverName` reads `CUSTOM_NAME`, then
    /// `ITEM_NAME`, then the item's description id; the first is the only one
    /// a server ever patches onto a stack inside a container, and the other two
    /// are answered by the item table on the rendering side. A patch that
    /// *removes* `custom_name` leaves this `None`, which is the same answer as
    /// an absent one — and correct, because both send `getHoverName` on to the
    /// next fallback.
    pub custom_name: Option<String>,
}

/// One `Optional<ItemStackTemplate>`, keeping what a container tooltip needs.
///
/// The capturing counterpart of walking
/// `Shape::List(&Shape::Optional(&Shape::ItemStackTemplate))`, sharing its
/// patch loop through [`walk_patch_with`]. `Ok(None)` is
/// [`WalkOutcome::Stuck`], exactly as it is for [`read_item_template`].
#[allow(clippy::result_unit_err)]
pub fn read_container_slot(
    r: &mut PacketReader,
    depth: u32,
    custom_name_id: i32,
) -> Result<Option<ContainerSlot>, ()> {
    let item_id = r.varint().map_err(|_| ())?;
    let count = r.varint().map_err(|_| ())?;
    let mut custom_name = None;
    let walked = walk_patch_with(r, depth + 1, &mut |ty, r| {
        if ty != custom_name_id {
            return None;
        }
        // Exactly what `Shape::NbtTag` reads, which is what the generic walk
        // would have read for this entry. Anything else here desynchronises.
        Some(match Nbt::read_network(r) {
            Ok(tag) => {
                custom_name = Some(nbt_text(&tag));
                Ok(true)
            }
            Err(_) => Err(()),
        })
    })?;
    Ok(walked.map(|_| ContainerSlot {
        item_id,
        count,
        custom_name,
    }))
}

/// `ItemContainerContents.STREAM_CODEC` — a list of optional templates.
///
/// **Empty slots are kept as `None` rather than dropped.** Vanilla's own
/// `items` is a `List<Optional<…>>` whose indices are slot numbers, and
/// `copyInto` reads them positionally; the tooltip is the one consumer that
/// does not care, and it filters (`nonEmptyItemsStream`). Dropping them here
/// would throw away information the wire carried and cannot be recovered.
#[allow(clippy::result_unit_err)]
pub fn read_container_slot_list(
    r: &mut PacketReader,
    depth: u32,
    custom_name_id: i32,
) -> Result<Option<Vec<Option<ContainerSlot>>>, ()> {
    let n = bounded_count(r)?;
    let mut out = Vec::new();
    for _ in 0..n {
        // `ByteBufCodecs.optional` — a bool, then the value if true. The same
        // byte `Shape::Optional` reads.
        if r.u8().map_err(|_| ())? == 0 {
            out.push(None);
            continue;
        }
        match read_container_slot(r, depth, custom_name_id)? {
            Some(slot) => out.push(Some(slot)),
            None => return Ok(None),
        }
    }
    Ok(Some(out))
}

// The id → shape mapping is installed once, from the registry, because the
// table above is keyed by name and the wire by id. A thread-local rather than
// a parameter threaded through the recursion: the walk is a pure function of
// the bytes plus this table, and the table does not change within a session.
thread_local! {
    static SHAPES: std::cell::RefCell<Vec<Option<&'static Shape>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Install the id → shape table for this session's registry.
///
/// `ids` maps a component's registry name to its protocol id. Returns how many
/// of [`CODECS`] were resolved — a name the registry does not know is dropped
/// rather than panicking, so a version that renames a component loses that one
/// codec instead of the whole client.
pub fn install_shapes(ids: &std::collections::HashMap<String, i32>) -> usize {
    let mut max = 0;
    for row in CODECS {
        if let Some(&id) = ids.get(row.name) {
            max = max.max(id);
        }
    }
    let mut table: Vec<Option<&'static Shape>> = vec![None; (max + 1).max(0) as usize];
    let mut n = 0;
    for row in CODECS {
        if let Some(&id) = ids.get(row.name) {
            if id >= 0 {
                table[id as usize] = Some(&row.shape);
                n += 1;
            }
        }
    }
    SHAPES.with(|s| *s.borrow_mut() = table);
    n
}

/// The shape for a component's protocol id, or `None` if Rewo has no codec.
pub fn shape_for_id(id: i32) -> Option<&'static Shape> {
    if id < 0 {
        return None;
    }
    SHAPES.with(|s| s.borrow().get(id as usize).copied().flatten())
}

/// Record the component that stopped a walk, once per id.
///
/// A component with no codec costs the rest of its packet, and the failure is
/// otherwise silent — a stack simply never appears. Naming it turns "an item
/// is missing from my inventory" into "component 68 has no codec", which is
/// the difference between a bug report and a table row.
pub fn report_unwalkable(id: i32) {
    thread_local! {
        static SEEN: std::cell::RefCell<std::collections::HashSet<i32>> =
            std::cell::RefCell::new(std::collections::HashSet::new());
    }
    let first = SEEN.with(|s| s.borrow_mut().insert(id));
    if first {
        log::warn!(
            "rewo-net: data component id {id} has no transcribed codec — the stack \
             carrying it, and everything after it in that packet, is dropped"
        );
    }
}

/// Whether any shapes are installed. A session that never called
/// [`install_shapes`] would find every component unwalkable, which is a
/// configuration bug rather than a hostile server, so callers can check.
pub fn shapes_installed() -> usize {
    SHAPES.with(|s| s.borrow().iter().filter(|v| v.is_some()).count())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- wire writers -----------------------------------------------------
    //
    // Written here rather than reused from a producer, so a test asserts the
    // shape against bytes laid out from the decompiled codec by hand. A round
    // trip through a writer that shared this table's assumptions would agree
    // with it however wrong both were.

    fn varint(v: i32, out: &mut Vec<u8>) {
        let mut n = v as u32;
        loop {
            let b = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    fn string(s: &str, out: &mut Vec<u8>) {
        varint(s.len() as i32, out);
        out.extend_from_slice(s.as_bytes());
    }

    fn float(v: f32, out: &mut Vec<u8>) {
        out.extend_from_slice(&v.to_be_bytes());
    }

    fn bool(v: bool, out: &mut Vec<u8>) {
        out.push(v as u8);
    }

    /// The shortest valid network tag: an empty compound, `TAG_Compound` then
    /// the `TAG_End` that closes it. `tagCodec` rejects a bare `TAG_End`, so
    /// this is the floor rather than zero bytes.
    fn tag(out: &mut Vec<u8>) {
        out.extend_from_slice(&[0x0A, 0x00]);
    }

    fn none(out: &mut Vec<u8>) {
        out.push(0);
    }

    /// `Optional` present — the caller writes the value after it.
    fn some(out: &mut Vec<u8>) {
        out.push(1);
    }

    /// `holderSet` in its id-list form: **`count + 1`**, then the raw ids. A
    /// literal 0 would mean a tag name follows instead.
    fn holder_set(ids: &[i32], out: &mut Vec<u8>) {
        varint(ids.len() as i32 + 1, out);
        for id in ids {
            varint(*id, out);
        }
    }

    /// A registry holder by id — `id + 1`, so never 0.
    fn holder_id(id: i32, out: &mut Vec<u8>) {
        varint(id + 1, out);
    }

    /// A `SoundEvent` holder by id.
    fn sound_id(out: &mut Vec<u8>) {
        holder_id(12, out);
    }

    /// Walk `bytes` as `shape`, reporting how many bytes were consumed.
    ///
    /// `None` covers both failure modes on purpose: a truncated body and a
    /// [`WalkOutcome::Stuck`] are the same answer to the only question a caller
    /// has, which is whether the reader may keep going.
    fn walked(shape: &Shape, bytes: &[u8]) -> Option<usize> {
        let mut r = PacketReader::new(bytes);
        match walk(&mut r, shape, 0) {
            Ok(true) => Some(r.offset()),
            _ => None,
        }
    }

    /// The shape a component name maps to, straight out of [`CODECS`].
    fn shape(name: &str) -> &'static Shape {
        &CODECS
            .iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| panic!("no codec row for {name}"))
            .shape
    }

    /// Ids for the handful of components the nesting tests reach through
    /// [`Shape::TypedComponent`]. Arbitrary, because the wire's numbers are the
    /// server's — what matters is that the table is keyed by them.
    const DAMAGE_ID: i32 = 3;
    const PROFILE_ID: i32 = 71;
    const CAN_PLACE_ON_ID: i32 = 13;
    const BUNDLE_ID: i32 = 50;
    const CUSTOM_NAME_ID: i32 = 9;
    const CONTAINER_ID: i32 = 51;
    /// An id no registry can hold, so nothing can ever give it a shape. A real
    /// but merely-uncovered id would silently stop testing this property the
    /// day its codec landed — which is how three `item_stack` fixtures rotted
    /// in M41 and M43.
    const NO_SUCH_COMPONENT: i32 = i32::MAX;

    fn install_test_shapes() {
        let ids: std::collections::HashMap<String, i32> = [
            ("minecraft:damage", DAMAGE_ID),
            ("minecraft:profile", PROFILE_ID),
            ("minecraft:can_place_on", CAN_PLACE_ON_ID),
            ("minecraft:bundle_contents", BUNDLE_ID),
            ("minecraft:custom_name", CUSTOM_NAME_ID),
            ("minecraft:container", CONTAINER_ID),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        install_shapes(&ids);
    }

    // ---- coverage ---------------------------------------------------------

    /// The seven components `DataComponents` registers with **no**
    /// `.networkSynchronized(...)` at all: `persistent`-only, so a server has
    /// no way to put one in a patch and the walk can never meet one.
    ///
    /// They are listed rather than counted so that a version which starts
    /// syncing one of them fails here — as a missing codec, which is the
    /// diagnosable failure — instead of at a stack that quietly vanishes.
    const NEVER_SYNCHRONISED: &[&str] = &[
        "minecraft:custom_data",
        "minecraft:intangible_projectile",
        "minecraft:map_decorations",
        "minecraft:debug_stick_state",
        "minecraft:recipes",
        "minecraft:lock",
        "minecraft:container_loot",
    ];

    /// 26.2 registers 111 components; 104 of them carry a stream codec.
    #[test]
    fn the_table_covers_every_network_synchronised_component() {
        assert_eq!(CODECS.len(), 104);
        for name in NEVER_SYNCHRONISED {
            assert!(
                !CODECS.iter().any(|row| row.name == *name),
                "{name} has no stream codec — a shape for it could never be reached"
            );
        }
        assert_eq!(CODECS.len() + NEVER_SYNCHRONISED.len(), 111);
    }

    /// Two rows with the same name would let `install_shapes` pick either, and
    /// the loser would be a codec nobody could tell was dead.
    #[test]
    fn no_component_appears_in_the_table_twice() {
        let mut names: Vec<&str> = CODECS.iter().map(|row| row.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before);
    }

    // ---- can_place_on / can_break -----------------------------------------

    /// blocks / properties / nbt absent, both component halves empty.
    fn empty_block_predicate(out: &mut Vec<u8>) {
        none(out); // blocks
        none(out); // properties
        none(out); // nbt
        varint(0, out); // components.exact
        varint(0, out); // components.partial
    }

    #[test]
    fn an_adventure_predicate_walks_its_whole_body_and_no_more() {
        let mut b = Vec::new();
        varint(2, &mut b); // two predicates
        empty_block_predicate(&mut b);
        empty_block_predicate(&mut b);
        let n = b.len();
        b.push(0xEE); // a sentinel the walk must not reach
        assert_eq!(walked(shape("minecraft:can_place_on"), &b), Some(n));
        // `can_break` is the same codec, and shares the shape rather than
        // repeating it — so a fix to one cannot miss the other.
        assert_eq!(walked(shape("minecraft:can_break"), &b), Some(n));
    }

    #[test]
    fn a_fully_populated_block_predicate_walks_every_field() {
        install_test_shapes();
        let mut b = Vec::new();
        varint(1, &mut b);
        some(&mut b);
        holder_set(&[5, 9], &mut b); // blocks
        some(&mut b);
        varint(1, &mut b); // one property matcher
        string("facing", &mut b);
        bool(true, &mut b); // ExactMatcher — the LEFT branch
        string("north", &mut b);
        some(&mut b);
        tag(&mut b); // nbt
        varint(1, &mut b); // components.exact
        varint(DAMAGE_ID, &mut b);
        varint(7, &mut b); // damage, a bare var-int
        varint(1, &mut b); // components.partial
        bool(false, &mut b); // the DATA_COMPONENT_TYPE branch
        varint(DAMAGE_ID, &mut b);
        tag(&mut b);
        let n = b.len();
        b.push(0xEE);
        assert_eq!(walked(shape("minecraft:can_place_on"), &b), Some(n));
    }

    /// The two `ValueMatcher` branches are different lengths, so reading the
    /// flag backwards does not fail — it desynchronises. This pins the
    /// orientation by showing the same bytes consume differently each way.
    #[test]
    fn a_ranged_property_matcher_is_the_false_branch_of_the_either() {
        let ranged = |flag: bool| {
            let mut b = Vec::new();
            varint(1, &mut b);
            none(&mut b); // blocks
            some(&mut b);
            varint(1, &mut b);
            string("age", &mut b);
            bool(flag, &mut b);
            some(&mut b);
            string("2", &mut b); // min
            none(&mut b); // max
            none(&mut b); // nbt
            varint(0, &mut b);
            varint(0, &mut b);
            b
        };
        let right = ranged(false);
        assert_eq!(
            walked(shape("minecraft:can_place_on"), &right),
            Some(right.len())
        );
        // Flag flipped: the min/max pair is now read as one string, which stops
        // four bytes short and leaves the rest to be parsed as garbage.
        let wrong = ranged(true);
        let consumed = walked(shape("minecraft:can_place_on"), &wrong);
        assert_ne!(consumed, Some(wrong.len()));
    }

    /// A `DataComponentExactPredicate` can name any component, so it inherits
    /// the patch's fail-closed rule rather than skipping past an unknown one.
    #[test]
    fn an_unwalkable_component_inside_a_block_predicate_stops_the_walk() {
        install_test_shapes();
        let mut b = Vec::new();
        varint(1, &mut b);
        none(&mut b);
        none(&mut b);
        none(&mut b);
        varint(1, &mut b); // components.exact
        varint(NO_SUCH_COMPONENT, &mut b);
        b.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        varint(0, &mut b);
        assert_eq!(walked(shape("minecraft:can_place_on"), &b), None);
    }

    /// The reason [`MAX_DEPTH`] had to stop counting static shape nesting.
    ///
    /// The profile's **property list is deliberately non-empty**: an empty one
    /// never enters its element shape, so it stops one level short of the limit
    /// and this test passed under the old accounting too — which is how the
    /// first version of it failed to test anything at all.
    #[test]
    fn a_deeply_nested_component_inside_a_predicate_still_walks() {
        install_test_shapes();
        let mut b = Vec::new();
        varint(1, &mut b);
        none(&mut b);
        none(&mut b);
        none(&mut b);
        varint(1, &mut b); // components.exact
        varint(PROFILE_ID, &mut b);
        bool(true, &mut b); // either → the full GameProfile
        b.extend_from_slice(&[0u8; 16]); // uuid
        string("lewlone", &mut b);
        varint(1, &mut b); // properties
        string("textures", &mut b);
        string("eyJ0", &mut b);
        none(&mut b); // signature
        none(&mut b); // the four skin-patch optionals
        none(&mut b);
        none(&mut b);
        none(&mut b);
        varint(0, &mut b); // components.partial
        assert_eq!(walked(shape("minecraft:can_place_on"), &b), Some(b.len()));
    }

    /// ...and the budget still bites where it is meant to. `can_place_on` can
    /// name itself, which is unbounded recursion the wire does not limit.
    #[test]
    fn the_recursion_limit_still_stops_a_self_nesting_predicate() {
        install_test_shapes();
        fn nest(levels: u32) -> Vec<u8> {
            let mut b = Vec::new();
            varint(1, &mut b); // one predicate
            none(&mut b);
            none(&mut b);
            none(&mut b);
            if levels == 0 {
                varint(0, &mut b); // exact: stop here
            } else {
                varint(1, &mut b);
                varint(CAN_PLACE_ON_ID, &mut b);
                b.extend_from_slice(&nest(levels - 1));
            }
            varint(0, &mut b); // partial
            b
        }
        let shallow = nest(7);
        assert_eq!(
            walked(shape("minecraft:can_place_on"), &shallow),
            Some(shallow.len())
        );
        assert_eq!(walked(shape("minecraft:can_place_on"), &nest(40)), None);
    }

    // ---- the six flat records ---------------------------------------------

    #[test]
    fn equippable_walks_all_eleven_fields() {
        let mut b = Vec::new();
        varint(2, &mut b); // slot
        sound_id(&mut b); // equipSound
        some(&mut b);
        string("minecraft:iron", &mut b); // assetId
        none(&mut b); // cameraOverlay
        some(&mut b);
        holder_set(&[1, 2, 3], &mut b); // allowedEntities
        bool(true, &mut b); // dispensable
        bool(true, &mut b); // swappable
        bool(false, &mut b); // damageOnHurt
        bool(false, &mut b); // equipOnInteract
        bool(true, &mut b); // canBeSheared
        sound_id(&mut b); // shearingSound
        let n = b.len();
        b.push(0xEE);
        assert_eq!(walked(shape("minecraft:equippable"), &b), Some(n));
    }

    /// A sound holder with id 0 means the `SoundEvent` follows inline, and an
    /// armour piece whose equip sound is a datapack entry sends exactly that.
    #[test]
    fn an_inline_sound_holder_is_read_rather_than_treated_as_an_id() {
        let mut b = Vec::new();
        varint(0, &mut b); // slot
        varint(0, &mut b); // equipSound — 0 = inline
        string("ewo:velvet", &mut b);
        some(&mut b);
        float(16.0, &mut b); // fixedRange
        none(&mut b); // assetId
        none(&mut b); // cameraOverlay
        none(&mut b); // allowedEntities
        for _ in 0..5 {
            bool(true, &mut b);
        }
        sound_id(&mut b);
        assert_eq!(walked(shape("minecraft:equippable"), &b), Some(b.len()));
    }

    #[test]
    fn blocks_attacks_walks_its_reductions_and_both_optional_sounds() {
        let mut b = Vec::new();
        float(0.25, &mut b); // blockDelaySeconds
        float(1.0, &mut b); // disableCooldownScale
        varint(2, &mut b); // damageReductions
        float(90.0, &mut b);
        none(&mut b); // type
        float(0.0, &mut b);
        float(1.0, &mut b);
        float(45.0, &mut b);
        some(&mut b);
        holder_set(&[4], &mut b); // type
        float(2.0, &mut b);
        float(0.5, &mut b);
        float(1.0, &mut b); // itemDamage.threshold
        float(0.0, &mut b); // itemDamage.base
        float(1.0, &mut b); // itemDamage.factor
        some(&mut b);
        holder_set(&[7, 8], &mut b); // bypassedBy
        some(&mut b);
        sound_id(&mut b); // blockSound
        none(&mut b); // disableSound
        let n = b.len();
        b.push(0xEE);
        assert_eq!(walked(shape("minecraft:blocks_attacks"), &b), Some(n));
    }

    /// The three conditions are consecutive optionals over the same nine-byte
    /// body, so a miscount inside the run reads one condition's floats as the
    /// next one's presence flag.
    #[test]
    fn kinetic_weapon_walks_each_of_its_three_optional_conditions() {
        let condition = |out: &mut Vec<u8>| {
            some(out);
            varint(20, out);
            float(1.5, out);
            float(0.5, out);
        };
        for present in [[true, true, true], [false, true, false], [false; 3]] {
            let mut b = Vec::new();
            varint(10, &mut b); // contactCooldownTicks
            varint(0, &mut b); // delayTicks
            for p in present {
                if p {
                    condition(&mut b);
                } else {
                    none(&mut b);
                }
            }
            float(0.0, &mut b); // forwardMovement
            float(1.0, &mut b); // damageMultiplier
            none(&mut b); // sound
            some(&mut b);
            sound_id(&mut b); // hitSound
            let n = b.len();
            b.push(0xEE);
            assert_eq!(
                walked(shape("minecraft:kinetic_weapon"), &b),
                Some(n),
                "conditions {present:?}"
            );
        }
    }

    /// `JukeboxPlayable` has no wrapper of its own — it *is* the song holder —
    /// so a datapack song arrives inline, description tag and all.
    #[test]
    fn jukebox_playable_reads_an_inline_song_as_well_as_a_registry_id() {
        let mut by_id = Vec::new();
        holder_id(31, &mut by_id);
        assert_eq!(
            walked(shape("minecraft:jukebox_playable"), &by_id),
            Some(by_id.len())
        );

        let mut inline = Vec::new();
        varint(0, &mut inline); // 0 = the song follows
        sound_id(&mut inline); // soundEvent
        tag(&mut inline); // description, a chat component
        float(180.0, &mut inline); // lengthInSeconds
        varint(15, &mut inline); // comparatorOutput
        let n = inline.len();
        inline.push(0xEE);
        assert_eq!(walked(shape("minecraft:jukebox_playable"), &inline), Some(n));
    }

    #[test]
    fn bees_walks_one_occupant_per_entry() {
        let mut b = Vec::new();
        varint(3, &mut b);
        for ticks in [0, 40, 600] {
            holder_id(5, &mut b); // entityData.type — a raw registry id
            tag(&mut b); // entityData.tag
            varint(ticks, &mut b); // ticksInHive
            varint(600, &mut b); // minTicksInHive
        }
        let n = b.len();
        b.push(0xEE);
        assert_eq!(walked(shape("minecraft:bees"), &b), Some(n));
    }

    /// `TypedEntityData`'s type is `ByteBufCodecs.registry` — a **raw** id, not
    /// `holder`'s `id + 1`. Written raw, a bee's entity type of 0 is one byte
    /// and everything after it still lines up.
    #[test]
    fn a_bee_entity_type_is_a_raw_registry_id_not_an_offset_holder() {
        let mut b = Vec::new();
        varint(1, &mut b);
        varint(0, &mut b); // type id 0, written raw
        tag(&mut b);
        varint(0, &mut b);
        varint(600, &mut b);
        assert_eq!(walked(shape("minecraft:bees"), &b), Some(b.len()));
    }

    // ---- bundle_contents (M61) --------------------------------------------
    //
    // `BundleContents.STREAM_CODEC` is
    // `ItemStackTemplate.STREAM_CODEC.apply(ByteBufCodecs.list())`, and
    // `ItemStackTemplate.STREAM_CODEC` is
    // `composite(Item.STREAM_CODEC, VAR_INT count, DataComponentPatch.STREAM_CODEC)`.
    // So: a var-int count, then per element a raw item id, a var-int count and
    // a patch. Nothing else — `selectedItem` is not on the wire.

    /// One `ItemStackTemplate`, with a patch of `added` entries written as
    /// `(type id, value bytes)` and no removals.
    fn template(item: i32, count: i32, added: &[(i32, Vec<u8>)], out: &mut Vec<u8>) {
        varint(item, out); // Item.STREAM_CODEC — a RAW registry id
        varint(count, out);
        varint(added.len() as i32, out);
        varint(0, out); // removed
        for (ty, value) in added {
            varint(*ty, out);
            out.extend_from_slice(value);
        }
    }

    /// Capture `bytes` as a template list, reporting what it read and how far
    /// it got — the capturing counterpart of [`walked`].
    fn captured(bytes: &[u8]) -> Option<(Vec<ItemTemplate>, usize)> {
        let mut r = PacketReader::new(bytes);
        match read_item_template_list(&mut r, 0) {
            Ok(Some(items)) => Some((items, r.offset())),
            _ => None,
        }
    }

    /// The one property everything else in this module depends on: capturing a
    /// bundle must move the reader **exactly** as far as walking past it did.
    ///
    /// The sentinel makes both directions fail. Over-consumption reads past
    /// `n` and the offset no longer matches; under-consumption stops short and
    /// leaves the sentinel to be parsed as the next component's type id, which
    /// is precisely the desynchronisation the whole shape table exists to
    /// prevent.
    #[test]
    fn capturing_a_bundle_consumes_exactly_the_bytes_walking_it_does() {
        install_test_shapes();
        let mut b = Vec::new();
        varint(3, &mut b);
        template(1, 64, &[], &mut b);
        template(2, 1, &[(DAMAGE_ID, vec![7])], &mut b);
        template(999, 16, &[], &mut b);
        let n = b.len();
        b.push(0xEE); // a sentinel neither path may reach

        let (items, read) = captured(&b).expect("captures");
        assert_eq!(read, n, "capture consumed the wrong number of bytes");
        assert_eq!(walked(shape("minecraft:bundle_contents"), &b), Some(n));
        assert_eq!(
            items,
            vec![
                ItemTemplate { item_id: 1, count: 64, patched: false },
                ItemTemplate { item_id: 2, count: 1, patched: true },
                ItemTemplate { item_id: 999, count: 16, patched: false },
            ]
        );
    }

    /// An empty bundle is one zero byte, and the reader must stop on it.
    ///
    /// `Some(vec![])` rather than `None`: a list the server explicitly sent as
    /// empty is a different statement from a component it never mentioned, and
    /// vanilla draws the two differently (the empty-bundle blurb against no
    /// tooltip image at all).
    #[test]
    fn an_empty_bundle_is_a_zero_count_and_nothing_else() {
        install_test_shapes();
        let mut b = Vec::new();
        varint(0, &mut b);
        let n = b.len();
        b.push(0xEE);
        assert_eq!(captured(&b), Some((Vec::new(), n)));
        assert_eq!(walked(shape("minecraft:bundle_contents"), &b), Some(n));
    }

    /// An element's patch is a real `DataComponentPatch`, so its entries have
    /// to be walked by their own codecs. A reader that assumed elements were
    /// bare `(id, count)` pairs would stop three bytes early here and read the
    /// damage value as the next element's item id.
    #[test]
    fn a_bundle_element_carries_its_own_component_patch() {
        install_test_shapes();
        let mut b = Vec::new();
        varint(2, &mut b);
        // A sword with damage 300 — two var-int bytes, so a length this test
        // would notice being mistaken for one.
        template(2, 1, &[(DAMAGE_ID, vec![0xAC, 0x02])], &mut b);
        template(3, 5, &[], &mut b);
        let n = b.len();
        b.push(0xEE);
        let (items, read) = captured(&b).expect("captures");
        assert_eq!(read, n);
        assert!(items[0].patched, "the first element's patch was not seen");
        assert!(!items[1].patched);
        assert_eq!(walked(shape("minecraft:bundle_contents"), &b), Some(n));
    }

    /// A *removal*-only patch still counts as components. `getOrDefault` then
    /// answers with the type's default rather than the item's prototype value,
    /// so an element that removes a component is not the same stack as one
    /// that never had it — the same reason `read_patch_at` folds removals into
    /// its fingerprint.
    #[test]
    fn an_element_that_only_removes_a_component_is_still_patched() {
        install_test_shapes();
        let mut b = Vec::new();
        varint(1, &mut b);
        varint(2, &mut b); // item
        varint(1, &mut b); // count
        varint(0, &mut b); // added
        varint(1, &mut b); // removed
        varint(DAMAGE_ID, &mut b);
        let n = b.len();
        b.push(0xEE);
        let (items, read) = captured(&b).expect("captures");
        assert_eq!(read, n);
        assert!(items[0].patched);
    }

    /// A bundle can hold a bundle — `BundleContents.getWeight` has a
    /// `BUNDLE_IN_BUNDLE_WEIGHT` case precisely because it can.
    ///
    /// The inner one is **walked, not captured**: the outer bundle's grid
    /// shows one slot for it, and what is inside that slot is the inner
    /// bundle's own tooltip. So the capture is one level deep by design, and
    /// the inner list still has to be consumed byte-exactly by the generic
    /// walk or the outer capture would end in the wrong place.
    #[test]
    fn a_bundle_inside_a_bundle_is_walked_rather_than_captured() {
        install_test_shapes();
        let mut inner = Vec::new();
        varint(2, &mut inner);
        template(1, 32, &[], &mut inner);
        template(4, 8, &[], &mut inner);

        let mut b = Vec::new();
        varint(2, &mut b);
        template(7, 1, &[(BUNDLE_ID, inner)], &mut b);
        template(9, 3, &[], &mut b);
        let n = b.len();
        b.push(0xEE);

        let (items, read) = captured(&b).expect("captures");
        assert_eq!(read, n, "the nested list was not consumed exactly");
        assert_eq!(
            items,
            vec![
                ItemTemplate { item_id: 7, count: 1, patched: true },
                ItemTemplate { item_id: 9, count: 3, patched: false },
            ],
            "only the outer bundle's own stacks are captured"
        );
        assert_eq!(walked(shape("minecraft:bundle_contents"), &b), Some(n));
    }

    /// A bundle chain `levels` deep, innermost first: each level is a one-item
    /// bundle whose single stack carries a `bundle_contents` of the level
    /// below.
    fn nested_bundles(levels: usize) -> Vec<u8> {
        let mut payload = Vec::new();
        varint(0, &mut payload); // the innermost bundle, empty
        for _ in 0..levels {
            let mut next = Vec::new();
            varint(1, &mut next);
            template(1, 1, &[(BUNDLE_ID, payload)], &mut next);
            payload = next;
        }
        payload
    }

    /// The bound is on **recursion**, and a bundle chain is the cheapest way
    /// to spend it: each level costs one `ItemStackTemplate` and one patch.
    ///
    /// A chain past the limit reports [`WalkOutcome::Stuck`] rather than
    /// overflowing the stack — the same fail-closed answer an unknown
    /// component gets, and the reason a hostile server cannot crash the client
    /// with one well-formed component.
    ///
    /// **Both ends are asserted, and the deep one's `20` is a literal on
    /// purpose.** An earlier version sized the chain as `MAX_DEPTH + 2`, which
    /// made it self-calibrating: raising the bound raised the payload with it,
    /// so the test passed for a bound of 8 and of 64 alike and only ever
    /// witnessed "recursion terminates", not "recursion is bounded where this
    /// module says". A legitimate change to [`MAX_DEPTH`] should have to come
    /// back here.
    #[test]
    fn a_bundle_chain_captures_while_shallow_and_stops_once_past_the_limit() {
        install_test_shapes();
        let shallow = nested_bundles(2);
        assert!(
            captured(&shallow).is_some(),
            "an ordinary bundle-in-a-bundle must not hit the recursion bound"
        );
        let deep = nested_bundles(20);
        assert_eq!(captured(&deep), None);
        assert_eq!(walked(shape("minecraft:bundle_contents"), &deep), None);
    }

    /// An element's patch inherits the patch's own fail-closed rule: a
    /// component with no transcribed codec has no length either, so the reader
    /// stops there rather than guessing past it.
    #[test]
    fn an_unwalkable_component_inside_a_bundle_element_stops_the_capture() {
        install_test_shapes();
        let mut b = Vec::new();
        varint(1, &mut b);
        template(2, 1, &[(NO_SUCH_COMPONENT, vec![0xAA, 0xBB])], &mut b);
        assert_eq!(captured(&b), None);
        assert_eq!(walked(shape("minecraft:bundle_contents"), &b), None);
    }

    // ---- container (M63) ---------------------------------------------------
    //
    // `ItemContainerContents.STREAM_CODEC` is `ItemStackTemplate.STREAM_CODEC
    // .apply(ByteBufCodecs::optional).apply(ByteBufCodecs.list(256))` — so it
    // differs from a bundle by exactly one presence byte per slot, and by
    // keeping the one nested component `getHoverName` needs.

    /// A chat component in its network-NBT form: `{"text": s}`. The root is a
    /// bare type byte with **no name**, which is what `read_network` reads.
    fn text_tag(s: &str, out: &mut Vec<u8>) {
        out.push(0x0A); // TAG_Compound
        out.push(0x08); // TAG_String
        out.extend_from_slice(&4u16.to_be_bytes());
        out.extend_from_slice(b"text");
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s.as_bytes());
        out.push(0x00); // TAG_End
    }

    /// One slot: a presence bool, then a template when present.
    fn slot(item: Option<(i32, i32, &[(i32, Vec<u8>)])>, out: &mut Vec<u8>) {
        match item {
            None => none(out),
            Some((id, count, added)) => {
                some(out);
                template(id, count, added, out);
            }
        }
    }

    /// Capture `bytes` as a container list — the capturing counterpart of
    /// [`walked`], as [`captured`] is for bundles.
    fn captured_container(bytes: &[u8]) -> Option<(Vec<Option<ContainerSlot>>, usize)> {
        let mut r = PacketReader::new(bytes);
        match read_container_slot_list(&mut r, 0, CUSTOM_NAME_ID) {
            Ok(Some(slots)) => Some((slots, r.offset())),
            _ => None,
        }
    }

    /// **The alignment witness, and the only one that really matters here.**
    ///
    /// Capturing a container must move the reader *exactly* as far as walking
    /// past it did — including through a slot whose nested patch carries a
    /// `custom_name`, which is the one entry the capture reads itself instead
    /// of handing to `Shape::NbtTag`.
    ///
    /// Mutation partner: have the capture consume anything other than one
    /// network tag (a `Str`, say, or nothing at all) and the two offsets
    /// diverge — the sentinel is then either reached or left to be parsed as
    /// the next component's type id, which is the desynchronisation the whole
    /// shape table exists to prevent.
    #[test]
    fn a_named_container_slot_consumes_exactly_what_walking_it_does() {
        install_test_shapes();
        let mut name = Vec::new();
        text_tag("Bag of Holding", &mut name);

        let mut b = Vec::new();
        varint(3, &mut b);
        slot(Some((1, 64, &[])), &mut b);
        slot(Some((2, 1, &[(CUSTOM_NAME_ID, name)])), &mut b);
        slot(None, &mut b);
        let n = b.len();
        b.push(0xEE); // a sentinel neither path may reach

        let (slots, read) = captured_container(&b).expect("captures");
        assert_eq!(read, n, "capture consumed the wrong number of bytes");
        assert_eq!(
            walked(shape("minecraft:container"), &b),
            Some(n),
            "the generic walk disagrees with the capture"
        );
        assert_eq!(slots.len(), 3);
        assert_eq!(slots[1].as_ref().unwrap().custom_name.as_deref(), Some("Bag of Holding"));
    }

    /// An empty slot is one zero byte and **is kept**. Dropping it would
    /// renumber every slot after it, and `ItemContainerContents.items` is
    /// indexed by slot number.
    #[test]
    fn an_empty_container_slot_is_kept_rather_than_dropped() {
        install_test_shapes();
        let mut b = Vec::new();
        varint(4, &mut b);
        slot(None, &mut b);
        slot(Some((7, 3, &[])), &mut b);
        slot(None, &mut b);
        slot(Some((8, 1, &[])), &mut b);
        let n = b.len();
        b.push(0xEE);

        let (slots, read) = captured_container(&b).expect("captures");
        assert_eq!(read, n);
        assert_eq!(walked(shape("minecraft:container"), &b), Some(n));
        assert_eq!(slots.len(), 4, "the gaps were dropped");
        assert!(slots[0].is_none() && slots[2].is_none());
        assert_eq!(slots[1].as_ref().unwrap().item_id, 7);
        assert_eq!(slots[1].as_ref().unwrap().count, 3);
        // …and the tooltip's own view skips them, per `nonEmptyItemsStream`.
        assert_eq!(slots.iter().flatten().count(), 2);
    }

    /// A slot with no `custom_name` reports `None` rather than an empty string
    /// — `getHoverName` then falls through to `item_name` and the item's own
    /// description id, which an empty string would suppress.
    #[test]
    fn an_unnamed_container_slot_has_no_custom_name() {
        install_test_shapes();
        let mut b = Vec::new();
        varint(1, &mut b);
        slot(Some((5, 2, &[(DAMAGE_ID, vec![9])])), &mut b);
        let (slots, read) = captured_container(&b).expect("captures");
        assert_eq!(read, b.len());
        let s = slots[0].as_ref().unwrap();
        assert_eq!(s.custom_name, None);
        assert_eq!((s.item_id, s.count), (5, 2));
    }

    /// The nested patch inherits the patch's fail-closed rule: a component
    /// with no transcribed codec has no length either, so the capture stops
    /// rather than guessing past it — and the generic walk agrees.
    #[test]
    fn an_unwalkable_component_inside_a_container_slot_stops_the_capture() {
        install_test_shapes();
        let mut b = Vec::new();
        varint(1, &mut b);
        slot(Some((2, 1, &[(NO_SUCH_COMPONENT, vec![0xAA, 0xBB])])), &mut b);
        assert_eq!(captured_container(&b), None);
        assert_eq!(walked(shape("minecraft:container"), &b), None);
    }

    /// A truncated body is a different failure from a missing codec, and it
    /// must not be reported as a short bundle. Every prefix of a valid bundle
    /// either fails or reads fewer items — never the full list.
    #[test]
    fn a_truncated_bundle_never_reports_the_whole_list() {
        install_test_shapes();
        let mut b = Vec::new();
        varint(2, &mut b);
        template(1, 64, &[], &mut b);
        template(2, 1, &[], &mut b);
        for cut in 1..b.len() {
            let got = captured(&b[..cut]);
            assert!(
                got.is_none_or(|(items, _)| items.len() < 2),
                "a {cut}-byte prefix claimed the whole two-item bundle"
            );
        }
    }
}
