//! `SlotRanges` — the table behind `item_slot` and `item_slots` (M124).
//!
//! This is the **construction**, not a transcribed list of the 165 names it
//! produces. `SlotRanges`' static initialiser is six `addSlotRange` calls and a
//! dozen `addSingleSlot`s, and mirroring those is both shorter than the names
//! they generate and impossible to get wrong one entry at a time — the M93r
//! lesson that a derivation pins a table better than its output does.
//!
//! Only the **name** and the **size** are kept. The slot ids are what the
//! server acts on; the client's parse needs the name to exist (`nameToIds`
//! returning null is `slot.unknown`) and, for `item_slot` alone, the size to be
//! exactly one. So `EquipmentSlot.getIndex(...)` never has to be chased.
//!
//! # `item_slot` and `item_slots` differ in more than their suggestions
//!
//! `SlotArgument` additionally rejects `size() != 1`, so `container.*` is
//! `slot.only_single_allowed` there and a perfectly good value one type over.
//! That is also exactly the split between `singleSlotNames()` and `allNames()`,
//! so the suggester and the parser agree by construction rather than by two
//! lists being kept in step.
//!
//! # A name may contain a `*`, so it is not an unquoted string
//!
//! Both argument types read with `ParserUtils.readWhile(c -> c != ' ')`, not
//! `readUnquotedString`. `container.5` happens to be legal either way; the star
//! forms are not, and reading them as unquoted strings stops at the `*` and
//! then fails the lookup on a truncated name.

use std::sync::OnceLock;

/// One entry: its serialized name, and how many slots it covers.
type Entry = (String, usize);

fn build() -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();

    // `addSingleSlot`.
    fn single(out: &mut Vec<Entry>, name: &str) {
        out.push((name.to_string(), 1));
    }
    // `addSlotRange` — `prefix + i` for each of `size`, and THEN `prefix + "*"`
    // covering all of them. The star is one entry of size `size`, which is why
    // it is absent from `singleSlotNames` for every range in the table.
    fn range(out: &mut Vec<Entry>, prefix: &str, size: usize) {
        for i in 0..size {
            out.push((format!("{prefix}{i}"), 1));
        }
        out.push((format!("{prefix}*"), size));
    }
    // `addSlots` — a named group with no numbered members of its own.
    fn slots(out: &mut Vec<Entry>, name: &str, size: usize) {
        out.push((name.to_string(), size));
    }

    single(&mut out, "contents");
    range(&mut out, "container.", 54);
    range(&mut out, "hotbar.", 9);
    range(&mut out, "inventory.", 27);
    range(&mut out, "enderchest.", 27);
    range(&mut out, "mob.inventory.", 8);
    range(&mut out, "horse.", 15);
    single(&mut out, "weapon");
    single(&mut out, "weapon.mainhand");
    single(&mut out, "weapon.offhand");
    slots(&mut out, "weapon.*", 2);
    single(&mut out, "armor.head");
    single(&mut out, "armor.chest");
    single(&mut out, "armor.legs");
    single(&mut out, "armor.feet");
    single(&mut out, "armor.body");
    slots(&mut out, "armor.*", 5);
    single(&mut out, "saddle");
    single(&mut out, "horse.chest");
    single(&mut out, "player.cursor");
    range(&mut out, "player.crafting.", 4);

    out
}

fn table() -> &'static [Entry] {
    static TABLE: OnceLock<Vec<Entry>> = OnceLock::new();
    TABLE.get_or_init(build)
}

/// `SlotRanges.nameToIds` — reduced to the size, which is all a client checks.
pub fn lookup(name: &str) -> Option<usize> {
    table().iter().find(|(n, _)| n == name).map(|&(_, size)| size)
}

/// `SlotRanges.allNames` — what `item_slots` suggests.
pub fn all_names() -> impl Iterator<Item = &'static str> {
    table().iter().map(|(n, _)| n.as_str())
}

/// `SlotRanges.singleSlotNames` — what `item_slot` suggests.
pub fn single_slot_names() -> impl Iterator<Item = &'static str> {
    table()
        .iter()
        .filter(|&&(_, size)| size == 1)
        .map(|(n, _)| n.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_has_the_shape_the_six_ranges_and_thirteen_singles_produce() {
        // 1 + 55 + 10 + 28 + 28 + 9 + 16 + 4 + 6 + 1 + 1 + 1 + 5.
        assert_eq!(all_names().count(), 165);
        // Nine of them are `*` groups, and every one covers more than one slot,
        // so `singleSlotNames` is the other 156.
        assert_eq!(single_slot_names().count(), 156);
        assert_eq!(all_names().filter(|n| n.ends_with('*')).count(), 9);
    }

    #[test]
    fn a_range_numbers_from_zero_and_stops_one_short_of_its_size() {
        assert_eq!(lookup("container.0"), Some(1));
        assert_eq!(lookup("container.53"), Some(1));
        assert_eq!(lookup("container.54"), None);
        assert_eq!(lookup("hotbar.8"), Some(1));
        assert_eq!(lookup("hotbar.9"), None);
        assert_eq!(lookup("player.crafting.3"), Some(1));
        assert_eq!(lookup("player.crafting.4"), None);
    }

    #[test]
    fn a_star_covers_its_whole_range_and_is_therefore_not_a_single_slot() {
        assert_eq!(lookup("container.*"), Some(54));
        assert_eq!(lookup("armor.*"), Some(5));
        assert_eq!(lookup("weapon.*"), Some(2));
        assert!(!single_slot_names().any(|n| n == "container.*"));
        assert!(single_slot_names().any(|n| n == "weapon.mainhand"));
    }

    #[test]
    fn horse_chest_is_its_own_entry_and_not_a_member_of_the_horse_range() {
        // `horse.` numbers 0..14 and `horse.chest` is added separately, so a
        // reading that folded the two together would lose one or the other.
        assert_eq!(lookup("horse.14"), Some(1));
        assert_eq!(lookup("horse.chest"), Some(1));
        assert_eq!(lookup("horse.*"), Some(15));
    }

    #[test]
    fn every_name_is_unique() {
        // `horse.chest` sits next to a `horse.` range and `player.cursor` next
        // to `player.crafting.`; a collision would make `nameToIds`' map
        // ambiguous and is worth failing loudly on.
        let mut names: Vec<&str> = all_names().collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total);
    }
}
