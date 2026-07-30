//! `StatsCounter` and `StatFormatter` (M84).
//!
//! The counter is trivial and the formatters are not. Both are transcribed from
//! `net/minecraft/stats/`.
//!
//! # `handleAwardStats` replaces, it does not accumulate
//!
//! ```java
//! for (entry : packet.stats()) player.getStats().setValue(player, entry.getKey(), entry.getIntValue());
//! ```
//!
//! `setValue`, not `increment` — and the map is **not cleared first**, so a
//! stat the packet omits keeps whatever it had. In practice the server sends
//! every non-zero stat it holds, so the distinction only shows if a stat goes
//! back to zero server-side while the client is connected.
//!
//! # Two formatter branches fall through to Java's own `toString`
//!
//! `DECIMAL_FORMAT` is `"########0.00"` and covers the km / m / y / d / h / min
//! branches. The **last** branch of each of `DISTANCE` and `TIME` does not use
//! it:
//!
//! ```java
//! return meters > 0.5 ? DECIMAL_FORMAT.format(meters) + " m" : cm + " cm";
//! //                                                          ^^ an int
//! return minutes > 0.5 ? DECIMAL_FORMAT.format(minutes) + " min" : seconds + " s";
//! //                                                               ^^^^^^^ a double
//! ```
//!
//! so a short distance prints as a bare integer with **no grouping and no
//! decimals** (`"37 cm"`, never `"0.37 m"`), and a short time prints through
//! `Double.toString` — which always emits at least one fractional digit, so 20
//! ticks is `"1.0 s"` and not `"1 s"`. Both are easy to "tidy" into
//! `DECIMAL_FORMAT` and both would then be wrong in a way no unit is attached
//! to.
//!
//! And `DEFAULT` is `NumberFormat.getIntegerInstance(Locale.US)`, which
//! **groups**: 12345 is `"12,345"`. The `DECIMAL_FORMAT` pattern has no `,` in
//! it, so the two disagree about grouping in opposite directions.

use std::collections::HashMap;

pub use rewo_data::stats::Formatter;

/// One statistic's identity on the wire: the `minecraft:stat_type` id and the
/// id of the value inside *that type's* registry.
///
/// Kept as the raw pair rather than resolved to names at decode time, and that
/// is deliberate. Vanilla's `byIdOrThrow` would reject an id its registry does
/// not hold; Rewo keeps it and resolves lazily, so an unresolvable value costs
/// one dropped row instead of a dropped packet. The wire shape does not depend
/// on either registry — see `rewo_data::stats`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub struct StatKey {
    pub type_id: i32,
    pub value_id: i32,
}

impl StatKey {
    pub fn new(type_id: i32, value_id: i32) -> Self {
        Self { type_id, value_id }
    }
}

/// `StatsCounter`, client side.
#[derive(Clone, Debug, Default)]
pub struct StatsCounter {
    stats: HashMap<StatKey, i32>,
    /// How many `award_stats` packets have landed. The screen watches this to
    /// know when its "Retrieving statistics…" state is over — vanilla uses the
    /// `onStatsUpdated` callback, which Rewo cannot, because the screen is a
    /// plain value and not a listener registered with the connection.
    pub updates: u64,
}

impl StatsCounter {
    /// `stats.getInt(stat)` with `defaultReturnValue(0)` — an absent stat is
    /// **zero, not missing**, which is what lets the item and mob lists ask
    /// about every registry entry without checking first.
    pub fn value(&self, key: StatKey) -> i32 {
        self.stats.get(&key).copied().unwrap_or(0)
    }

    /// `setValue`.
    pub fn set(&mut self, key: StatKey, value: i32) {
        self.stats.insert(key, value);
    }

    /// One `award_stats` packet: every pair, then the update counter.
    pub fn apply(&mut self, pairs: &[(StatKey, i32)]) {
        for (key, value) in pairs {
            self.set(*key, *value);
        }
        self.updates = self.updates.saturating_add(1);
    }

    pub fn len(&self) -> usize {
        self.stats.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stats.is_empty()
    }

    /// Every stat of one type that is non-zero, as `(value_id, count)`.
    pub fn of_type(&self, type_id: i32) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.stats
            .iter()
            .filter(move |(k, v)| k.type_id == type_id && **v != 0)
            .map(|(k, v)| (k.value_id, *v))
    }
}

/// `NumberFormat.getIntegerInstance(Locale.US)::format` — grouped by threes
/// with a comma.
pub fn grouped(value: i32) -> String {
    // `abs()` on `i32::MIN` overflows; go through i64.
    let digits = (value as i64).unsigned_abs().to_string();
    let n = digits.len();
    let mut out = String::with_capacity(n + n / 3 + 1);
    if value < 0 {
        out.push('-');
    }
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (n - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `DECIMAL_FORMAT` — `new DecimalFormat("########0.00", ROOT)`.
///
/// Two fractional digits always, at least one integer digit, **no grouping**
/// (the pattern has no `,`), `RoundingMode.HALF_EVEN`.
///
/// Java rounds the double's *exact* binary value; this rounds `x * 100.0`,
/// which introduces one extra rounding step. The two can only disagree when
/// the scaled value lands exactly on a tie, and none of the four call sites
/// can produce one: `v * 0.1` has at most one decimal place, `cm / 100.0` at
/// most two, and the distance/time quotients are irrational multiples of the
/// input. Named because it is an approximation, not because it bites.
pub fn decimal2(x: f64) -> String {
    if !x.is_finite() {
        // `DecimalFormat` prints "∞"/"NaN"; unreachable from an i32 input.
        return format!("{x}");
    }
    let neg = x < 0.0;
    let scaled = (x.abs() * 100.0).round_ties_even();
    let whole = (scaled / 100.0).floor() as i64;
    let frac = (scaled as i64) - whole * 100;
    let body = format!("{whole}.{frac:02}");
    if neg && (whole != 0 || frac != 0) {
        format!("-{body}")
    } else {
        body
    }
}

/// `Double.toString` for the seconds branch of `TIME`.
///
/// The value is always `ticks / 20.0`, i.e. an exact multiple of 0.05 whose
/// nearest double round-trips to at most two decimal places — so the shortest
/// round-tripping decimal Java prints is reproduced exactly by writing those
/// hundredths and stripping one trailing zero. Java never prints a bare
/// integer: `1.0`, not `1`.
fn java_double_of_twentieths(ticks: i32) -> String {
    let neg = ticks < 0;
    let hundredths = (ticks as i64).unsigned_abs() * 5; // ticks/20 == ticks*5/100
    let whole = hundredths / 100;
    let frac = hundredths % 100;
    let body = if frac == 0 {
        format!("{whole}.0")
    } else if frac % 10 == 0 {
        format!("{whole}.{}", frac / 10)
    } else {
        format!("{whole}.{frac:02}")
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

/// `StatFormatter.format(value)`, all four implementations.
///
/// A free function rather than a method on [`Formatter`], because the *table*
/// (which stat takes which formatter) is a `rewo-data` concern and the
/// *behaviour* is this crate's; a trait impl across the boundary would buy
/// nothing but a second name for the same call.
pub fn format_stat(formatter: Formatter, value: i32) -> String {
    match formatter {
        Formatter::Default => grouped(value),
        Formatter::DivideByTen => decimal2(value as f64 * 0.1),
        Formatter::Distance => {
            let meters = value as f64 / 100.0;
            let kilometers = meters / 1000.0;
            if kilometers > 0.5 {
                format!("{} km", decimal2(kilometers))
            } else if meters > 0.5 {
                format!("{} m", decimal2(meters))
            } else {
                // `cm + " cm"` — the raw int, ungrouped.
                format!("{value} cm")
            }
        }
        Formatter::Time => {
            let seconds = value as f64 / 20.0;
            let minutes = seconds / 60.0;
            let hours = minutes / 60.0;
            let days = hours / 24.0;
            let years = days / 365.0;
            if years > 0.5 {
                format!("{} y", decimal2(years))
            } else if days > 0.5 {
                format!("{} d", decimal2(days))
            } else if hours > 0.5 {
                format!("{} h", decimal2(hours))
            } else if minutes > 0.5 {
                format!("{} min", decimal2(minutes))
            } else {
                format!("{} s", java_double_of_twentieths(value))
            }
        }
    }
}

/// `StatsScreen.getTranslationKey` — `"stat." + value.toString().replace(':', '.')`.
///
/// Only the **custom** type has one: the general list is `Stats.CUSTOM`'s
/// entries and nothing else.
pub fn custom_stat_key(custom_stat_name: &str) -> String {
    format!("stat.{}", custom_stat_name.replace(':', "."))
}

/// `StatType.getDisplayName()` — `"stat_type.minecraft." + name`, built from
/// the **bare** name `makeRegistryStatType` was called with, not the namespaced
/// registry key.
pub fn stat_type_key(stat_type_name: &str) -> String {
    let bare = stat_type_name
        .split_once(':')
        .map(|(_, s)| s)
        .unwrap_or(stat_type_name);
    format!("stat_type.minecraft.{bare}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_formatter_groups_by_threes() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(7), "7");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(12345), "12,345");
        assert_eq!(grouped(1234567), "1,234,567");
        assert_eq!(grouped(-12345), "-12,345");
        assert_eq!(grouped(i32::MAX), "2,147,483,647");
        assert_eq!(grouped(i32::MIN), "-2,147,483,648");
    }

    /// `"########0.00"` has no `,` in it, so the two formatters disagree about
    /// grouping — and they are used side by side in the same list.
    #[test]
    fn the_decimal_formatter_does_not_group() {
        assert_eq!(decimal2(12345.6), "12345.60");
        assert_eq!(grouped(12345), "12,345");
        assert_eq!(decimal2(0.0), "0.00");
        assert_eq!(decimal2(0.5), "0.50");
        assert_eq!(decimal2(1.239), "1.24");
        assert_eq!(decimal2(-1.5), "-1.50");
    }

    #[test]
    fn divide_by_ten_is_a_tenth_at_two_decimal_places() {
        // 3 hearts of damage is 60 half-hearts * ... — the stat counts tenths.
        assert_eq!(format_stat(Formatter::DivideByTen, 0), "0.00");
        assert_eq!(format_stat(Formatter::DivideByTen, 1), "0.10");
        assert_eq!(format_stat(Formatter::DivideByTen, 205), "20.50");
        assert_eq!(format_stat(Formatter::DivideByTen, 10000), "1000.00");
    }

    /// The three-way ladder, sampled **on** both boundaries. `> 0.5`, not
    /// `>=`, so 50 cm is still centimetres and 51 is metres.
    #[test]
    fn distance_switches_units_on_a_strict_half() {
        assert_eq!(format_stat(Formatter::Distance, 0), "0 cm");
        assert_eq!(format_stat(Formatter::Distance, 50), "50 cm");
        assert_eq!(
            format_stat(Formatter::Distance, 51),
            "0.51 m",
            "meters = 0.51 > 0.5"
        );
        // km takes over above 500 m == 50,000 cm.
        assert_eq!(format_stat(Formatter::Distance, 50_000), "500.00 m");
        assert_eq!(format_stat(Formatter::Distance, 50_001), "0.50 km");
    }

    /// The centimetre branch is `cm + " cm"` on an **int**: no grouping, no
    /// decimals. A tidy-up into `DECIMAL_FORMAT` would print "50.00 cm".
    #[test]
    fn the_centimetre_branch_is_a_bare_ungrouped_integer() {
        assert_eq!(format_stat(Formatter::Distance, 50), "50 cm");
        assert!(!format_stat(Formatter::Distance, 50).contains('.'));
        // Reachable only for |cm| <= 50, so grouping never shows here — but
        // the *negative* side is, and it is still bare.
        assert_eq!(format_stat(Formatter::Distance, -1234), "-1234 cm");
    }

    /// The seconds branch is `Double.toString`, which always prints a
    /// fractional digit.
    #[test]
    fn the_seconds_branch_never_prints_a_bare_integer() {
        assert_eq!(format_stat(Formatter::Time, 0), "0.0 s");
        assert_eq!(format_stat(Formatter::Time, 20), "1.0 s");
        assert_eq!(format_stat(Formatter::Time, 1), "0.05 s");
        assert_eq!(format_stat(Formatter::Time, 3), "0.15 s");
        assert_eq!(format_stat(Formatter::Time, 10), "0.5 s");
        assert_eq!(format_stat(Formatter::Time, 600), "30.0 s");
    }

    /// Every rung of the time ladder, sampled on its boundary. `minutes > 0.5`
    /// is 601 ticks, not 600.
    #[test]
    fn time_switches_units_on_strict_halves() {
        assert_eq!(format_stat(Formatter::Time, 600), "30.0 s");
        assert_eq!(format_stat(Formatter::Time, 601), "0.50 min");
        // hours > 0.5 → 30 min → 36000 ticks
        assert_eq!(format_stat(Formatter::Time, 36_000), "30.00 min");
        assert_eq!(format_stat(Formatter::Time, 36_001), "0.50 h");
        // days > 0.5 → 12 h → 864000 ticks
        assert_eq!(format_stat(Formatter::Time, 864_000), "12.00 h");
        assert_eq!(format_stat(Formatter::Time, 864_001), "0.50 d");
    }

    #[test]
    fn the_counter_replaces_rather_than_accumulating() {
        let mut c = StatsCounter::default();
        let k = StatKey::new(8, 1);
        c.apply(&[(k, 5)]);
        assert_eq!(c.value(k), 5);
        c.apply(&[(k, 3)]);
        assert_eq!(c.value(k), 3, "setValue, not increment");
        assert_eq!(c.updates, 2);
        // A stat the second packet omits keeps its value — the map is not
        // cleared.
        let other = StatKey::new(8, 2);
        c.apply(&[(other, 9)]);
        assert_eq!(c.value(k), 3);
        assert_eq!(c.value(other), 9);
    }

    #[test]
    fn an_absent_stat_reads_as_zero_rather_than_missing() {
        let c = StatsCounter::default();
        assert_eq!(c.value(StatKey::new(0, 0)), 0);
        assert!(c.is_empty());
    }

    #[test]
    fn the_two_translation_keys_replace_the_colon_and_drop_the_namespace() {
        assert_eq!(
            custom_stat_key("minecraft:play_time"),
            "stat.minecraft.play_time"
        );
        assert_eq!(stat_type_key("minecraft:killed"), "stat_type.minecraft.killed");
        // `getDisplayName` hard-codes `minecraft.` and appends the *bare* name,
        // so a namespaced key would double the namespace.
        assert_ne!(
            stat_type_key("minecraft:killed"),
            "stat_type.minecraft.minecraft:killed"
        );
    }
}
