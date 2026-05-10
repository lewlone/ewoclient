//! Library + argument rule evaluator.
//!
//! Mojang's per-version manifests attach `rules` blocks to libraries (and
//! some game arguments) to gate them on OS, arch, and feature flags. The
//! evaluator walks the rules in order; the last matching rule wins.
//!
//! Reference: <https://wiki.vg/Game_files#Rules>
//!
//! Examples:
//!   - LWJGL has separate native bundles for windows/linux/osx; each
//!     bundle's rule narrows to its OS.
//!   - macOS-only patches have `disallow` rules for windows/linux.
//!   - Some args are gated on features like `is_demo_user` or
//!     `has_custom_resolution`; we treat all features as `false` for
//!     now (no demo mode, no custom resolution UI).

use crate::versions::per_version::{Rule, RuleAction};

/// Decide whether a library/arg with the given rules applies on this
/// host. Empty `rules` → always applies.
///
/// Algorithm:
///   1. Start as "doesn't apply".
///   2. For each rule in order: if its conditions match the host, set
///      "applies" iff the rule action is `allow`.
///   3. Final state at end of iteration is the answer.
pub fn rules_pass(rules: &[Rule]) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut applies = false;
    for rule in rules {
        if rule_matches_host(rule) {
            applies = matches!(rule.action, RuleAction::Allow);
        }
    }
    applies
}

fn rule_matches_host(rule: &Rule) -> bool {
    if let Some(os) = &rule.os {
        if let Some(name) = &os.name {
            if !os_name_matches(name) {
                return false;
            }
        }
        if let Some(arch) = &os.arch {
            if !os_arch_matches(arch) {
                return false;
            }
        }
        // `os.version` is a regex against host version; we don't bother
        // — it's only used for ancient macOS quirks we'll never hit.
    }
    if let Some(features) = &rule.features {
        // We only set the feature → required-true if our host has the
        // feature enabled. Currently no features are enabled, so any
        // rule that requires `feature: true` doesn't match.
        for (key, want) in features {
            let host_has = host_feature(key);
            if host_has != *want {
                return false;
            }
        }
    }
    true
}

fn os_name_matches(name: &str) -> bool {
    // Mojang uses: "windows", "osx", "linux".
    let host = std::env::consts::OS;
    match name {
        "windows" => host == "windows",
        "osx" | "mac-os" | "macos" => host == "macos",
        "linux" => host == "linux",
        other => host == other,
    }
}

fn os_arch_matches(arch: &str) -> bool {
    // Mojang occasionally sets "x86" for 32-bit-only libs; rare. Modern
    // hosts are x86_64 / aarch64. `std::env::consts::ARCH` returns
    // "x86_64", "aarch64", "x86", etc.
    let host = std::env::consts::ARCH;
    match arch {
        "x86" => host == "x86",
        "x86_64" => host == "x86_64",
        "arm64" | "aarch64" => host == "aarch64",
        other => host == other,
    }
}

fn host_feature(_key: &str) -> bool {
    // No features on by default. If we ever ship a `--demo` flag, set
    // `is_demo_user` true here. For now everything is false.
    false
}

/// Mojang's classifier key for the host's native bundle. Used by legacy
/// (≤1.12) library entries that store natives in `downloads.classifiers`
/// keyed by an OS-specific string.
///
/// e.g. `natives-windows`, `natives-linux`, `natives-osx`.
pub fn host_natives_classifier() -> &'static str {
    match std::env::consts::OS {
        "windows" => "natives-windows",
        "macos" => "natives-osx",
        "linux" => "natives-linux",
        _ => "natives-linux",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow_rule(os_name: Option<&str>) -> Rule {
        Rule {
            action: RuleAction::Allow,
            os: os_name.map(|n| crate::versions::per_version::OsCondition {
                name: Some(n.to_string()),
                arch: None,
                version: None,
            }),
            features: None,
        }
    }

    fn disallow_rule(os_name: &str) -> Rule {
        Rule {
            action: RuleAction::Disallow,
            os: Some(crate::versions::per_version::OsCondition {
                name: Some(os_name.to_string()),
                arch: None,
                version: None,
            }),
            features: None,
        }
    }

    #[test]
    fn empty_rules_pass() {
        assert!(rules_pass(&[]));
    }

    #[test]
    fn allow_all_then_disallow_macos() {
        // The classic LWJGL-ish pattern: allow on all, except macOS.
        let rules = vec![allow_rule(None), disallow_rule("osx")];
        let host = std::env::consts::OS;
        if host == "macos" {
            assert!(!rules_pass(&rules));
        } else {
            assert!(rules_pass(&rules));
        }
    }
}
