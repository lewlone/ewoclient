//! The Java mod and this crate must agree on which `profiles.toml` active name
//! is a usable profile folder and what that name is. The mod used to join the
//! raw string while `ewo_core::pvp::profile_file_in` refused unsafe names, so
//! a hand-edited `active = "../.."` meant different directories on the two
//! sides. `EwoProfileName.java` now mirrors the Rust rule; these tests keep it
//! from drifting.
//!
//! The pin that matters runs the real Java: it compiles `EwoProfileName.java`
//! with the tiny driver in `java_probe/` and compares its answer with Rust's
//! for a table of awkward names. It needs a JDK (`JAVA_HOME` or `javac` on
//! `PATH`); without one it skips loudly, and fails under
//! `REWO_REQUIRE_ASSETS=1` like the other local-asset tests. The source pins
//! below it always run.

use crate::java_mirror::{get, int_constants, read_java};
use ewo_core::pvp::{active_profile_from_toml, profile_file_in, MAX_PROFILE_NAME_LEN};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn safe_in_rust(name: &str) -> bool {
    profile_file_in(Path::new("cfg"), name, "pvp.toml").is_some()
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(h: &str) -> String {
    let bytes: Vec<u8> = (0..h.len() / 2)
        .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap())
        .collect();
    String::from_utf8(bytes).unwrap()
}

fn profile_names() -> Vec<String> {
    let mut v: Vec<String> = [
        "Default",
        "PvP 2",
        "a b",
        "",
        " ",
        " Default",
        "Default ",
        "\u{a0}x",
        "x\u{a0}",
        "\u{3000}",
        "\u{85}x",
        "x\u{2007}",
        "x\u{2028}",
        "\u{200b}x",
        "\u{feff}x",
        "..",
        ".",
        "a.b",
        "a..b",
        "../x",
        "../..",
        "..\\..\\evil",
        "trailing.",
        ".hidden",
        "con",
        "CON",
        "Con.txt",
        "CON .x",
        "nul",
        "NUL.txt",
        "aux",
        "prn",
        "com1",
        "COM1",
        "COM9.log",
        "com0",
        "COM10",
        "lpt9",
        "LPT0",
        "lptA",
        "co",
        "\u{ff23}\u{ff2f}\u{ff2e}",
        "a/b",
        "a\\b",
        "a:b",
        "C:",
        "a*",
        "a?",
        "a\"",
        "a<",
        "a>",
        "a|",
        "a\u{0}b",
        "a\tb",
        "a\nb",
        "\u{7f}",
        "\u{80}",
        "\u{9f}",
        "\u{a1}",
        "\u{e9}",
        "\u{65e5}\u{672c}\u{8a9e}",
        "\u{1f600} profile",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for n in [MAX_PROFILE_NAME_LEN, MAX_PROFILE_NAME_LEN + 1] {
        v.push("x".repeat(n));
        // Astral characters count once as chars, twice as UTF-16 units.
        v.push("\u{1f600}".repeat(n));
    }
    v
}

fn toml_texts() -> Vec<&'static str> {
    vec![
        "",
        "active = \"Cinematic\"\n",
        "active=Cinematic",
        "  active  =  \"A B\"  ",
        "activeX = \"y\"",
        "active_profile = \"y\"\nactive = \"z\"",
        "[section]\nactive = \"q\"",
        "active = \"\"\"\"",
        "active = '\"x\"'",
        "active = ..\\..\\evil",
        "active",
        "active =",
        "active = \"\"",
        "\u{a0}active = \"nb\"",
        "active\u{a0}= \"nb2\"",
        "x = 1\r\nactive = \"crlf\"\r\n",
        "active = \"a\" # c",
        "# active = \"c\"\nactive = \"d\"",
        "active = \"\u{e9}\u{65e5}\"",
        "inactive = \"x\"",
        "active = \"a\" \"b\"",
        "active = \" padded \"",
        "active = \"\u{a0}nb \"",
    ]
}

/// A JDK tool: `$JAVA_HOME/bin/<name>` if that exists, else `<name>` on PATH.
fn tool(name: &str) -> PathBuf {
    if let Some(home) = std::env::var_os("JAVA_HOME").filter(|h| !h.is_empty()) {
        let exe = Path::new(&home).join("bin").join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_string()
        });
        if exe.is_file() {
            return exe;
        }
    }
    PathBuf::from(name)
}

#[test]
fn java_profile_rule_matches_rust() {
    let javac = tool("javac");
    if Command::new(&javac)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        if std::env::var_os("REWO_REQUIRE_ASSETS").is_some() {
            panic!("no JDK (javac) found; set JAVA_HOME");
        }
        eprintln!("SKIP: no JDK (javac) found; set JAVA_HOME to run the Java profile-rule pin");
        return;
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mod_src =
        root.join("../../ingame-mod/src/main/java/dev/lewlone/ewohud/EwoProfileName.java");
    let probe_src = root.join("src/java_probe/ProfileProbe.java");
    let out = std::env::temp_dir().join(format!("ewo-jni-profile-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let compiled = Command::new(&javac)
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-d")
        .arg(&out)
        .arg(&mod_src)
        .arg(&probe_src)
        .output()
        .expect("run javac");
    assert!(
        compiled.status.success(),
        "javac failed:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let names = profile_names();
    let texts = toml_texts();
    let mut input = String::new();
    for n in &names {
        input.push_str(&format!("S {}\n", hex(n)));
    }
    for t in &texts {
        input.push_str(&format!("A {}\n", hex(t)));
    }
    let mut child = Command::new(tool("java"))
        .arg("-cp")
        .arg(&out)
        .arg("ProfileProbe")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run java");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let result = child.wait_with_output().expect("java output");
    let _ = std::fs::remove_dir_all(&out);
    assert!(
        result.status.success(),
        "java failed:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8(result.stdout).unwrap();
    let mut lines = stdout.lines();

    for n in &names {
        let java = lines.next().expect("one answer per name");
        let want = format!("S {}", u8::from(safe_in_rust(n)));
        assert_eq!(java, want, "profile name {n:?}: Java vs Rust disagree");
    }
    for t in &texts {
        let java = lines.next().expect("one answer per toml text");
        let java = unhex(java.strip_prefix("A ").expect("A answer"));
        assert_eq!(
            java,
            active_profile_from_toml(t),
            "profiles.toml {t:?}: Java vs Rust disagree on the active profile"
        );
    }
    assert!(lines.next().is_none());
}

/// Source pins that run without a JDK: the length bound is one number, and
/// the mod's profile paths all go through the shared rule instead of joining
/// the raw string.
#[test]
fn java_profile_sources_use_the_shared_rule() {
    let consts = int_constants(&read_java("EwoProfileName.java"));
    assert_eq!(
        get(&consts, "MAX_PROFILE_NAME_LEN"),
        MAX_PROFILE_NAME_LEN as i64,
        "EwoProfileName.MAX_PROFILE_NAME_LEN and ewo_core::pvp::MAX_PROFILE_NAME_LEN differ"
    );
    for file in ["pvp/EwoPvpModule.java", "pvp/EwoPvpConfig.java"] {
        let src = read_java(file);
        assert!(
            src.contains("EwoProfileName.profileFile("),
            "{file} must build pvp.toml's path with EwoProfileName.profileFile"
        );
        assert!(
            !src.contains("\"profiles\", profile"),
            "{file} joins an unvalidated profile name into a path"
        );
        assert!(
            src.contains("EwoProfileName.activeFromToml("),
            "{file} must parse profiles.toml with EwoProfileName.activeFromToml"
        );
    }
}

/// The dll is mapped by `System.load`, so the exit watchdog must be armable
/// from that point on, before the ABI check can refuse the bridge.
#[test]
fn exit_watchdog_is_armed_before_the_abi_check() {
    let src = read_java("EwoHudMod.java");
    let load = src.find("System.load(").expect("loads the dll");
    let kill = src
        .find("killPathLoaded = true;")
        .expect("arms the kill path");
    let hello = src
        .find("EwoHudNative.nativeHello()")
        .expect("calls nativeHello");
    assert!(
        load < kill && kill < hello,
        "killPathLoaded must be set between System.load and the ABI handshake"
    );
}
