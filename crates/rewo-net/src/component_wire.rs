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
    c("minecraft:entity_data", Shape::Tuple(&[S_HOLDER_REG, S_NBT])),
    c(
        "minecraft:block_entity_data",
        Shape::Tuple(&[S_HOLDER_REG, S_NBT]),
    ),
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

/// How deep nested item templates may go before the walk gives up.
///
/// A shulker box holds stacks that hold shulker boxes. Vanilla bounds this by
/// its own rules; the wire does not, so a hostile server could nest until the
/// stack overflowed. The limit reports [`WalkOutcome::Stuck`], which is the
/// same fail-closed answer an unknown component gets.
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
            return walk(r, v, depth + 1);
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
                return walk(r, inline, depth + 1);
            }
        }
        Shape::Optional(inner) => {
            if r.u8().map_err(|_| ())? != 0 {
                return walk(r, inner, depth + 1);
            }
        }
        Shape::List(inner) => {
            let n = bounded_count(r)?;
            for _ in 0..n {
                if !walk(r, inner, depth + 1)? {
                    return Ok(false);
                }
            }
        }
        Shape::Map(k, v) => {
            let n = bounded_count(r)?;
            for _ in 0..n {
                if !walk(r, k, depth + 1)? || !walk(r, v, depth + 1)? {
                    return Ok(false);
                }
            }
        }
        Shape::Tuple(fields) => {
            for f in *fields {
                if !walk(r, f, depth + 1)? {
                    return Ok(false);
                }
            }
        }
        Shape::Either(left, right) => {
            // `ByteBufCodecs.either` writes **true for the left** alternative.
            let is_left = r.u8().map_err(|_| ())? != 0;
            return walk(r, if is_left { left } else { right }, depth + 1);
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
fn bounded_count(r: &mut PacketReader) -> Result<i32, ()> {
    let n = r.varint().map_err(|_| ())?;
    if !(0..=65536).contains(&n) {
        return Err(());
    }
    Ok(n)
}

/// `ItemStackTemplate.STREAM_CODEC` — item, count, and a nested patch.
fn walk_item_template(r: &mut PacketReader, depth: u32) -> Result<bool, ()> {
    r.varint().map_err(|_| ())?; // item id
    r.varint().map_err(|_| ())?; // count
    walk_patch_opaque(r, depth + 1)
}

/// Walk a nested `DataComponentPatch` without interpreting it.
///
/// The outer patch is walked by [`crate::item_stack`], which needs the values;
/// a patch inside a shulker box inside a stack does not, so it shares this
/// simpler path.
pub fn walk_patch_opaque(r: &mut PacketReader, depth: u32) -> Result<bool, ()> {
    if depth > MAX_DEPTH {
        return Ok(false);
    }
    let added = bounded_count(r)?;
    let removed = bounded_count(r)?;
    for _ in 0..added {
        let ty = r.varint().map_err(|_| ())?;
        match shape_for_id(ty) {
            Some(shape) => {
                if !walk(r, shape, depth + 1)? {
                    return Ok(false);
                }
            }
            None => return Ok(false),
        }
    }
    for _ in 0..removed {
        r.varint().map_err(|_| ())?;
    }
    Ok(true)
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
