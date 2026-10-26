//! `open_screen` and `container_set_data` — the two packets that tell the
//! client a container is open and what its progress bars say.
//!
//! Both were absent from `ids.rs` entirely, which is the whole reason Rewo can
//! show the player's own inventory and nothing else: without `open_screen`
//! there is no menu to route a non-zero `container_set_content` into.
//!
//! # `open_screen` — three fields, one of which has bitten this project thrice
//!
//! ```text
//! VarInt containerId    // ByteBufCodecs.CONTAINER_ID -> FriendlyByteBuf.readContainerId -> VarInt
//! VarInt menuType       // ByteBufCodecs.registry(Registries.MENU) -> RAW 0-based
//! Nbt    title          // ComponentSerialization.TRUSTED_STREAM_CODEC
//! ```
//!
//! **The menu type is `registry(...)`, not `holder(...)`** — a bare
//! `VarInt.read` into `byIdOrThrow`, with no `id + 1` and no `0 = inline`
//! branch. Reading it as a holder shifts every menu by one, and the shift is
//! *quiet*: id 2 (`generic_9x3`, a single chest) would read as id 1
//! (`generic_9x2`), which is a real menu with a real screen and 9 fewer slots,
//! so the packet decodes, a container opens, and the bottom row of a chest is
//! simply missing. M16 (dimension types), M21 (damage types) and M55
//! (attributes) each hit the same fork; the tell is `holderRegistry` /
//! `registry` in the codec versus `holder`.
//!
//! # `container_set_data` — a VarInt and then two *signed shorts*
//!
//! ```text
//! VarInt containerId
//! i16    id           // readShort
//! i16    value        // readShort
//! ```
//!
//! `readShort` in the middle of a var-int protocol, both of them, and both
//! **signed**. A var-int reading consumes the wrong number of bytes and
//! desynchronises the rest of the packet; an unsigned reading turns a
//! legitimately negative value into ~65,000. Negative values are real here —
//! `AnvilMenu` writes a cost, and `BeaconMenu`'s effect slots use a sentinel
//! for "no effect" — so this is not a theoretical distinction.
//!
//! # Where each one is dropped
//!
//! Vanilla gates both, and the two gates are *different*:
//!
//! * `handleContainerSetData` applies only when
//!   `player.containerMenu.containerId == packet.getContainerId()`. Data for a
//!   menu that is not the open one is discarded.
//! * `handleOpenScreen` calls `MenuScreens.create`, which on an unregistered
//!   type **logs a warning and does nothing at all** — it does not throw and it
//!   does not substitute. So an unknown menu type leaves whatever was open
//!   open, which is what `layout_of` returning `None` reproduces.

use rewo_proto::nbt::Nbt;
use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

/// `ClientboundOpenScreenPacket`.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenScreen {
    /// The id the server will address this menu by in `container_set_content`,
    /// `container_set_slot` and `container_set_data`. Never 0 for a real
    /// container — 0 is the player's own `InventoryMenu`, which is never
    /// opened by this packet.
    pub container_id: i32,
    /// The **raw** `minecraft:menu` registry id. Resolve with
    /// `rewo_world::menu_layout::layout_of`.
    pub menu_type: i32,
    /// The screen title, as one NBT `Component`.
    pub title: Nbt,
}

/// Decode `open_screen`.
pub fn read_open_screen(body: &[u8]) -> Result<OpenScreen> {
    let mut r = PacketReader::new(body);
    let container_id = r.varint()?;
    let menu_type = r.varint()?;
    let title = r.nbt()?;
    Ok(OpenScreen {
        container_id,
        menu_type,
        title,
    })
}

/// `ClientboundContainerSetDataPacket`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetData {
    pub container_id: i32,
    /// Which data slot. Its meaning is per-menu — `AbstractFurnaceMenu` uses
    /// 0..=3 for lit/litDuration/cookingProgress/cookingTotalTime, `BeaconMenu`
    /// 0..=2 for level and the two effects, and so on.
    pub id: i16,
    pub value: i16,
}

/// Decode `container_set_data`.
pub fn read_container_set_data(body: &[u8]) -> Result<SetData> {
    let mut r = PacketReader::new(body);
    let container_id = r.varint()?;
    let id = r.i16()?;
    let value = r.i16()?;
    Ok(SetData {
        container_id,
        id,
        value,
    })
}

/// `ClientboundMountScreenOpenPacket` (M220).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MountScreenOpen {
    pub container_id: i32,
    /// `getInventoryColumns()` — 0 for a horse, 5 for a chested donkey or
    /// mule, a llama's strength.
    pub columns: i32,
    pub entity_id: i32,
}

/// Decode `mount_screen_open`: `readContainerId` (a VarInt), `readVarInt`,
/// then **`readInt`** — the entity id is a fixed four-byte int here, not the
/// VarInt every other entity id on the wire is.
pub fn read_mount_screen_open(body: &[u8]) -> Result<MountScreenOpen> {
    let mut r = PacketReader::new(body);
    let container_id = r.varint()?;
    let columns = r.varint()?;
    let entity_id = r.i32()?;
    Ok(MountScreenOpen {
        container_id,
        columns,
        entity_id,
    })
}

/// What the mount is, as `handleMountScreenOpen` and the two menus read it.
#[derive(Debug, Clone, Copy)]
pub struct MountFacts {
    /// `instanceof AbstractHorse`.
    pub horse: bool,
    /// `instanceof AbstractNautilus`.
    pub nautilus: bool,
    /// `instanceof Llama` (a llama or trader llama).
    pub llama: bool,
    pub baby: bool,
    /// `is(EntityTypeTags.CAN_EQUIP_SADDLE)`.
    pub can_equip_saddle: bool,
    /// `is(EntityTypeTags.CAN_WEAR_HORSE_ARMOR)`.
    pub can_wear_horse_armor: bool,
}

/// `handleMountScreenOpen`'s branch and the two `ArmorSlot.isActive`
/// overrides (M220): `(kind, saddle_active, body_active)`, or `None` for an
/// entity that is neither (vanilla then does nothing).
///
/// - Horse saddle: `canUseSlot(SADDLE) && is(CAN_EQUIP_SADDLE)`, where
///   `AbstractHorse.canUseSlot(SADDLE)` is `isAlive() && !isBaby() &&
///   isTamed()`.
/// - Horse body: `canUseSlot(BODY)` (`LivingEntity`'s `true`) `&&
///   (is(CAN_WEAR_HORSE_ARMOR) || this instanceof Llama)`.
/// - Nautilus, both: `isAlive() && !isBaby() && isTame()`.
///
/// Alive and tamed are taken as given: the server opens these menus only for
/// a tamed, living mount (`openCustomInventoryScreen`'s guard).
pub fn resolve_mount(f: MountFacts) -> Option<(rewo_world::menu_layout::MountKind, bool, bool)> {
    use rewo_world::menu_layout::MountKind;
    if f.horse {
        Some((
            MountKind::Horse,
            !f.baby && f.can_equip_saddle,
            f.can_wear_horse_armor || f.llama,
        ))
    } else if f.nautilus {
        Some((MountKind::Nautilus, !f.baby, !f.baby))
    } else {
        None
    }
}

/// The open mount's side of `LivingEntity.isEquippableInSlot` (M220), as an
/// owned snapshot the click path's item-property closure can hold while the
/// menu is borrowed mutably.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MountFit {
    /// The mount's `minecraft:entity_type` id.
    pub type_name: String,
    /// Every `entity_type` tag the server says contains it.
    pub type_tags: Vec<String>,
    /// `canUseSlot(SADDLE)` / `canUseSlot(BODY)`.
    pub saddle_usable: bool,
    pub body_usable: bool,
}

impl MountFit {
    /// `slot == equippable.slot() && canUseSlot(slot) &&
    /// equippable.canBeEquippedBy(type)` for an item whose equippable slot is
    /// the saddle (`saddle`) or the body, with `allowed` as
    /// `rewo_data::item_props_table::equip_allowed` spells it (`None` = any
    /// entity).
    pub fn accepts(&self, saddle: bool, allowed: Option<&str>) -> bool {
        let usable = if saddle {
            self.saddle_usable
        } else {
            self.body_usable
        };
        usable
            && match allowed {
                None => true,
                Some(a) => match a.strip_prefix('#') {
                    Some(tag) => self.type_tags.iter().any(|t| t == tag),
                    None => a.split('|').any(|n| n == self.type_name),
                },
            }
    }
}

/// The two packet ids this module owns.
///
/// A local struct rather than the full [`crate::ids::Ids`] for the same reason
/// `BorderIds` and `ClientStateIds` are: it makes the fan-out testable without
/// a datagen report. M71's finding is the reason that matters — `PlaySession`'s
/// dispatch was entirely unwitnessed, so deleting a whole branch survived the
/// suite. The routing below is a tested function; `crate::route_menu` is a
/// six-line adapter over it.
#[derive(Debug, Clone, Copy)]
pub struct MenuIds {
    pub open_screen: i32,
    pub container_set_data: i32,
}

/// Route one `(packet id, body)` to the menu state.
///
/// Returns whether the id matched — **not** whether anything was applied.
pub fn route(ids: MenuIds, id: i32, body: &[u8], menus: &mut rewo_world::menu::Menus) -> bool {
    if id == ids.open_screen {
        match read_open_screen(body) {
            Ok(p) => {
                // `title` stays the unresolved flatten (a log line, a witness);
                // the screen draws `title_component`, resolved against the
                // language table at draw time.
                let title = crate::hud_state::plain(&p.title);
                if menus.apply_open_screen(p.container_id, p.menu_type, title) {
                    if let Some(open) = menus.open_mut() {
                        open.title_component = Some(p.title);
                    }
                } else {
                    // `MenuScreens.create` logs a warning and does nothing for
                    // an unregistered type; same here, and at the same volume.
                    log::warn!(
                        "open_screen: no layout for menu type {} (container {})",
                        p.menu_type,
                        p.container_id
                    );
                }
            }
            Err(e) => log::warn!("open_screen: malformed body: {e:?}"),
        }
        return true;
    }
    if id == ids.container_set_data {
        match read_container_set_data(body) {
            Ok(p) => {
                menus.apply_set_data(p.container_id, p.id, p.value);
            }
            Err(e) => log::warn!("container_set_data: malformed body: {e:?}"),
        }
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an `open_screen` body: varint id, varint type, then a minimal
    /// NBT string component.
    fn open_screen_body(container_id: u8, menu_type: u8) -> Vec<u8> {
        let mut v = vec![container_id, menu_type];
        // Network NBT: a bare TAG_String (8) payload with no name.
        v.push(8);
        v.extend_from_slice(&(5u16).to_be_bytes());
        v.extend_from_slice(b"Chest");
        v
    }

    #[test]
    fn open_screen_reads_id_type_and_title() {
        let got = read_open_screen(&open_screen_body(3, 2)).unwrap();
        assert_eq!(got.container_id, 3);
        assert_eq!(got.menu_type, 2);
    }

    #[test]
    fn the_menu_type_is_a_raw_registry_id_not_a_holder() {
        // The whole point of this witness: byte 2 is the menu id verbatim.
        // A `holder` reading would treat it as `id + 1` and answer 1
        // (generic_9x2) for a generic_9x3 chest -- a real menu, with a real
        // screen, 9 slots short. So the mutation is not observable as an
        // error, only as a wrong menu, and this is the only place it can be
        // caught.
        for raw in 0..25u8 {
            let got = read_open_screen(&open_screen_body(1, raw)).unwrap();
            assert_eq!(
                got.menu_type, raw as i32,
                "menu type must be the raw byte, not raw - 1"
            );
        }
    }

    #[test]
    fn set_data_reads_two_signed_shorts_after_a_varint() {
        // container 5, id 1, value -3.
        let body = [5u8, 0x00, 0x01, 0xFF, 0xFD];
        let got = read_container_set_data(&body).unwrap();
        assert_eq!(
            got,
            SetData {
                container_id: 5,
                id: 1,
                value: -3
            }
        );
    }

    #[test]
    fn a_negative_data_value_stays_negative() {
        // An unsigned reading gives 65533 here. Negative values are real:
        // beacon effect slots carry a "none" sentinel and the anvil a cost.
        let body = [1u8, 0x00, 0x02, 0xFF, 0xFF];
        assert_eq!(read_container_set_data(&body).unwrap().value, -1);
    }

    #[test]
    fn set_data_is_five_bytes_for_a_small_container_id() {
        // Pins the shorts as fixed-width: a var-int encoding of the same
        // values would be shorter, and a reader expecting var-ints would
        // consume 3 bytes here and leave 2 behind.
        let body = [1u8, 0x00, 0x01, 0x00, 0x01];
        assert!(read_container_set_data(&body).is_ok());
        let truncated = [1u8, 0x00, 0x01, 0x00];
        assert!(
            read_container_set_data(&truncated).is_err(),
            "four bytes cannot satisfy a varint plus two shorts"
        );
    }

    const IDS: MenuIds = MenuIds {
        open_screen: 59,
        container_set_data: 19,
    };

    #[test]
    fn the_opened_menu_keeps_the_title_component_for_the_screen_to_resolve() {
        let mut m = rewo_world::menu::Menus::new();
        assert!(route(IDS, 59, &open_screen_body(3, 2), &mut m));
        let open = m.open().unwrap();
        assert_eq!(
            open.title_component,
            Some(rewo_proto::nbt::Nbt::String("Chest".into()))
        );
    }

    #[test]
    fn routing_open_screen_opens_the_menu() {
        let mut m = rewo_world::menu::Menus::new();
        assert!(route(IDS, 59, &open_screen_body(3, 2), &mut m));
        assert_eq!(m.open().unwrap().layout.name, "generic_9x3");
        assert_eq!(m.open().unwrap().container_id, 3);
    }

    #[test]
    fn routing_set_data_reaches_the_open_menu() {
        let mut m = rewo_world::menu::Menus::new();
        route(IDS, 59, &open_screen_body(4, 14), &mut m);
        // container 4, data slot 2, value 137.
        assert!(route(IDS, 19, &[4, 0x00, 0x02, 0x00, 0x89], &mut m));
        assert_eq!(m.open().unwrap().data(2), 137);
    }

    #[test]
    fn an_unrelated_id_is_not_claimed_and_changes_nothing() {
        // The branch-deletion witness M71's finding asks for: if either arm is
        // removed, the matching test above fails; if an arm over-claims, this
        // one does.
        let mut m = rewo_world::menu::Menus::new();
        assert!(!route(IDS, 58, &open_screen_body(1, 0), &mut m));
        assert!(!route(IDS, 20, &[1, 0, 0, 0, 0], &mut m));
        assert!(m.open().is_none());
    }

    #[test]
    fn a_malformed_body_is_claimed_but_applies_nothing() {
        // The id matched, so the chain must not fall through to another seam;
        // but a body that cannot be decoded leaves the state alone rather than
        // opening a half-read menu.
        let mut m = rewo_world::menu::Menus::new();
        assert!(route(IDS, 59, &[], &mut m), "the id still matched");
        assert!(m.open().is_none(), "but nothing opened");
    }

    // --- container_target: the routing rule `handleContainerContent` states ---

    /// A `container_set_content` body's first field is the container id; the
    /// rest is irrelevant to routing, which is the point.
    fn body_for(container_id: u8) -> Vec<u8> {
        vec![container_id, 0, 0]
    }

    #[test]
    fn id_zero_routes_to_the_player_even_with_a_container_open() {
        // The inversion: it is not "the open menu, defaulting to the
        // inventory". inventoryMenu and containerMenu are separate objects and
        // id 0 always means the former. Routing it to the open chest would
        // write the chest's slots with the player's items.
        //
        // HONESTY NOTE, on M59's terms: no single-point mutation breaks this
        // one, and it was mutation-tested to find that out. Two independent
        // guards enforce it -- `container_target` tests id 0 first, and
        // `menu_for` gates on equality with an id that is never 0 -- so
        // inverting the priority leaves it passing (the non-matching-id
        // witness is what catches that), and opening `menu_for` to any id
        // leaves it passing too. It takes both at once. The witness is kept as
        // a statement of the rule rather than deleted, because the realistic
        // regression is someone rewriting this function wholesale as "the open
        // menu, else the player", and then this is the line that says what the
        // answer must be.
        let mut player = rewo_world::inventory::Inventory::default();
        let mut menus = rewo_world::menu::Menus::new();
        menus.apply_open_screen(3, 5, "Double Chest".into());
        let (id, target) = crate::container_target(&body_for(0), &mut player, &mut menus).unwrap();
        assert_eq!(id, 0);
        assert_eq!(
            target.slot_count(),
            rewo_world::inventory::MENU_SLOTS,
            "id 0 must reach the 46-slot player menu, not the 90-slot chest"
        );
    }

    #[test]
    fn a_matching_nonzero_id_routes_to_the_container() {
        // The whole point of M87d: before it, this returned None and every
        // chest was invisible.
        let mut player = rewo_world::inventory::Inventory::default();
        let mut menus = rewo_world::menu::Menus::new();
        menus.apply_open_screen(3, 5, "Double Chest".into());
        let (id, target) = crate::container_target(&body_for(3), &mut player, &mut menus).unwrap();
        assert_eq!(id, 3);
        assert_eq!(target.slot_count(), 54 + 36);
    }

    #[test]
    fn a_nonmatching_nonzero_id_routes_nowhere() {
        let mut player = rewo_world::inventory::Inventory::default();
        let mut menus = rewo_world::menu::Menus::new();
        menus.apply_open_screen(3, 5, "Double Chest".into());
        assert!(crate::container_target(&body_for(4), &mut player, &mut menus).is_none());
        // ...and with nothing open at all.
        let mut empty = rewo_world::menu::Menus::new();
        assert!(crate::container_target(&body_for(3), &mut player, &mut empty).is_none());
    }

    #[test]
    fn an_empty_body_routes_nowhere_rather_than_defaulting_to_the_player() {
        let mut player = rewo_world::inventory::Inventory::default();
        let mut menus = rewo_world::menu::Menus::new();
        assert!(crate::container_target(&[], &mut player, &mut menus).is_none());
    }

    #[test]
    fn a_truncated_open_screen_is_an_error_not_a_default() {
        assert!(read_open_screen(&[]).is_err());
        assert!(read_open_screen(&[1]).is_err());
        // id and type but no title tag.
        assert!(read_open_screen(&[1, 2]).is_err());
    }

    /// M220 — the entity id is `readInt`, four big-endian bytes, after two
    /// VarInts. A VarInt reading of it would take one byte and leave three.
    #[test]
    fn mount_screen_open_reads_a_fixed_int_entity_id() {
        let mut body = vec![7u8, 5];
        body.extend_from_slice(&300i32.to_be_bytes());
        let p = read_mount_screen_open(&body).unwrap();
        assert_eq!(
            p,
            MountScreenOpen {
                container_id: 7,
                columns: 5,
                entity_id: 300
            }
        );
        assert!(
            read_mount_screen_open(&[7, 5, 1]).is_err(),
            "a short int is an error"
        );
    }

    /// Which slots each mount gets: a horse both, a donkey (not in
    /// `can_wear_horse_armor`) no armour, a llama no saddle but its carpet, a
    /// baby horse no saddle, a baby nautilus neither.
    #[test]
    fn mounts_resolve_their_menu_and_active_slots() {
        use rewo_world::menu_layout::MountKind;
        let base = MountFacts {
            horse: true,
            nautilus: false,
            llama: false,
            baby: false,
            can_equip_saddle: true,
            can_wear_horse_armor: true,
        };
        assert_eq!(resolve_mount(base), Some((MountKind::Horse, true, true)));
        let donkey = MountFacts {
            can_wear_horse_armor: false,
            ..base
        };
        assert_eq!(resolve_mount(donkey), Some((MountKind::Horse, true, false)));
        let llama = MountFacts {
            llama: true,
            can_equip_saddle: false,
            can_wear_horse_armor: false,
            ..base
        };
        assert_eq!(resolve_mount(llama), Some((MountKind::Horse, false, true)));
        let foal = MountFacts { baby: true, ..base };
        assert_eq!(resolve_mount(foal), Some((MountKind::Horse, false, true)));
        let nautilus = MountFacts {
            horse: false,
            nautilus: true,
            ..base
        };
        assert_eq!(
            resolve_mount(nautilus),
            Some((MountKind::Nautilus, true, true))
        );
        assert_eq!(
            resolve_mount(MountFacts {
                baby: true,
                ..nautilus
            }),
            Some((MountKind::Nautilus, false, false))
        );
        assert_eq!(
            resolve_mount(MountFacts {
                horse: false,
                ..base
            }),
            None
        );
    }

    /// `canBeEquippedBy`'s three spellings against a horse's tags.
    #[test]
    fn a_mount_fit_reads_tags_lists_and_usability() {
        let horse = MountFit {
            type_name: "minecraft:horse".into(),
            type_tags: vec![
                "minecraft:can_equip_saddle".into(),
                "minecraft:can_wear_horse_armor".into(),
            ],
            saddle_usable: true,
            body_usable: true,
        };
        assert!(horse.accepts(true, Some("#minecraft:can_equip_saddle")));
        assert!(horse.accepts(false, Some("#minecraft:can_wear_horse_armor")));
        assert!(!horse.accepts(false, Some("minecraft:wolf")));
        assert!(!horse.accepts(false, Some("minecraft:llama|minecraft:trader_llama")));
        assert!(horse.accepts(false, None), "no allowed set: anyone");
        let foal = MountFit {
            saddle_usable: false,
            ..horse.clone()
        };
        assert!(
            !foal.accepts(true, Some("#minecraft:can_equip_saddle")),
            "a foal cannot use its saddle slot"
        );
        let llama = MountFit {
            type_name: "minecraft:trader_llama".into(),
            type_tags: vec![],
            ..horse
        };
        assert!(llama.accepts(false, Some("minecraft:llama|minecraft:trader_llama")));
    }
}
