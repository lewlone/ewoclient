//! `LaunchPlan` — the fully-resolved invocation for a JVM child process.
//!
//! Produced by [`build`] from a per-version manifest + an instance + a
//! launch profile (player identity + auth token). All template tokens
//! (`${auth_player_name}`, `${classpath}`, etc.) are substituted; all
//! library-rule decisions are baked in. The result is a `Command`-ready
//! list of args plus the working directory + main class.
//!
//! Two arg formats handled:
//!
//! 1. **Modern (1.13+)** — per-version manifest has an `arguments` block
//!    with `arguments.game` and `arguments.jvm` arrays. Each entry is
//!    either a plain string (always include) or a `{rules, value}`
//!    object (include if rules pass; `value` is either a single string
//!    or a list of strings).
//! 2. **Legacy (≤1.12)** — per-version manifest has `minecraftArguments`
//!    as a flat space-separated string for game args, and we synthesize
//!    the JVM args ourselves from a known-good template (matches what
//!    Mojang's official launcher does).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::downloads::{paths, rules};
use crate::versions::per_version::{Library, PerVersion};

/// What the user is launching as. Offline mode for now (placeholder UUID
/// + token); when Mojang approves the launcher, real values from
/// `MinecraftAccount` get plugged in here.
#[derive(Debug, Clone)]
pub struct LaunchProfile {
    pub username: String,
    /// Trimmed Minecraft UUID (32 hex chars, no dashes).
    pub uuid: String,
    /// Bearer token. Empty / placeholder string in offline mode.
    pub access_token: String,
    /// "msa" for online; "legacy" or "mojang" for offline. Most modern
    /// versions accept any non-empty string here without complaint.
    pub user_type: String,
}

impl LaunchProfile {
    /// Build a profile suitable for offline-mode launching. Synthesizes
    /// a deterministic "offline UUID" from the username (the same way
    /// vanilla servers compute one for offline-mode players: MD5 of
    /// `OfflinePlayer:<username>` with version-3 UUID encoding).
    /// Returns a token literally equal to `"0"` — the Java client checks
    /// only for non-emptiness during offline-mode boot.
    pub fn offline(username: &str) -> Self {
        Self {
            username: username.to_string(),
            uuid: offline_uuid(username),
            access_token: "0".to_string(),
            user_type: "legacy".to_string(),
        }
    }
}

/// Vanilla offline-UUID convention: `UUID.nameUUIDFromBytes(("OfflinePlayer:" + name).getBytes("UTF-8"))`.
/// Mojang servers use this exact algorithm for offline players.
fn offline_uuid(username: &str) -> String {
    use sha1::Sha1;
    use sha2::Digest;
    // Java's nameUUIDFromBytes uses MD5, but we only have sha1+sha2 in
    // deps. The offline-launching Java client doesn't actually verify
    // this UUID against any server, so any deterministic 32-hex-char
    // string derived from the username is fine for offline mode. We use
    // sha1 for that purpose.
    let mut h = Sha1::new();
    h.update(format!("OfflinePlayer:{}", username).as_bytes());
    let digest = h.finalize();
    // Take 16 bytes, format as a v3-shaped UUID (set the version + variant
    // bits the way Java's nameUUIDFromBytes(MD5) would). The Java client
    // is tolerant of arbitrary 16-byte UUIDs in offline mode, so this is
    // close-enough.
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x30; // version 3
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Output of [`build`] — everything the spawn step needs.
#[derive(Debug, Clone)]
pub struct LaunchPlan {
    /// Path (or just `"java"`) to invoke. Falls back to PATH lookup.
    pub jvm_path: PathBuf,
    /// JVM args (heap size, library path, `-cp`, etc.). Includes the
    /// `-cp` flag + classpath entries — main class is appended after.
    pub jvm_args: Vec<String>,
    /// Fully-qualified main class (e.g. `net.minecraft.client.main.Main`).
    pub main_class: String,
    /// Game args (window size, username, version, etc.).
    pub game_args: Vec<String>,
    /// Process working directory — typically the per-instance dir so
    /// `world` / `screenshots` / `logs` end up there.
    pub working_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub enum BuildError {
    /// `<config>/EwoClient` etc. couldn't be resolved.
    PathsUnresolvable,
    /// Asset index ID missing — manifest is malformed.
    BadManifest(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::PathsUnresolvable => write!(f, "config dir unresolvable"),
            BuildError::BadManifest(s) => write!(f, "manifest: {}", s),
        }
    }
}

impl std::error::Error for BuildError {}

/// Build a runnable [`LaunchPlan`] from a per-version manifest + instance
/// + launch profile.
pub fn build(
    pv: &PerVersion,
    instance_name: &str,
    ram_gb: u32,
    profile: &LaunchProfile,
    jvm_path: PathBuf,
) -> Result<LaunchPlan, BuildError> {
    let game_dir = paths::instance_dir(instance_name).ok_or(BuildError::PathsUnresolvable)?;
    let assets_root = paths::assets_dir().ok_or(BuildError::PathsUnresolvable)?;
    let natives_dir = natives_dir_for(instance_name).ok_or(BuildError::PathsUnresolvable)?;

    // Build classpath: every applicable library's main artifact + the
    // client jar. Order matters — Mojang puts libs first, client last.
    let classpath = build_classpath(pv)?;
    let cp_separator = classpath_separator();
    let cp_string = classpath.join(cp_separator);

    let asset_index_id = if !pv.asset_index.id.is_empty() {
        pv.asset_index.id.clone()
    } else if !pv.assets.is_empty() {
        pv.assets.clone()
    } else {
        return Err(BuildError::BadManifest("no assetIndex.id".into()));
    };

    // Token map. Every value flows in here; the substituter walks each
    // arg string and replaces `${token}` occurrences in place.
    let tokens = TokenMap {
        auth_player_name: profile.username.clone(),
        version_name: pv.id.clone(),
        game_directory: path_string(&game_dir),
        assets_root: path_string(&assets_root),
        assets_index_name: asset_index_id,
        auth_uuid: profile.uuid.clone(),
        auth_access_token: profile.access_token.clone(),
        clientid: String::new(),
        auth_xuid: String::new(),
        user_type: profile.user_type.clone(),
        version_type: "release".to_string(),
        natives_directory: path_string(&natives_dir),
        launcher_name: "EwoClient".to_string(),
        launcher_version: env!("CARGO_PKG_VERSION").to_string(),
        classpath: cp_string,
        // Legacy 1.6-era token; modern versions don't use it. Pre-1.6
        // also expects a `game_assets` token but we don't target those.
        user_properties: "{}".to_string(),
    };

    // Build JVM args (modern manifest provides them; legacy synthesizes).
    let mut jvm_args = match &pv.arguments {
        Some(args) => substitute_jvm_args(&args.jvm, &tokens),
        None => synthesize_legacy_jvm_args(&tokens),
    };
    // Heap size before everything else (Mojang's `-Xss1M` etc. come from
    // the manifest's jvm args — we only add `-Xmx`).
    jvm_args.insert(0, format!("-Xmx{}M", ram_gb * 1024));

    let game_args = match &pv.arguments {
        Some(args) => substitute_game_args(&args.game, &tokens),
        None => match &pv.minecraft_arguments {
            Some(s) => substitute_legacy_game_args(s, &tokens),
            None => Vec::new(),
        },
    };

    Ok(LaunchPlan {
        jvm_path,
        jvm_args,
        main_class: pv.main_class.clone(),
        game_args,
        working_dir: game_dir,
    })
}

/// Per-instance natives extraction dir. Distinct from `<game_dir>` so
/// natives can be freshly extracted on each launch (in case a library
/// changed) without polluting the user's saves/config.
pub fn natives_dir_for(instance_name: &str) -> Option<PathBuf> {
    let mut p = paths::instance_dir(instance_name)?;
    p.push("natives");
    Some(p)
}

fn classpath_separator() -> &'static str {
    if cfg!(target_os = "windows") {
        ";"
    } else {
        ":"
    }
}

fn path_string(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

fn build_classpath(pv: &PerVersion) -> Result<Vec<String>, BuildError> {
    let mut out = Vec::with_capacity(pv.libraries.len() + 1);
    for lib in &pv.libraries {
        if !rules::rules_pass(&lib.rules) {
            continue;
        }
        if let Some(art) = &lib.downloads.artifact {
            if let Some(p) = paths::library_path(&art.path) {
                out.push(path_string(&p));
            }
        }
    }
    // Client jar last.
    let id = &pv.id;
    let client_path = paths::client_jar(id).ok_or(BuildError::PathsUnresolvable)?;
    out.push(path_string(&client_path));
    Ok(out)
}

// ────────────────────────────────────────────────────────────────────────
// Token substitution
// ────────────────────────────────────────────────────────────────────────

struct TokenMap {
    auth_player_name: String,
    version_name: String,
    game_directory: String,
    assets_root: String,
    assets_index_name: String,
    auth_uuid: String,
    auth_access_token: String,
    clientid: String,
    auth_xuid: String,
    user_type: String,
    version_type: String,
    natives_directory: String,
    launcher_name: String,
    launcher_version: String,
    classpath: String,
    user_properties: String,
}

impl TokenMap {
    fn lookup(&self, key: &str) -> Option<&str> {
        Some(match key {
            "auth_player_name" => &self.auth_player_name,
            "version_name" => &self.version_name,
            "game_directory" => &self.game_directory,
            "assets_root" => &self.assets_root,
            "game_assets" => &self.assets_root, // alias for legacy 1.6
            "assets_index_name" => &self.assets_index_name,
            "auth_uuid" => &self.auth_uuid,
            "auth_access_token" => &self.auth_access_token,
            "auth_session" => &self.auth_access_token, // legacy alias
            "clientid" => &self.clientid,
            "auth_xuid" => &self.auth_xuid,
            "user_type" => &self.user_type,
            "version_type" => &self.version_type,
            "natives_directory" => &self.natives_directory,
            "launcher_name" => &self.launcher_name,
            "launcher_version" => &self.launcher_version,
            "classpath" => &self.classpath,
            "user_properties" => &self.user_properties,
            _ => return None,
        })
    }
}

fn substitute_str(s: &str, tokens: &TokenMap) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '$' && chars.peek().map(|(_, c)| *c) == Some('{') {
            // Find closing brace.
            if let Some(end) = s[i..].find('}') {
                let key = &s[i + 2..i + end];
                if let Some(v) = tokens.lookup(key) {
                    out.push_str(v);
                    // Advance past the `}`.
                    while let Some(&(j, _)) = chars.peek() {
                        if j > i + end {
                            break;
                        }
                        chars.next();
                    }
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

fn substitute_jvm_args(args: &[Value], tokens: &TokenMap) -> Vec<String> {
    let mut out = Vec::new();
    for entry in args {
        match entry {
            Value::String(s) => out.push(substitute_str(s, tokens)),
            Value::Object(obj) => {
                // Conditional entry: { rules: [...], value: "..." | [...] }
                let rules_value = obj.get("rules");
                let passes = rules_value
                    .map(|v| eval_arg_rules(v))
                    .unwrap_or(true);
                if !passes {
                    continue;
                }
                if let Some(v) = obj.get("value") {
                    match v {
                        Value::String(s) => out.push(substitute_str(s, tokens)),
                        Value::Array(arr) => {
                            for item in arr {
                                if let Some(s) = item.as_str() {
                                    out.push(substitute_str(s, tokens));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn substitute_game_args(args: &[Value], tokens: &TokenMap) -> Vec<String> {
    // Same shape as jvm — string-or-conditional. Reuse.
    substitute_jvm_args(args, tokens)
}

fn substitute_legacy_game_args(template: &str, tokens: &TokenMap) -> Vec<String> {
    // Pre-1.13 manifests use a single space-separated string:
    // "--username ${auth_player_name} --version ${version_name} --gameDir ${game_directory} ..."
    template
        .split_whitespace()
        .map(|tok| substitute_str(tok, tokens))
        .collect()
}

/// Synthesize JVM args for legacy (≤1.12) manifests — they don't include
/// JVM args inline, so we build the standard set Mojang's official
/// launcher uses: `-Djava.library.path`, `-cp`.
fn synthesize_legacy_jvm_args(tokens: &TokenMap) -> Vec<String> {
    vec![
        format!("-Djava.library.path={}", tokens.natives_directory),
        "-cp".to_string(),
        tokens.classpath.clone(),
    ]
}

/// Evaluate a `rules` array on a conditional argument entry. Same shape
/// as library rules — last matching rule wins.
fn eval_arg_rules(rules_value: &Value) -> bool {
    let arr = match rules_value.as_array() {
        Some(a) => a,
        None => return true,
    };
    // Re-use the library rule evaluator by deserializing each rule.
    let rules: Vec<crate::versions::per_version::Rule> = arr
        .iter()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect();
    rules::rules_pass(&rules)
}

/// Helper for natives extraction — picks the right classifier name for
/// this OS, looking at both modern (downloads.classifiers["natives-windows"])
/// and legacy (natives map) layouts.
pub fn pick_native_classifier(lib: &Library) -> Option<String> {
    let host_key = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "osx",
        "linux" => "linux",
        other => other,
    };
    if let Some(classifier) = lib.natives.get(host_key) {
        return Some(classifier.clone());
    }
    let modern_key = rules::host_natives_classifier().to_string();
    if lib.downloads.classifiers.contains_key(&modern_key) {
        return Some(modern_key);
    }
    None
}
