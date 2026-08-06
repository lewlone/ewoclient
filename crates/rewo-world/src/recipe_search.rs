//! The recipe book's search (M99).
//!
//! `updateCollections`' second stage — the one M93z modelled as
//! `matches_search` and left unfed:
//!
//! ```java
//! String searchTarget = this.searchBox.getValue();
//! if (!searchTarget.isEmpty()) {
//!    var set = new ObjectLinkedOpenHashSet<>(connection.searchTrees().recipes()
//!        .search(searchTarget.toLowerCase(Locale.ROOT)));
//!    collection.removeIf(e -> !set.contains(e));
//! }
//! ```
//!
//! # It is a substring match, and the suffix array is not needed for it
//!
//! Vanilla indexes each collection into a `SuffixArray`, which adds **every
//! suffix** of every indexed string — so `search(q)` returns the elements where
//! `q` occurs anywhere in an indexed string. The array exists to make that fast
//! and to return matches in a defined order.
//!
//! **Neither property is used here.** The result is poured into a set and read
//! only through `contains`, and the surviving collections keep the order they
//! already had (`removeIf`, not a re-sort). So a plain `contains` is exactly
//! equivalent for this consumer, and is what this transcribes. Building a
//! suffix array would be faster on a very large book and would not change a
//! single answer — recorded so a future reader knows the simplification was
//! measured against the consumer rather than assumed.
//!
//! # Two indexes, and a colon picks between them
//!
//! `FullTextSearchTree` carries a **name** index (the result items' tooltip
//! lines) and an **id** index (their registry keys), and `IdSearchTree.search`
//! dispatches on the first `:` in the query:
//!
//! * no colon → **names only**. A colon-less query does *not* search ids, which
//!   is easy to get wrong because the tree holds both.
//! * a colon → `namespace ∩ (path ∪ name)`, with both halves **trimmed**.
//!
//! That asymmetry is the whole reason the two indexes are kept apart.

/// One collection's searchable text (M99).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchEntry {
    /// `getTooltipLines` over the result items, lowercased.
    ///
    /// For Rewo this is the display **name** of each result item and nothing
    /// else — a tooltip's other lines come from components, and a recipe's
    /// result arrives as a bare item id with none. So it is exact here rather
    /// than an approximation, and would stop being exact the day results carry
    /// components.
    pub names: Vec<String>,
    /// `(namespace, path)` per result item, lowercased.
    pub ids: Vec<(String, String)>,
}

impl SearchEntry {
    fn name_matches(&self, q: &str) -> bool {
        self.names.iter().any(|n| n.contains(q))
    }

    fn path_matches(&self, q: &str) -> bool {
        self.ids.iter().any(|(_, p)| p.contains(q))
    }

    fn namespace_matches(&self, q: &str) -> bool {
        self.ids.iter().any(|(ns, _)| ns.contains(q))
    }
}

/// Whether a collection survives the search stage.
///
/// **An empty query is not a match-everything query** — vanilla skips the whole
/// stage rather than running it with an empty string, and the two differ: an
/// empty string is a substring of everything, so a collection with *no*
/// searchable text at all would be dropped by a match-everything reading and
/// kept by the skip. Modelled as the skip, which is what the code does.
pub fn matches(entry: &SearchEntry, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    match query.find(':') {
        None => entry.name_matches(query),
        Some(colon) => {
            let namespace = query[..colon].trim();
            let path = query[colon + 1..].trim();
            entry.namespace_matches(namespace)
                && (entry.path_matches(path) || entry.name_matches(path))
        }
    }
}

/// `searchBox.getValue().toLowerCase(Locale.ROOT)`.
///
/// `Locale.ROOT` rather than the default locale — the same reason M93z records
/// for the query it already lowercased: a Turkish user's dotless ı must not
/// break a search for "iron".
pub fn normalize(raw: &str) -> String {
    raw.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(names: &[&str], ids: &[(&str, &str)]) -> SearchEntry {
        SearchEntry {
            names: names.iter().map(|s| s.to_lowercase()).collect(),
            ids: ids
                .iter()
                .map(|(a, b)| (a.to_lowercase(), b.to_lowercase()))
                .collect(),
        }
    }

    fn sword() -> SearchEntry {
        e(&["Diamond Sword"], &[("minecraft", "diamond_sword")])
    }

    /// An empty query SKIPS the stage rather than matching everything, and the
    /// two differ for an entry with no searchable text.
    #[test]
    fn an_empty_query_skips_the_stage_rather_than_matching_everything() {
        assert!(matches(&sword(), ""));
        // An entry with nothing to search is kept by the skip and would be
        // dropped by a match-everything reading, since `"".contains` over an
        // empty list is false.
        let blank = SearchEntry::default();
        assert!(matches(&blank, ""));
        assert!(!matches(&blank, "x"));
    }

    /// A substring anywhere in the name matches — the suffix array's semantics.
    #[test]
    fn a_name_matches_on_any_substring_not_a_prefix() {
        assert!(matches(&sword(), "diamond"));
        assert!(matches(&sword(), "sword"), "not just a prefix");
        assert!(matches(&sword(), "ond sw"), "across the space, too");
        assert!(!matches(&sword(), "gold"));
    }

    /// A colon-less query searches NAMES ONLY, though the tree holds ids too.
    #[test]
    fn a_query_without_a_colon_does_not_search_ids() {
        // An entry whose id says "oak" and whose name does not.
        let x = e(&["Wooden Plank"], &[("minecraft", "oak_planks")]);
        assert!(matches(&x, "plank"), "the name matches");
        assert!(!matches(&x, "oak"), "the ID is not searched without a colon");
        // ...and with a colon it is.
        assert!(matches(&x, "minecraft:oak"));
    }

    /// A colon means `namespace ∩ (path ∪ name)` — the namespace must match, and
    /// then EITHER the path or the name.
    #[test]
    fn a_colon_intersects_the_namespace_with_the_path_or_the_name() {
        let x = e(&["Wooden Plank"], &[("minecraft", "oak_planks")]);
        assert!(matches(&x, "minecraft:oak"), "namespace and path");
        // The NAME half, isolated: "wooden" is in the display name and NOT in
        // the id's path. `plank` would not do — it is a substring of
        // `oak_planks` too, so a mutation dropping the name half survived it.
        assert!(matches(&x, "minecraft:wooden"), "namespace and NAME alone");
        assert!(!x.path_matches("wooden"), "and the path really lacks it");
        assert!(!matches(&x, "mymod:oak"), "the namespace fails");
        assert!(!matches(&x, "minecraft:gold"), "neither path nor name");
        // The namespace is a substring match too, so a partial one works.
        assert!(matches(&x, "mine:oak"));
    }

    /// Both halves are TRIMMED, so a query typed with spaces round the colon
    /// still works.
    #[test]
    fn both_halves_of_a_colon_query_are_trimmed() {
        let x = e(&["Wooden Plank"], &[("minecraft", "oak_planks")]);
        assert!(matches(&x, "minecraft : oak"));
        assert!(matches(&x, "  minecraft:oak  ".trim()));
        // Without the trim the leading space would be part of the namespace and
        // fail, which is what makes this load-bearing rather than cosmetic.
        assert!(matches(&x, "minecraft: oak"));
    }

    /// The FIRST colon splits, so a second one lands in the path.
    #[test]
    fn the_first_colon_splits_and_later_ones_do_not() {
        let x = e(&["A"], &[("ns", "a:b")]);
        assert!(matches(&x, "ns:a:b"));
        assert!(!matches(&x, "ns:a:c"));
    }

    /// An empty namespace matches nothing, because no id has an empty
    /// namespace... except that `"".contains` is true for every string, so it
    /// matches ALL of them. Vanilla's suffix array behaves the same way: an
    /// empty search string matches every indexed element.
    #[test]
    fn a_bare_colon_matches_on_the_path_alone() {
        let x = e(&["Wooden Plank"], &[("minecraft", "oak_planks")]);
        assert!(matches(&x, ":oak"), "an empty namespace matches any");
        assert!(!matches(&x, ":gold"));
    }

    /// Several result items: any one of them matching is enough, which is what
    /// `flatMap` over the collection's recipes gives.
    #[test]
    fn any_result_item_matching_is_enough() {
        let both = e(
            &["Diamond Sword", "Golden Sword"],
            &[("minecraft", "diamond_sword"), ("minecraft", "golden_sword")],
        );
        assert!(matches(&both, "golden"));
        assert!(matches(&both, "diamond"));
        assert!(matches(&both, "sword"));
        assert!(!matches(&both, "iron"));
    }

    #[test]
    fn the_query_is_lowercased_with_the_root_locale() {
        assert_eq!(normalize("Diamond SWORD"), "diamond sword");
        // The entry's text is lowercased at index time, so a mixed-case query
        // only matches once normalized — which is why the caller must do it.
        assert!(matches(&sword(), &normalize("DIAMOND")));
        assert!(!matches(&sword(), "DIAMOND"));
    }
}
