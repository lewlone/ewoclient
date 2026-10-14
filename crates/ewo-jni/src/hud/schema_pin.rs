//! Pins the HUD data block (`hud/mod.rs`) to its Java writer
//! (`EwoHudData.java`, plus the indicator records `EwoIndicators.java` writes
//! into it).
//!
//! The block is a byte-for-byte contract between two languages. This test
//! reads the Java *source* and compares every constant, so editing one side
//! alone fails here instead of drawing garbage in-game (or, worse, nothing at
//! all: a schema mismatch only disables the widgets, with one log line).
//!
//! Bumping the layout means changing both files in the same commit; this test
//! is the reminder.

use std::collections::BTreeMap;

use super::*;
use crate::java_mirror::{accesses, between, get, int_constants, read_java};

/// The `pub const NAME: usize = N;` lines of `mod off`, read from the Rust
/// source (the module's items cannot be enumerated any other way), so a field
/// added on the Rust side without a Java twin is caught too.
fn rust_offsets() -> BTreeMap<String, i64> {
    let src = include_str!("mod.rs");
    let body = between(src, "pub(crate) mod off {", "\n}\n");
    let mut out = BTreeMap::new();
    for line in body.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        let Some(rest) = line.strip_prefix("pub const ") else {
            continue;
        };
        let (name, rest) = rest.split_once(':').expect("const name");
        let (_, value) = rest.split_once('=').expect("const value");
        let value = value
            .trim()
            .strip_suffix(';')
            .expect("const ends with ;")
            .trim();
        let value: i64 = value
            .parse()
            .unwrap_or_else(|_| panic!("off::{name} is not a plain literal: {value}"));
        out.insert(name.trim().to_string(), value);
    }
    assert!(out.len() > 20, "parsed too few offsets: {out:?}");
    out
}

#[test]
fn hud_schema_version_matches_the_java_mirror() {
    let java = int_constants(&read_java("EwoHudData.java"));
    assert_eq!(
        get(&java, "SCHEMA_VERSION"),
        SCHEMA_VERSION as i64,
        "EwoHudData.SCHEMA_VERSION and hud::SCHEMA_VERSION must be bumped together"
    );
}

#[test]
fn hud_offsets_match_the_java_mirror_exactly() {
    let java = int_constants(&read_java("EwoHudData.java"));
    let rust = rust_offsets();

    assert_eq!(
        get(&java, "OFF_VERSION"),
        0,
        "the schema word is read at offset 0 (HudData::schema_version)"
    );
    let java_offs: BTreeMap<String, i64> = java
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("OFF_")
                .filter(|n| *n != "VERSION")
                .map(|n| (n.to_string(), *v))
        })
        .collect();

    let only_java: Vec<_> = java_offs
        .keys()
        .filter(|k| !rust.contains_key(*k))
        .collect();
    let only_rust: Vec<_> = rust
        .keys()
        .filter(|k| !java_offs.contains_key(*k))
        .collect();
    assert!(
        only_java.is_empty() && only_rust.is_empty(),
        "offset names differ: only in EwoHudData.java {only_java:?}, only in hud::off {only_rust:?}"
    );
    for (name, rust_value) in &rust {
        assert_eq!(
            java_offs[name], *rust_value,
            "OFF_{name}: Java {} vs Rust {rust_value}",
            java_offs[name]
        );
    }
}

#[test]
fn hud_block_size_and_regions_are_consistent_with_java() {
    let java = int_constants(&read_java("EwoHudData.java"));
    let rust = rust_offsets();

    // `BLOCK_BYTES` is "one past the last field the reader touches"; the
    // registration check (`register_direct_buffer`) refuses anything smaller.
    // The last field is a 4-byte one.
    let last = *rust.values().max().unwrap();
    assert_eq!(
        BLOCK_BYTES as i64,
        last + 4,
        "BLOCK_BYTES must end exactly after the highest offset"
    );
    assert!(
        BLOCK_BYTES as i64 <= get(&java, "CAPACITY"),
        "EwoHudData.CAPACITY ({}) is smaller than the {BLOCK_BYTES} bytes the Rust reader touches",
        get(&java, "CAPACITY")
    );

    // Record strides and string capacities.
    assert_eq!(get(&java, "MAX_POTIONS"), MAX_POTIONS as i64);
    assert_eq!(get(&java, "POTION_REC"), POTION_REC as i64);
    assert_eq!(get(&java, "POTION_NAME_CAP"), POTION_NAME_CAP as i64);
    assert_eq!(get(&java, "TARGET_NAME_CAP"), TARGET_NAME_CAP as i64);
    assert_eq!(get(&java, "SERVER_CAP"), SERVER_CAP as i64);
    assert_eq!(get(&java, "PLAYER_NAME_CAP"), PLAYER_NAME_CAP as i64);

    let ind = int_constants(&read_java("EwoIndicators.java"));
    assert_eq!(get(&ind, "MAX_TRACKED"), MAX_INDICATORS as i64);
    assert_eq!(get(&ind, "RECORD"), INDICATOR_RECORD as i64);
    assert_eq!(get(&ind, "HEADER"), 4, "the reader skips an i32 count");

    // The variable-length regions must end where the next field starts, or a
    // capacity change on one side overlaps its neighbour.
    let o = |n: &str| rust[n];
    let end_of = |start: i64, header: i64, count: usize, stride: usize| {
        start + header + (count * stride) as i64
    };
    assert_eq!(
        end_of(o("POTIONS"), 0, MAX_POTIONS, POTION_REC),
        o("TARGET_PRESENT")
    );
    assert_eq!(
        end_of(o("TARGET_NAME"), 4, 1, TARGET_NAME_CAP),
        o("PLAYTIME")
    );
    assert_eq!(end_of(o("SERVER"), 4, 1, SERVER_CAP), o("PLAYER_NAME"));
    assert_eq!(
        end_of(o("PLAYER_NAME"), 4, 1, PLAYER_NAME_CAP),
        o("PVP_JUMP")
    );
    assert_eq!(
        end_of(o("INDICATORS"), 4, MAX_INDICATORS, INDICATOR_RECORD),
        o("SHIELD_COOLDOWN")
    );
}

#[test]
fn hud_flags_tiers_and_key_bits_match_the_java_mirror() {
    let java = int_constants(&read_java("EwoHudData.java"));

    let flags: [(&str, i32); 7] = [
        ("FLAG_WORLD", FLAG_WORLD),
        ("FLAG_PING", FLAG_PING),
        ("FLAG_ARMOR", FLAG_ARMOR),
        ("FLAG_TARGET", FLAG_TARGET),
        ("FLAG_OVERLAY", FLAG_OVERLAY),
        ("FLAG_PVP_JUMP", FLAG_PVP_JUMP),
        ("FLAG_PVP_HIT", FLAG_PVP_HIT),
    ];
    let java_flags: Vec<&String> = java.keys().filter(|k| k.starts_with("FLAG_")).collect();
    assert_eq!(
        java_flags.len(),
        flags.len(),
        "flag sets differ: {java_flags:?}"
    );
    for (name, rust) in flags {
        assert_eq!(get(&java, name), rust as i64, "{name}");
    }

    let tiers: [(&str, PvpTier); 6] = [
        ("TIER_NONE", PvpTier::None),
        ("TIER_PERFECT", PvpTier::Perfect),
        ("TIER_SLIGHTLY_EARLY", PvpTier::SlightlyEarly),
        ("TIER_EARLY", PvpTier::Early),
        ("TIER_SLIGHTLY_LATE", PvpTier::SlightlyLate),
        ("TIER_LATE", PvpTier::Late),
    ];
    let java_tiers: Vec<&String> = java.keys().filter(|k| k.starts_with("TIER_")).collect();
    assert_eq!(
        java_tiers.len(),
        tiers.len(),
        "tier sets differ: {java_tiers:?}"
    );
    for (name, tier) in tiers {
        assert_eq!(PvpTier::from_wire(get(&java, name) as i32), tier, "{name}");
    }

    // The keystroke widget declares its bit masks locally (`draw_keystrokes`);
    // read them back from its source.
    let widgets = include_str!("widgets.rs");
    let body = between(widgets, "pub(super) fn draw_keystrokes(", "draw_key(");
    let mut seen = 0;
    for line in body.lines() {
        let Some(rest) = line.trim().strip_prefix("const ") else {
            continue;
        };
        let Some((name, rest)) = rest.split_once(": i32 =") else {
            continue;
        };
        let expr = rest.trim().trim_end_matches(';');
        let value = match expr.split_once("<<") {
            Some((a, b)) => a.trim().parse::<i64>().unwrap() << b.trim().parse::<i64>().unwrap(),
            None => expr.parse::<i64>().unwrap(),
        };
        assert_eq!(get(&java, &format!("K_{name}")), value, "key bit {name}");
        seen += 1;
    }
    assert_eq!(
        seen, 5,
        "draw_keystrokes should declare FWD/LEFT/BACK/RIGHT/JUMP"
    );
}

#[test]
fn hud_record_layouts_match_what_java_writes() {
    let hud_src = include_str!("mod.rs");
    let calls_rust = [("i32_at(", "i32"), ("f32_at(", "f32"), ("str_at(", "str")];
    let calls_java = [
        ("putInt(", "i32"),
        ("putFloat(", "f32"),
        ("putString(b, ", "str"),
    ];

    // Potion record: Java builds `rec`, Rust reads `rec`.
    let java = read_java("EwoHudData.java");
    let java_potion = between(&java, "int rec = OFF_POTIONS", "count++");
    let rust_potion = between(hud_src, "pub fn potion(&self", "\n    }\n");
    let j = accesses(java_potion, "rec", &calls_java);
    let r = accesses(rust_potion, "rec", &calls_rust);
    assert!(j.len() >= 4, "found too few potion writes: {j:?}");
    assert_eq!(j, r, "potion record field layout: Java {j:?} vs Rust {r:?}");

    // Indicator record: Java writes `off + N`, Rust reads `rec + N`.
    let java_ind = read_java("EwoIndicators.java");
    let java_rec = between(
        &java_ind,
        "int off = recordsOff + count * RECORD",
        "count++",
    );
    let rust_rec = between(hud_src, "pub fn indicator(&self", "\n    }\n");
    let j = accesses(java_rec, "off", &calls_java);
    let r = accesses(rust_rec, "rec", &calls_rust);
    assert!(j.len() >= 10, "found too few indicator writes: {j:?}");
    assert_eq!(
        j, r,
        "indicator record field layout: Java {j:?} vs Rust {r:?}"
    );
}
