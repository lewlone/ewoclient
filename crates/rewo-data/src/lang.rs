//! The language map, and `Component.translatable`'s argument substitution
//! (M50).
//!
//! **`assets/minecraft/lang/en_us.json` is not the language map.** It is the
//! first of three steps, and a client that stops there answers `None` for keys
//! the real client resolves. `ClientLanguage.loadFrom` — and
//! `Language.loadDefault`, which is the same sequence for the built-in
//! instance — is:
//!
//! 1. `Language.loadFromJson` reads the file **and rewrites every unsupported
//!    format specifier**: `UNSUPPORTED_FORMAT_PATTERN` is
//!    `%(\d+\$)?[\d.]*[df]` and the replacement is `%$1s`, so a `%d` becomes
//!    `%s` and a `%1$.2f` becomes `%1$s`. This is why `decomposeTemplate` only
//!    has to understand `s`.
//! 2. `DeprecatedTranslationsInfo.loadFromDefaultResource().applyToMap` —
//!    `assets/minecraft/lang/deprecated.json`, which 26.2 ships with **383
//!    removed keys and 146 renames**.
//! 3. The result is frozen with `Map.copyOf`.
//!
//! `applyToMap` is two passes and **the order between them is load-bearing**:
//!
//! ```java
//! for (String key : this.removed) translations.remove(key);
//! this.renamed.forEach((fromKey, toKey) -> {
//!    String value = translations.remove(fromKey);
//!    if (value == null) translations.remove(toKey);   // and warn
//!    else                translations.put(toKey, value);
//! });
//! ```
//!
//! Three of 26.2's removed keys — `debug.crash.message`,
//! `debug.profiling.start` and `selectWorld.backupRequiredTooltip` — are also
//! rename **targets**, so they are deleted and then written back. Running the
//! renames first would delete them for good.
//!
//! What it costs to skip the pass: **105 of the 146 rename targets do not
//! appear in `en_us.json` at all**, so they resolve to nothing without it.
//! `item.container.item_count` (`%s x%s`) and `item.container.more_items`
//! (`and %s more...`) are both in that set. A further 41 targets *are* present
//! and are **overwritten** — which is how `item.minecraft.bolt_armor_trim_
//! smithing_template` stops reading "Smithing Template" and starts reading
//! "Bolt Armor Trim".
//!
//! The rename map is kept in **file order**, because `Codec.unboundedMap`
//! decodes a gson `JsonObject` (insertion-ordered) into an `ImmutableMap` and
//! `forEach` walks it in that order. 26.2's data happens to be order-free —
//! no two renames share a target, and the one rename whose target is also a
//! source is `subtitles.entity.sulfur_cube.squish` onto itself — but a version
//! that chained two renames would depend on it.

use std::collections::HashMap;
use std::path::Path;

use indexmap::IndexMap;

/// `assets/minecraft/lang/<code>.json`, for the one language Rewo ships.
pub const EN_US_PATH: &str = "assets/minecraft/lang/en_us.json";
/// `DeprecatedTranslationsInfo.loadFromDefaultResource`'s path.
pub const DEPRECATED_PATH: &str = "assets/minecraft/lang/deprecated.json";

/// `DeprecatedTranslationsInfo` — the record, and the pass it applies.
#[derive(Clone, Debug, Default)]
pub struct DeprecatedTranslations {
    removed: Vec<String>,
    /// `IndexMap`, not `HashMap`: see the module docs on why the order the
    /// file lists these in is part of their meaning.
    renamed: IndexMap<String, String>,
}

impl DeprecatedTranslations {
    /// `DeprecatedTranslationsInfo.EMPTY`, which is also what
    /// `loadFromResource` falls back to when the file is missing.
    pub fn empty() -> Self {
        Self::default()
    }

    /// `DeprecatedTranslationsInfo.CODEC` — both fields are required, so a
    /// document missing either is an error rather than a half-applied pass.
    pub fn parse(json: &str) -> Result<Self, String> {
        #[derive(serde::Deserialize)]
        struct Doc {
            removed: Vec<String>,
            renamed: IndexMap<String, String>,
        }
        let doc: Doc =
            serde_json::from_str(json).map_err(|e| format!("deprecated.json: {e}"))?;
        Ok(Self {
            removed: doc.removed,
            renamed: doc.renamed,
        })
    }

    /// `DeprecatedTranslationsInfo.applyToMap`.
    ///
    /// Vanilla logs a warning on a rename whose source is absent; the
    /// *behaviour* is the `translations.remove(toKey)` on the next line, which
    /// is the half that is easy to drop. Dropping it leaves a stale value
    /// under a key the game has declared dead.
    pub fn apply_to_map(&self, translations: &mut HashMap<String, String>) {
        for key in &self.removed {
            translations.remove(key);
        }
        for (from_key, to_key) in &self.renamed {
            match translations.remove(from_key) {
                Some(value) => {
                    translations.insert(to_key.clone(), value);
                }
                None => {
                    log::debug!("rewo-data: missing translation key for rename: {from_key}");
                    translations.remove(to_key);
                }
            }
        }
    }

    pub fn removed(&self) -> &[String] {
        &self.removed
    }

    /// The renames, in file order.
    pub fn renamed(&self) -> impl Iterator<Item = (&str, &str)> {
        self.renamed.iter().map(|(a, b)| (a.as_str(), b.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.renamed.is_empty()
    }
}

/// The loaded language — `en_us.json` after the deprecation pass.
///
/// One map for the whole client. Every caller that used to parse
/// `en_us.json` itself and filter it by prefix reads this instead, so no two
/// of them can disagree about what a key resolves to.
#[derive(Clone, Debug, Default)]
pub struct Language {
    map: HashMap<String, String>,
}

impl Language {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Read both files out of the client jar and run the three steps.
    ///
    /// A missing `en_us.json` yields an empty map rather than an error: every
    /// caller already treats an unresolved key as "no text", and a client that
    /// refused to start over a language file would be worse than one whose
    /// tooltips are bare. A missing `deprecated.json` is
    /// `DeprecatedTranslationsInfo.EMPTY`, which is vanilla's own fallback.
    pub fn load(client_jar: &Path) -> Self {
        let Some(raw) = crate::assets::jar_text(client_jar, EN_US_PATH) else {
            log::warn!("rewo-data: no en_us.json — every translation key is unresolved");
            return Self::empty();
        };
        let deprecated = match crate::assets::jar_text(client_jar, DEPRECATED_PATH) {
            Some(d) => match DeprecatedTranslations::parse(&d) {
                Ok(d) => d,
                Err(e) => {
                    log::warn!("rewo-data: {e} — the deprecation pass is skipped");
                    DeprecatedTranslations::empty()
                }
            },
            None => {
                log::warn!("rewo-data: no deprecated.json — the deprecation pass is skipped");
                DeprecatedTranslations::empty()
            }
        };
        let out = Self::from_json(&raw, &deprecated);
        log::info!(
            "rewo-data: {} translation(s) ({} removed, {} renamed)",
            out.map.len(),
            deprecated.removed.len(),
            deprecated.renamed.len()
        );
        out
    }

    /// The pipeline with both documents already in hand, for the gate.
    pub fn from_json(lang_json: &str, deprecated: &DeprecatedTranslations) -> Self {
        let mut map = load_from_json(lang_json);
        deprecated.apply_to_map(&mut map);
        Self { map }
    }

    /// `Language.loadFromJson` alone — the raw file with the format rewrite,
    /// and **no** deprecation pass. This is what Rewo used to do everywhere,
    /// and it exists so the gate can measure the difference.
    pub fn raw(lang_json: &str) -> Self {
        Self {
            map: load_from_json(lang_json),
        }
    }

    pub fn from_map(map: HashMap<String, String>) -> Self {
        Self { map }
    }

    /// `Language.has`.
    pub fn has(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    /// The value, or `None`. Prefer this to [`Language::or_key`] wherever the
    /// caller can render *nothing* instead — a tooltip line reading
    /// `item.minecraft.thing` is worse than no line.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(String::as_str)
    }

    /// `Language.getOrDefault(key, default)`.
    pub fn get_or_default<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.map.get(key).map(String::as_str).unwrap_or(default)
    }

    /// `Language.getOrDefault(key)` — the single-argument form, whose default
    /// is **the key itself**. That is what puts a bare `block.minecraft.thing`
    /// on screen in a real client when a translation is missing.
    pub fn or_key<'a>(&'a self, key: &'a str) -> &'a str {
        self.get_or_default(key, key)
    }

    /// `Component.translatable(key, args...)` resolved to plain text.
    pub fn translate(&self, key: &str, args: &[&str]) -> String {
        format(self.or_key(key), args)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Every key, for callers that want a filtered sub-map (the enchantment
    /// strings) rather than point lookups.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.map.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

/// `Language.loadFromJson` — parse, then rewrite unsupported specifiers.
///
/// A value that is not a string is skipped; gson's `convertToString` would
/// stringify a number, but 26.2's file has none and a silently stringified
/// object would be worse than a missing key.
fn load_from_json(json: &str) -> HashMap<String, String> {
    let Ok(raw) = serde_json::from_str::<HashMap<String, serde_json::Value>>(json) else {
        log::warn!("rewo-data: en_us.json did not parse — every key is unresolved");
        return HashMap::new();
    };
    raw.into_iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k, rewrite_unsupported_formats(s))))
        .collect()
}

/// `UNSUPPORTED_FORMAT_PATTERN.matcher(text).replaceAll("%$1s")`, where the
/// pattern is `%(\d+\$)?[\d.]*[df]`.
///
/// **Inert on 26.2's own `en_us.json`** — zero of its 8,123 values match. It is
/// transcribed anyway because it is a real step of the load, a resource pack
/// may carry one, and its absence would be invisible until the day it wasn't:
/// `decomposeTemplate` throws on any specifier but `s`, and a throw collapses
/// the whole line to its raw pattern.
fn rewrite_unsupported_formats(text: &str) -> String {
    let b = text.as_bytes();
    // Byte-wise, because a `%` scan must not care about code-point widths:
    // every byte copied is either copied verbatim or replaced by ASCII, so the
    // result is still valid UTF-8.
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while i < b.len() {
        if b[i] != b'%' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        match match_unsupported(b, i) {
            Some((end, (gs, ge))) => {
                out.push(b'%');
                out.extend_from_slice(&b[gs..ge]);
                out.push(b's');
                i = end;
            }
            None => {
                out.push(b'%');
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// One `%(\d+\$)?[\d.]*[df]` match starting at `i`, as `(end, group-1 range)`.
///
/// The optional group is greedy, so it is tried first and the whole match is
/// retried without it — which is what Java's backtracking does. `[\d.]*` needs
/// no backtracking of its own: it only ever consumes digits and dots, and
/// neither is `d` or `f`.
fn match_unsupported(b: &[u8], i: usize) -> Option<(usize, (usize, usize))> {
    debug_assert_eq!(b[i], b'%');
    let j = i + 1;
    let mut k = j;
    while k < b.len() && b[k].is_ascii_digit() {
        k += 1;
    }
    let with_group = (k > j && b.get(k) == Some(&b'$')).then_some((j, k + 1));
    for group in [with_group, None] {
        let (start, end) = group.unwrap_or((j, j));
        let mut p = end;
        while p < b.len() && (b[p].is_ascii_digit() || b[p] == b'.') {
            p += 1;
        }
        if p < b.len() && (b[p] == b'd' || b[p] == b'f') {
            return Some((p + 1, (start, end)));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// `TranslatableContents.decomposeTemplate` — the argument substitution.
// ---------------------------------------------------------------------------

/// `Component.translatable(key, args).getString()`, given the resolved format
/// string.
///
/// Vanilla's `decompose` wraps `decomposeTemplate` in
/// `catch (TranslatableFormatException)` and falls back to
/// `FormattedText.of(format)` — **the unsubstituted pattern**. So every error
/// here (an unsupported specifier, a stray `%`, an argument index past the end
/// of the list) renders the raw string rather than dropping the line or
/// panicking, and that is why this returns a `String` and not a `Result`.
pub fn format(template: &str, args: &[&str]) -> String {
    decompose(template, args).unwrap_or_else(|| template.to_string())
}

/// The body, where `None` is vanilla's `TranslatableFormatException`.
fn decompose(template: &str, args: &[&str]) -> Option<String> {
    let mut out = String::with_capacity(template.len());
    let mut replacement_index = 0usize;
    let mut current = 0usize;
    while let Some(m) = find_format(template, current) {
        if m.start > current {
            let prefix = &template[current..m.start];
            // A `%` that the pattern did not match is an error, not literal
            // text: `%q` and a bare trailing `%` both land here.
            if prefix.contains('%') {
                return None;
            }
            out.push_str(prefix);
        }
        let format_string = &template[m.start..m.end];
        if m.ty == Some(b'%') && format_string == "%%" {
            // Note the second half of the guard: `%1$%` also has `%` for a
            // format type, and it is an error rather than a literal percent.
            out.push('%');
        } else {
            if m.ty != Some(b's') {
                return None;
            }
            let index = match m.digits {
                // `Integer.parseInt(...) - 1`. A non-representable number and
                // an explicit `%0$s` both become an out-of-range index, which
                // `getArgument` throws on.
                Some((a, b)) => template[a..b].parse::<usize>().ok()?.checked_sub(1)?,
                None => {
                    let i = replacement_index;
                    replacement_index += 1;
                    i
                }
            };
            out.push_str(args.get(index)?);
        }
        current = m.end;
    }
    if current < template.len() {
        let tail = &template[current..];
        if tail.contains('%') {
            return None;
        }
        out.push_str(tail);
    }
    Some(out)
}

struct FormatMatch {
    start: usize,
    end: usize,
    /// Group 1, `(\d+)` of the `\d+\$` prefix.
    digits: Option<(usize, usize)>,
    /// Group 2. `None` is the `$` alternative matching empty at end of input,
    /// which is Java's `""` — neither `"%"` nor `"s"`, so it throws.
    ty: Option<u8>,
}

/// `FORMAT_PATTERN.matcher(template).find(from)` where the pattern is
/// `%(?:(\d+)\$)?([A-Za-z%]|$)`.
///
/// The `$` alternative is implemented as end-of-input. Java's non-multiline
/// `$` also matches before a *final* line terminator, but the difference is
/// unobservable: both readings end in a `TranslatableFormatException` for the
/// same template, because a trailing `%` is either an unsupported format type
/// or an unmatched `%` in the tail.
fn find_format(template: &str, from: usize) -> Option<FormatMatch> {
    let b = template.as_bytes();
    for start in from..b.len() {
        if b[start] != b'%' {
            continue;
        }
        let j = start + 1;
        let mut k = j;
        while k < b.len() && b[k].is_ascii_digit() {
            k += 1;
        }
        // The optional group is greedy, so it is attempted first.
        let with_group = (k > j && b.get(k) == Some(&b'$')).then_some(((j, k), k + 1));
        for attempt in [with_group, Some(((0, 0), j))] {
            let Some((digits, p)) = attempt else { continue };
            let digits = (digits.1 > digits.0).then_some(digits);
            if p < b.len() && (b[p].is_ascii_alphabetic() || b[p] == b'%') {
                return Some(FormatMatch {
                    start,
                    end: p + 1,
                    digits,
                    ty: Some(b[p]),
                });
            }
            if p == b.len() {
                return Some(FormatMatch {
                    start,
                    end: p,
                    digits,
                    ty: None,
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dep(json: &str) -> DeprecatedTranslations {
        DeprecatedTranslations::parse(json).expect("parse")
    }

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// The rename moves the value; it does not copy it.
    #[test]
    fn a_rename_moves_the_value_and_leaves_no_source() {
        let d = dep(r#"{"removed":[],"renamed":{"old.key":"new.key"}}"#);
        let mut m = map(&[("old.key", "%s x%s")]);
        d.apply_to_map(&mut m);
        assert_eq!(m.get("new.key").map(String::as_str), Some("%s x%s"));
        assert!(!m.contains_key("old.key"));
    }

    /// The half that is easy to drop: an absent source **deletes** the target.
    #[test]
    fn a_rename_with_no_source_deletes_the_target() {
        let d = dep(r#"{"removed":[],"renamed":{"gone":"survivor"}}"#);
        let mut m = map(&[("survivor", "stale")]);
        d.apply_to_map(&mut m);
        assert!(!m.contains_key("survivor"));
    }

    /// Removal runs first, so a key that is both removed and a rename target
    /// comes back. 26.2 has three of these.
    #[test]
    fn removal_runs_before_the_renames() {
        let d = dep(r#"{"removed":["a"],"renamed":{"a.rebindable":"a"}}"#);
        let mut m = map(&[("a", "old"), ("a.rebindable", "new")]);
        d.apply_to_map(&mut m);
        assert_eq!(m.get("a").map(String::as_str), Some("new"));
    }

    #[test]
    fn removal_deletes() {
        let d = dep(r#"{"removed":["a","b"],"renamed":{}}"#);
        let mut m = map(&[("a", "1"), ("b", "2"), ("c", "3")]);
        d.apply_to_map(&mut m);
        assert_eq!(m.len(), 1);
        assert!(m.contains_key("c"));
    }

    /// The rename order is the file's, not a hash's.
    #[test]
    fn the_rename_order_is_the_files() {
        let d = dep(r#"{"removed":[],"renamed":{"z":"y","a":"b","m":"n"}}"#);
        let order: Vec<&str> = d.renamed().map(|(a, _)| a).collect();
        assert_eq!(order, ["z", "a", "m"]);
    }

    #[test]
    fn plain_substitution() {
        assert_eq!(format("%s x%s", &["Dirt", "64"]), "Dirt x64");
        assert_eq!(format("and %s more...", &["3"]), "and 3 more...");
        assert_eq!(format("no args here", &[]), "no args here");
    }

    /// The positional form, and the fact that it does **not** advance the
    /// implicit counter — so a mixed template reuses argument 0.
    #[test]
    fn positional_substitution() {
        assert_eq!(format("%2$s then %1$s", &["a", "b"]), "b then a");
        assert_eq!(format("%s %1$s %s", &["a", "b"]), "a a b");
    }

    /// `%%` is one literal percent, and a percent adjacent to a specifier
    /// still parses.
    #[test]
    fn a_literal_percent() {
        assert_eq!(format("100%%", &[]), "100%");
        assert_eq!(format("%s%%", &["50"]), "50%");
        assert_eq!(format("%%s", &["x"]), "%s");
    }

    /// Every error path renders the **raw pattern**, because `decompose`
    /// catches `TranslatableFormatException` and falls back to it.
    #[test]
    fn an_error_renders_the_unsubstituted_pattern() {
        // Unsupported format type.
        assert_eq!(format("%d apples", &["3"]), "%d apples");
        // Not enough arguments.
        assert_eq!(format("%s and %s", &["one"]), "%s and %s");
        // A stray percent in the prefix.
        assert_eq!(format("50% of %s", &["x"]), "50% of %s");
        // A trailing bare percent.
        assert_eq!(format("done %", &[]), "done %");
        // `%1$%` is not `%%`.
        assert_eq!(format("%1$%", &["x"]), "%1$%");
        // A one-based index of zero is out of range.
        assert_eq!(format("%0$s", &["x"]), "%0$s");
    }

    /// The pattern rewrite that runs at load, before any of the above.
    #[test]
    fn unsupported_specifiers_are_rewritten_at_load() {
        assert_eq!(rewrite_unsupported_formats("%d"), "%s");
        assert_eq!(rewrite_unsupported_formats("%1$d"), "%1$s");
        assert_eq!(rewrite_unsupported_formats("%.2f"), "%s");
        assert_eq!(rewrite_unsupported_formats("%03d"), "%s");
        assert_eq!(rewrite_unsupported_formats("%2$.1f done"), "%2$s done");
        // Untouched: the supported specifier, a literal percent, and a
        // trailing percent.
        assert_eq!(rewrite_unsupported_formats("%s %% 100%"), "%s %% 100%");
    }

    /// The rewrite is what makes a `%d` template work at all — the two steps
    /// only compose in this order.
    #[test]
    fn the_rewrite_and_the_substitution_compose() {
        assert_eq!(format(&rewrite_unsupported_formats("%d apples"), &["3"]), "3 apples");
    }

    /// A multi-byte value must survive both passes byte-for-byte.
    #[test]
    fn non_ascii_text_is_untouched() {
        assert_eq!(rewrite_unsupported_formats("caf\u{e9} \u{2014} %d"), "caf\u{e9} \u{2014} %s");
        assert_eq!(format("caf\u{e9} %s", &["\u{2014}"]), "caf\u{e9} \u{2014}");
    }

    /// `Language.getOrDefault(key)` defaults to the key, which is what a real
    /// client puts on screen for a missing translation.
    #[test]
    fn a_missing_key_resolves_to_itself() {
        let l = Language::from_json(
            r#"{"a.b":"Value"}"#,
            &DeprecatedTranslations::empty(),
        );
        assert_eq!(l.get("a.b"), Some("Value"));
        assert_eq!(l.get("nope"), None);
        assert_eq!(l.or_key("nope"), "nope");
        assert_eq!(l.translate("nope", &[]), "nope");
    }

    #[test]
    fn translate_substitutes() {
        let l = Language::from_json(
            r#"{"item.container.item_count":"%s x%s"}"#,
            &DeprecatedTranslations::empty(),
        );
        assert_eq!(
            l.translate("item.container.item_count", &["Dirt", "64"]),
            "Dirt x64"
        );
    }
}
