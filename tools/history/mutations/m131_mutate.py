"""M131's mutation harness — the audio model.

Copied from `tools/m125_mutate.py`, with two things added:

* the restore explicitly bumps the file's mtime, because a restore that
  preserves an older timestamp makes cargo skip the rebuild and silently grade
  the *mutated* binary on the next entry;
* a `SURVIVED` verdict is a legitimate expectation, used here to *measure* the
  composition-root gap rather than to hide it. The two `live_cmd.rs` call sites
  have no test seam anywhere — only `live --render-check` could see them — so
  the battery states that as an expectation and proves it.

Verdicts come from the EXIT CODE, never from a substring: M109 grepped for
witness names, saw `ok` everywhere, and missed that the command was red — and a
battery run against an already-red check reads KILLED for every entry. Hence
the baseline guard.

    python tools/m131_mutate.py <battery>
"""

import os
import subprocess
import sys

ROOT = "."


def run(cmd, timeout):
    try:
        p = subprocess.run(cmd, shell=True, cwd=ROOT, capture_output=True,
                           timeout=timeout, env=dict(os.environ))
        return p.returncode, (p.stdout + p.stderr).decode("utf-8", "replace")
    except subprocess.TimeoutExpired:
        return 124, "<timed out>"


def write(path, data):
    with open(path, "wb") as f:
        f.write(data)
    # cp/mv preserve the older timestamp; an explicit touch is what stops cargo
    # deciding the crate is unchanged and re-running the previous binary.
    os.utime(path, None)


def battery(name, check, timeout, mutations):
    print(f"=== {name} ===")
    print(f"    check: {check}")
    code, _ = run(check, timeout)
    if code != 0:
        print(f"    ABORT: the check is already red (exit {code}). "
              f"Every verdict below would read KILLED.")
        return 1
    print("    baseline green")

    bad = 0
    for path, old, new, expect, why in mutations:
        full = f"{ROOT}/{path}"
        with open(full, "rb") as f:
            orig = f.read()
        n = orig.decode("utf-8").count(old)
        if n != 1:
            print(f"  BAD ?         anchor matched {n} times, not 1 — SKIPPED: {why}")
            bad += 1
            continue
        try:
            write(full, orig.decode("utf-8").replace(old, new, 1).encode("utf-8"))
            code, _ = run(check, timeout)
        finally:
            write(full, orig)
        got = "KILLED" if code != 0 else "SURVIVED"
        ok = got == expect
        if not ok:
            bad += 1
        print(f"  {'ok ' if ok else 'BAD'} {got:9s} (want {expect:9s})  {why}")
    print(f"    {'ALL AS EXPECTED' if bad == 0 else str(bad) + ' UNEXPECTED'}")
    return bad


SI = "crates/rewo-net/src/sound_instance.rs"
SE = "crates/rewo-net/src/sound_engine.rs"
SEV = "crates/rewo-data/src/sound_events.rs"
SJ = "crates/rewo-data/src/sounds_json.rs"
LC = "crates/rewo-app/src/live_cmd.rs"

NET = "cargo test -q -p rewo-net --lib"
DATA = "cargo test -q -p rewo-data --lib"
APP = "cargo test -q -p rewo-app --bins"

BATTERIES = {
    "a": (
        "the instance model and its arithmetic",
        NET, 900,
        [
            (SI, "pub fn calculate_pitch(pitch: f32) -> f32 {",
                 "pub fn calculate_pitch(pitch: f32) -> f32 { // no-op control",
                 "SURVIVED", "NO-OP CONTROL: a comment"),
            (SI, "    instance_volume.max(1.0) * sound_attenuation_distance as f32",
                 "    instance_volume * sound_attenuation_distance as f32",
                 "KILLED", "no `max(_, 1)` — a quiet sound loses range too"),
            (SI, "        if source == SoundSource::Master {\n            self.slider(SoundSource::Master)\n        } else {",
                 "        if false {\n            self.slider(SoundSource::Master)\n        } else {",
                 "KILLED", "MASTER multiplied by itself"),
            (SI, "    if value < min {\n        min\n    } else if value > max {\n        max\n    } else {\n        value\n    }",
                 "    value.max(min).min(max)",
                 "KILLED", "Mth.clamp as min/max — NaN becomes silence"),
            (SI, "pub const PITCH_MIN: f32 = 0.5;",
                 "pub const PITCH_MIN: f32 = 0.25;",
                 "KILLED", "the pitch floor is not 0.5"),
            (SI, "pub const UI_DEFAULT_VOLUME: f32 = 0.25;",
                 "pub const UI_DEFAULT_VOLUME: f32 = 1.0;",
                 "KILLED", "forUI's two-arg overload does not quarter the volume"),
            (SI, "        SoundInstance {\n            volume,\n            pitch,\n            attenuation: Attenuation::None,\n            relative: true,\n            ..SoundInstance::bare(identifier, SoundSource::Ambient)\n        }",
                 "        SoundInstance {\n            volume: pitch,\n            pitch: volume,\n            attenuation: Attenuation::None,\n            relative: true,\n            ..SoundInstance::bare(identifier, SoundSource::Ambient)\n        }",
                 "KILLED", "forLocalAmbience read left to right (volume/pitch swapped)"),
            (SI, "    pub fn should_loop_automatically(&self) -> bool {\n        self.looping && !self.requires_manual_looping()",
                 "    pub fn should_loop_automatically(&self) -> bool {\n        self.looping",
                 "KILLED", "a delayed loop is also an automatic one"),
            (SI, "            x: ex as f32 as f64,\n            y: ey as f32 as f64,\n            z: ez as f32 as f64,",
                 "            x: ex,\n            y: ey,\n            z: ez,",
                 "KILLED", "EntityBoundSoundInstance's f32 narrowing dropped"),
            (SI, "            bx as f64 + 0.5,\n            by as f64 + 0.5,\n            bz as f64 + 0.5,",
                 "            bx as f64,\n            by as f64,\n            bz as f64,",
                 "KILLED", "the BlockPos overload does not centre on the block"),
            (SI, "            Binding::Fixed => true,\n            Binding::Entity(_) => !silent,",
                 "            Binding::Fixed => !silent,\n            Binding::Entity(_) => !silent,",
                 "KILLED", "a fixed sound can be silenced by an entity flag"),
            (SI, "        * mth_clamp(options.final_volume(source), VOLUME_MIN, VOLUME_MAX)\n        * gain",
                 "        * mth_clamp(options.final_volume(source), VOLUME_MIN, VOLUME_MAX)",
                 "KILLED", "the runtime category gain is dropped from the product"),
        ],
    ),
    "b": (
        "the channel budget, the OpenAL parameters and the play path",
        NET, 900,
        [
            (SE, "pub fn pool_sizes(total_channel_count: i32) -> (i32, i32) {",
                 "pub fn pool_sizes(total_channel_count: i32) -> (i32, i32) { // no-op control",
                 "SURVIVED", "NO-OP CONTROL: a comment"),
            (SE, "    let streaming = ((total_channel_count as f32).sqrt() as i32).clamp(2, 8);",
                 "    let streaming = ((total_channel_count as f32).sqrt().round() as i32).clamp(2, 8);",
                 "KILLED", "a rounding sqrt where the cast truncates"),
            (SE, "    let static_count = (total_channel_count - streaming).clamp(8, 255);",
                 "    let static_count = (total_channel_count - streaming).clamp(0, 255);",
                 "KILLED", "the static pool's lower bound of 8 dropped"),
            (SE, "        if *used >= limit {\n            return false;\n        }",
                 "        if *used > limit {\n            return false;\n        }",
                 "KILLED", "the pool admits one channel past its limit"),
            (SE, "pub const MIN_SOURCE_LIFETIME: i32 = 20;",
                 "pub const MIN_SOURCE_LIFETIME: i32 = 19;",
                 "KILLED", "the grace period is not 20 ticks"),
            (SE, "            if !l.handle_stopped || l.delete_time > tick_count {\n                return true;\n            }",
                 "            if !l.handle_stopped || l.delete_time >= tick_count {\n                return true;\n            }",
                 "KILLED", "reclaim is `minDeleteTime < tickCount`, not `<=`"),
            (SE, "    pub const REFERENCE_DISTANCE: f32 = 0.0;",
                 "    pub const REFERENCE_DISTANCE: f32 = 1.0;",
                 "KILLED", "a non-zero AL_REFERENCE_DISTANCE"),
            (SE, "        raw.clamp(0.0, 1.0)",
                 "        raw",
                 "KILLED", "no AL_MIN_GAIN clamp — the gain goes negative past max"),
            (SE, "    pub const AL_LINEAR_DISTANCE: i32 = 53251;",
                 "    pub const AL_LINEAR_DISTANCE: i32 = 53252;",
                 "KILLED", "the CLAMPED distance model, which vanilla does not use"),
            (SE, "        device.submit(channel, ChannelCall::SetPitch(pitch));\n        device.submit(channel, ChannelCall::SetVolume(gain));",
                 "        device.submit(channel, ChannelCall::SetVolume(gain));\n        device.submit(channel, ChannelCall::SetPitch(pitch));",
                 "KILLED", "the setup call order swapped"),
            (SE, "        device.submit(channel, ChannelCall::SetLooping(looping && !streaming));",
                 "        device.submit(channel, ChannelCall::SetLooping(looping));",
                 "KILLED", "a streaming loop also loops on the source"),
            (SE, "            if !instance.can_start_silent && instance.source != SoundSource::Music {",
                 "            if !instance.can_start_silent {",
                 "KILLED", "silent music is dropped (the MUSIC disjunct removed)"),
            (SE, "        let inst_volume = instance_volume(instance.volume, resolved.volume);",
                 "        let inst_volume = instance.volume;",
                 "KILLED", "the sounds.json entry's own volume is ignored"),
            (SE, "    fn stopped(&self, _channel: ChannelId) -> bool {\n        true\n    }",
                 "    fn stopped(&self, _channel: ChannelId) -> bool {\n        false\n    }",
                 "KILLED", "SilentDevice never frees a channel and the pool wedges"),
            (SE, "        self.gain_by_source = [1.0; SoundSource::ALL.len()];",
                 "        let _ = &mut self.gain_by_source;",
                 "KILLED", "stopAll does not reset gainBySource"),
            (SE, "        self.gain_by_source[source.ordinal() as usize] =\n            crate::sound_instance::mth_clamp(gain, 0.0, 1.0);",
                 "        self.gain_by_source[source.ordinal() as usize] = gain;",
                 "KILLED", "updateCategoryVolume does not clamp on the way in"),
        ],
    ),
    "c": (
        "resolution order, the wire adapter and the system drain",
        NET, 900,
        [
            (SE, "    pub fn is_active(&self, id: InstanceId) -> bool {",
                 "    pub fn is_active(&self, id: InstanceId) -> bool { // no-op control",
                 "SURVIVED", "NO-OP CONTROL: a comment"),
            (SE, "            None if sounds.get(&instance.identifier).is_some() => {\n                return (id, PlayResult::NotStarted(NotStarted::EmptySound));\n            }",
                 "            None if false => {\n                return (id, PlayResult::NotStarted(NotStarted::EmptySound));\n            }",
                 "KILLED", "a registered-but-empty event reported as unknown"),
            (SE, "            Some(r) if r.is_intentionally_empty() => {",
                 "            Some(r) if false && r.is_intentionally_empty() => {",
                 "KILLED", "intentionally_empty is not short-circuited"),
            (SE, "        let world = EntityTableWorld(entities);\n        let before = self.system.stats;",
                 "        let world = EmptyWorld;\n        let before = self.system.stats;",
                 "KILLED", "LiveSounds drives against an empty world"),
            (SE, "        let p = e.render_pos(1.0);\n        Some((p[0], p[1], p[2]))",
                 "        Some((e.x, e.y, e.z))",
                 "KILLED", "the synced target read instead of the current position"),
            (SE, "    let (x, y, z) = world\n        .entity_position(e.entity_id)\n        .ok_or(NoInstance::UnknownEntity)?;",
                 "    let (x, y, z) = world.entity_position(e.entity_id).unwrap_or((0.0, 0.0, 0.0));",
                 "KILLED", "an untracked entity's sound relocated to the origin"),
            (SE, "                self.engine.stop_matching(name, source, device);\n                self.stats.stops += 1;",
                 "                let _ = (name, source);\n                self.stats.stops += 1;",
                 "KILLED", "a stop_sound in the batch does nothing"),
            (SE, "            Some(seed) => sounds.get_sound_seeded(&instance.identifier, seed),",
                 "            Some(_) => sounds.get_sound_seeded(&instance.identifier, 0),",
                 "KILLED", "the packet seed is ignored (needs a multi-variant fixture to see)"),
        ],
    ),
    "d": (
        "the asset-index derivation",
        DATA, 900,
        [
            (SJ, "pub fn asset_index_id(version: &str) -> Option<String> {",
                 "pub fn asset_index_id(version: &str) -> Option<String> { // no-op control",
                 "SURVIVED", "NO-OP CONTROL: a comment"),
            (SJ, "    p.push(\"versions\");",
                 "    p.push(\"assets\");",
                 "KILLED", "the manifest read from the wrong directory"),
            (SJ, "    json.get(\"assetIndex\")?\n        .get(\"id\")?",
                 "    json.get(\"assetIndex\")?\n        .get(\"sha1\")?",
                 "KILLED", "the sha1 read as the index id"),
            (SEV, "        for (id, name) in pairs {\n            by_id.insert(id, name.clone());",
                  "        for (i, (_id, name)) in pairs.into_iter().enumerate() {\n            let id = i as i32;\n            by_id.insert(id, name.clone());",
                  "KILLED", "from_pairs renumbers by position (M64's trap, reintroduced)"),
        ],
    ),
    "e": (
        "the live composition roots — a measurement of the gap, not a check",
        APP, 900,
        [
            (LC, "fn build_sounds(version: &str, registry: &rewo_data::sound_events::SoundEvents) -> LiveSounds {",
                 "fn build_sounds(version: &str, registry: &rewo_data::sound_events::SoundEvents) -> LiveSounds { // no-op control",
                 "SURVIVED", "NO-OP CONTROL: a comment"),
            (LC, "        Ok(idx) => idx,",
                 "        Ok(_) => rewo_data::sounds_json::SoundsIndex::new(),",
                 "KILLED", "build_sounds always returns an empty index"),
            (LC, "    LiveSounds::new(sounds, registry.clone())",
                 "    LiveSounds::new(sounds, Default::default())",
                 "KILLED", "build_sounds hands over an empty registry"),
            (LC, "        let queued = session.take_sound_events();\n        sounds.drive(&queued, &session.world.entities);",
                 "        let queued = session.take_sound_events();\n        let _ = (&queued, &mut sounds);",
                 "SURVIVED", "GAP: run_headless never drains — no test seam reaches it"),
            (LC, "            let queued = session.take_sound_events();\n            self.sounds.drive(&queued, &session.world.entities);",
                 "            let queued = session.take_sound_events();\n            let _ = &queued;",
                 "SURVIVED", "GAP: LiveApp::frame never drains — no test seam reaches it"),
        ],
    ),
}

if __name__ == "__main__":
    which = sys.argv[1]
    name, check, timeout, muts = BATTERIES[which]
    sys.exit(1 if battery(name, check, timeout, muts) else 0)
