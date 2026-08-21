//! The `*shot` witness-NAME namespace, checked (M167).
//!
//! **`REWO_PLAN.md` §0.0 listed this as one of the three most valuable open
//! items and its description was wrong in two ways**, both found by measuring
//! before building:
//!
//! * *"`Checker::record` dedups nothing"* implies one type. There are **30
//!   independent `Checker` definitions**, one per gate file, sharing no code —
//!   `record`'s body falls into three shapes (22 identical, 7 identical, 1
//!   unique). A dedup inside `record` is therefore a 30-file refactor, not the
//!   one-line change the item reads as.
//! * *"two witnesses sharing a name are counted twice and read as a pass"* is
//!   true of the mechanism and **there is no live instance**: all 37 gates were
//!   run and their emitted rows counted, in both `soundshot` configurations,
//!   and **zero** duplicate names are produced today.
//!
//! So this is hardening rather than a bug fix, and it buys the cheap half: the
//! commonest way to create a collision is to copy a `record(...)` call and
//! forget to rename it, which is a **source** property and is caught here at
//! `cargo test` time — the same reasoning that put
//! `REWO_PACKET_COVERAGE.md`'s check in `ids.rs` rather than in a `*shot` gate
//! (M74): a guard fires best on the event that causes the drift, and the event
//! here is someone editing a gate file.
//!
//! ## What this does NOT catch, stated rather than implied
//!
//! A name built at runtime. Several gates format theirs (`mobshot`'s per-mob
//! rows, `eventshot`'s `c3.head_rot_x@0.25`), and two such names can collide
//! without either appearing as a literal. Only a check inside `record` sees
//! those — that is the 30-file version, and it is the strictly stronger design.
//! The runtime sweep above is the current evidence that neither kind collides.
//!
//! ## The four exclusions are real, not noise
//!
//! Four files carry the same literal twice, and in every case the two calls are
//! in **mutually exclusive branches**, so no run emits both — which the runtime
//! sweep confirms. `soundshot`'s are the clearest: it grades a different set of
//! layers under `--features audio`, so one row name legitimately has two
//! definitions. They are listed by name below rather than skipped by a pattern,
//! so a *new* duplicate in the same file still fails.

/// Files whose duplicate literal names are branch-exclusive, with the count of
/// distinct names allowed to repeat. Keyed by file stem.
///
/// **A count, not a blanket exemption**: raising it is a deliberate edit, and a
/// fifth duplicate in `soundshot` fails until someone says why.
#[cfg(test)]
const BRANCH_EXCLUSIVE: &[(&str, usize)] = &[
    // `w8` is asserted once inside a bundle and once outside it.
    ("abilityshot_cmd", 1),
    // Per-frame keyframe rows, emitted by two different rig paths.
    ("eventshot_cmd", 7),
    // The default build grades layers w+s; `--features audio` adds d+m. Four
    // rows exist in both configurations with the same claim.
    ("soundshot_cmd", 4),
    // Two rows asserted for both the title and the subtitle path.
    ("titleshot_cmd", 2),
];

#[cfg(test)]
mod tests {
    use super::BRANCH_EXCLUSIVE;
    use std::collections::HashMap;

    /// Floors, so that "found nothing" cannot pass.
    ///
    /// M138a's rule: a check that turns a missing input into an empty result is
    /// green precisely on the machine where it stopped working. Both numbers
    /// are below what is present today (30 files, 1632 names) and above zero.
    const MIN_FILES: usize = 25;
    const MIN_NAMES: usize = 1200;

    /// Every `.record("name", ...)` literal in one source file.
    ///
    /// Deliberately a scan rather than a parse: the alternative is calling the
    /// gates, which needs a Vulkan device and the asset store.
    fn literal_names(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = src;
        while let Some(i) = rest.find(".record(") {
            rest = &rest[i + ".record(".len()..];
            let head = rest.trim_start();
            let Some(body) = head.strip_prefix('"') else {
                // A non-literal name — formatted at runtime. Invisible to this
                // check by construction; see the module doc.
                continue;
            };
            if let Some(end) = body.find('"') {
                out.push(body[..end].to_string());
            }
        }
        out
    }

    #[test]
    fn no_gate_file_defines_one_witness_name_twice() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let allow: HashMap<&str, usize> = BRANCH_EXCLUSIVE.iter().copied().collect();

        let mut files = 0usize;
        let mut names = 0usize;
        let mut faults = Vec::new();

        let entries = std::fs::read_dir(&dir).expect("crates/rewo-app/src must be readable");
        for e in entries {
            let path = e.expect("readable dir entry").path();
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            // The gates, and `dimensioncheck` which is one by another name.
            if !(stem.ends_with("shot_cmd") || stem == "dimensioncheck_cmd") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("readable gate source");
            let found = literal_names(&src);
            if found.is_empty() {
                continue;
            }
            files += 1;
            names += found.len();

            let mut counts: HashMap<&str, usize> = HashMap::new();
            for n in &found {
                *counts.entry(n.as_str()).or_default() += 1;
            }
            let repeated = counts.values().filter(|v| **v > 1).count();
            let allowed = allow.get(stem).copied().unwrap_or(0);
            if repeated > allowed {
                let mut which: Vec<&str> = counts
                    .iter()
                    .filter(|(_, v)| **v > 1)
                    .map(|(k, _)| *k)
                    .collect();
                which.sort_unstable();
                faults.push(format!(
                    "{stem}: {repeated} duplicated name(s), {allowed} allowed -> {which:?}"
                ));
            }
        }

        assert!(
            files >= MIN_FILES,
            "scanned only {files} gate files (floor {MIN_FILES}) — the scan found \
             nothing to grade, which is not the same as finding no duplicates"
        );
        assert!(
            names >= MIN_NAMES,
            "scanned only {names} witness names (floor {MIN_NAMES}) — see above"
        );
        assert!(
            faults.is_empty(),
            "a witness name is defined twice in one gate file. Every gate is \
             fail-closed on a COUNT, so a duplicate is counted twice and reads \
             as a pass while one claim goes ungraded:\n  {}",
            faults.join("\n  ")
        );
    }

    #[test]
    fn the_exclusions_are_still_needed_and_not_over_wide() {
        // An allow-list that outlives its reason is the failure mode of every
        // allow-list. This asserts each entry is EXACTLY right: too low and the
        // test above fails, too high and this one does — so a file that gets
        // its duplicates cleaned up cannot leave a standing exemption behind.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for (stem, allowed) in BRANCH_EXCLUSIVE {
            let src = std::fs::read_to_string(dir.join(format!("{stem}.rs")))
                .unwrap_or_else(|e| panic!("{stem}.rs must exist: {e}"));
            let mut counts: HashMap<String, usize> = HashMap::new();
            for n in literal_names(&src) {
                *counts.entry(n).or_default() += 1;
            }
            let repeated = counts.values().filter(|v| **v > 1).count();
            assert_eq!(
                repeated, *allowed,
                "{stem} has {repeated} duplicated witness name(s) but the \
                 exclusion says {allowed}"
            );
        }
    }

    #[test]
    fn the_scanner_sees_a_duplicate_when_there_is_one() {
        // The scanner is the instrument, so it gets its own witness: a check
        // that silently matched nothing would pass the whole file above.
        let src = r#"
            c.record("a.one", true, "x");
            c.record("b.two", true, "y");
            c.record("a.one", true, "z");
            c.record(format!("dyn.{i}"), true, "w");
        "#;
        let found = literal_names(src);
        assert_eq!(found, vec!["a.one", "b.two", "a.one"]);
        // The formatted one is absent, which is the documented blind spot.
        assert!(!found.iter().any(|n| n.starts_with("dyn")));
    }
}
