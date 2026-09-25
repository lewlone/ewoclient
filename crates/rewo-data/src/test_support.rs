//! Support for tests that need the local 26.2 assets (datagen report,
//! decompile, client jar, unpacked asset store) and so cannot run everywhere.
//!
//! Such a test calls [`skip_test!`](crate::skip_test) and returns when its
//! input is missing. Normally that prints a `SKIP:` line; with
//! `REWO_REQUIRE_ASSETS=1` it panics instead, so a full local run can prove
//! that nothing was silently skipped.

/// Record a skipped test; panics when `REWO_REQUIRE_ASSETS=1`.
pub fn skip(reason: &str) {
    if std::env::var_os("REWO_REQUIRE_ASSETS").is_some_and(|v| v == "1") {
        panic!("REWO_REQUIRE_ASSETS=1 but the test would skip: {reason}");
    }
    eprintln!("SKIP: {reason}");
}

/// `skip_test!("no datagen report at {}", path.display())` — see the module docs.
#[macro_export]
macro_rules! skip_test {
    ($($arg:tt)*) => {
        $crate::test_support::skip(&format!($($arg)*))
    };
}
