//! `sounds.json` — the weighted-variant index, and the seeded pick (M66).
//!
//! M64 turned a wire id into `"minecraft:block.stone.break"`. **That name is
//! not a file.** It is a key into `assets/<namespace>/sounds.json`, which maps
//! it to a *weighted set* of variants — `break1.ogg` … `break4.ogg` — each with
//! its own volume, pitch, weight, attenuation distance and stream flag. This
//! module is that map, plus `WeighedSoundEvents.getSound(RandomSource)`, the
//! walk that chooses one.
//!
//! Still **data only**: nothing here opens a device, decodes Vorbis or mixes.
//! Choosing *which* file to play is arithmetic a machine can grade; playing it
//! is not, which is the same split M63 drew.
//!
//! ## Why the seeded pick belongs here rather than in a mixer
//!
//! `ClientboundSoundPacket` carries a `seed`, decoded by M63 and so far
//! unused. It is not decoration:
//! `ClientLevel.playSound` builds `SimpleSoundInstance(…, RandomSource
//! .create(seed), …)`, `AbstractSoundInstance.resolve` calls
//! `soundEvent.getSound(this.random)`, and that is the **first** draw off the
//! seeded generator. So the variant every client picks for a given packet is
//! fixed by the server — two players standing together hear the same
//! `break3.ogg`. Get the walk wrong and you still hear *a* stone break, just
//! never the one everyone else heard, which no decode gate can see.
//!
//! ## Where the file is — **not** in the client jar
//!
//! `assets/minecraft/sounds.json` is delivered through the **asset index**,
//! not the jar: on 26.2 the jar's only `*sounds*` entries are the
//! `net/minecraft/client/…/sounds/*.class` files. It lives at
//! `<assets>/objects/<hash[0..2]>/<hash>`, named by
//! `<assets>/indexes/<id>.json` under the key `minecraft/sounds.json`. Every
//! `.ogg` is in the same store. [`load_from_asset_store`] does that lookup;
//! `crate::assets::jar_text` — the route every other jar-derived table takes —
//! would come back empty here.
//!
//! ## Four rules whose wrong answer is plausible
//!
//! 1. **A variant is a string *or* an object.** `getSounds` branches on
//!    `GsonHelper.isStringValue`, and the string form is not shorthand for
//!    `{"name": s}` — it is a full `Sound` with volume 1, pitch 1, weight 1,
//!    type FILE, `stream` false and attenuation **16**. 5,271 of 26.2's 8,024
//!    variants take that path.
//!
//! 2. **`"type": "event"` redirects to another event, it is not a filename.**
//!    61 of 26.2's variants do this. Read as a filename you get
//!    `sounds/<event.name>.ogg`, which does not exist — and because vanilla
//!    only runs `validateSoundResource` on `FILE` variants, the mistake shows
//!    up as *exactly the redirecting sounds* going silent while every other
//!    sound works.
//!
//! 3. **A redirect contributes the target's total weight, not its own.**
//!    `Preparations`' anonymous `Weighted` overrides `getWeight()` as
//!    `registry.get(target).getWeight()`; `sound.getWeight()` survives only as
//!    the `weight` field of the *produced* `Sound`, which nothing downstream
//!    selects on. So `{"type":"event","weight":5}` does not make that branch
//!    five times likelier — the target's own variant count does. And a
//!    redirect to an unregistered event weighs **0**, so it can never be
//!    picked rather than being picked and falling silent.
//!
//! 4. **`replace` resets, it does not merge.** `handleRegistration` builds a
//!    fresh `WeighedSoundEvents` when the event is absent *or* the entry says
//!    `replace: true`, and otherwise **appends** to what is already there —
//!    which also means a later entry's `subtitle` is silently discarded.
//!    26.2's own `sounds.json` sets `replace` nowhere; it exists for resource
//!    packs, where the difference between "adds a fifth variant" and
//!    "substitutes the only variant" is the whole point of the flag.
//!
//! ## The walk, and its off-by-one
//!
//! ```java
//! int index = random.nextInt(weight);
//! for (Weighted<Sound> w : list) { index -= w.getWeight(); if (index < 0) return w.getSound(random); }
//! ```
//!
//! `< 0`, not `<= 0`. With `[1, 1]` and a roll of 0 the first entry must win:
//! `0 - 1 = -1 < 0`. Under `<= 0` the roll would fall through to the second,
//! and the first variant of every two-variant sound would become unreachable —
//! a bias no test of "a sound plays" can see, and one that leaves the
//! *distribution* of a four-variant footstep visibly lopsided only to a
//! listener.
//!
//! ## Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/client/resources/sounds/SoundEventRegistrationSerializer.java`
//! - `net/minecraft/client/resources/sounds/Sound.java`
//! - `net/minecraft/client/sounds/WeighedSoundEvents.java`
//! - `net/minecraft/client/sounds/SoundManager.java` — `Preparations
//!   .handleRegistration`, `validateSoundResource`
//! - `net/minecraft/client/resources/sounds/AbstractSoundInstance.java` —
//!   `resolve`
//! - `net/minecraft/client/multiplayer/ClientLevel.java` — `playSound`, where
//!   the packet seed becomes `RandomSource.create(seed)`
//! - `net/minecraft/util/BitRandomSource.java` — `nextInt(bound)`

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

/// `SoundManager.SOUNDS_PATH`.
pub const SOUNDS_PATH: &str = "sounds.json";

/// The attenuation distance both `Sound` constructors default to.
pub const DEFAULT_ATTENUATION_DISTANCE: i32 = 16;

/// `SoundManager.EMPTY_SOUND_LOCATION`, for callers that want to name what a
/// `None` pick stands for.
pub const EMPTY_SOUND: &str = "minecraft:empty";

/// `SoundManager.INTENTIONALLY_EMPTY_SOUND_LOCATION` — silence on purpose.
///
/// **It is a registered `sound_event` with no `sounds.json` entry**, and the
/// only one. (The two sets are not nested either way: 26.2's file also
/// describes one event the registry does not name, which is why both counts
/// land on 1,968 — see the cross-check test.) That is
/// not an omission. `AbstractSoundInstance.resolve` tests the identifier
/// *before* consulting the registry and short-circuits to
/// `INTENTIONALLY_EMPTY_SOUND`, so a pack cannot give it a variant either.
///
/// `SoundEngine.play` then distinguishes three outcomes, and a data layer that
/// collapsed them would make a real bug unreportable: an unregistered event
/// warns "Unable to play unknown soundEvent", a registered one that resolves
/// to `EMPTY_SOUND` warns "Unable to play empty soundEvent", and this one
/// returns quietly. [`SoundsIndex::get_sound`] keeps the third distinct by
/// answering `Some` here — check [`ResolvedSound::is_intentionally_empty`]
/// before trying to open a file, because there is no
/// `sounds/intentionally_empty.ogg` to open.
pub const INTENTIONALLY_EMPTY_SOUND: &str = "minecraft:intentionally_empty";

/// How deep [`SoundsIndex`] follows `type: "event"` redirects.
///
/// Vanilla has no limit: `getWeight` on a redirect calls the target's
/// `getWeight`, so a pack whose two events point at each other makes real
/// Minecraft throw `StackOverflowError` inside a resource reload. A client
/// that a malformed pack can crash is worse than one that goes quiet, so the
/// recursion is bounded and an over-deep chain weighs 0 — which, by rule 3
/// above, is the same as a redirect to a missing event.
pub const MAX_REDIRECT_DEPTH: u32 = 16;

/// `Sound.Type`. The wire for this is the JSON `"type"` field, whose two legal
/// values are `"file"` (the default) and `"event"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundType {
    /// `Sound.Type.FILE` — `location` names an `.ogg` under `sounds/`.
    File,
    /// `Sound.Type.SOUND_EVENT` — `location` names another entry of this same
    /// index. See rule 2 in the module docs.
    Event,
}

impl SoundType {
    /// `Sound.Type.getByName`. `None` is vanilla's `null`, which
    /// `Objects.requireNonNull(type, "Invalid type")` turns into a throw —
    /// see [`parse_document`] on what a throw costs.
    pub fn by_name(name: &str) -> Option<SoundType> {
        match name {
            "file" => Some(SoundType::File),
            "event" => Some(SoundType::Event),
            _ => None,
        }
    }
}

/// One entry of an event's `sounds` array — vanilla's `Sound`.
///
/// `volume` and `pitch` are vanilla's `SampledFloat`s. Every one the
/// serializer builds is a `ConstantFloat`, so they are plain `f32` here; the
/// `SampledFloat` machinery only earns its keep for the `MultipliedFloats` a
/// redirect produces, and that multiplication is done eagerly in
/// [`SoundsIndex::get_sound`].
#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    /// A fully-qualified identifier — `Identifier.parse` applies the default
    /// namespace, so a bare `"block/stone/break1"` is `minecraft:…`.
    pub name: String,
    pub volume: f32,
    pub pitch: f32,
    pub weight: i32,
    pub ty: SoundType,
    pub stream: bool,
    pub preload: bool,
    pub attenuation_distance: i32,
}

impl Sound {
    /// The string form of `getSounds`' branch for a bare string: everything
    /// default except the name.
    pub fn file(name: impl Into<String>) -> Sound {
        Sound {
            name: name.into(),
            volume: 1.0,
            pitch: 1.0,
            weight: 1,
            ty: SoundType::File,
            stream: false,
            preload: false,
            attenuation_distance: DEFAULT_ATTENUATION_DISTANCE,
        }
    }
}

/// `SoundEventRegistration` — one entry of one `sounds.json`, before it is
/// merged into the index.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoundEventRegistration {
    pub sounds: Vec<Sound>,
    pub replace: bool,
    /// The **translation key**, not the text. `WeighedSoundEvents` wraps it in
    /// `Component.translatable`, so resolving it is `crate::lang`'s job.
    pub subtitle: Option<String>,
}

/// `WeighedSoundEvents` — one event's accumulated variants.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WeighedSoundEvents {
    pub subtitle: Option<String>,
    pub sounds: Vec<Sound>,
}

/// What a pick produces — vanilla's returned `Sound`, always `Type.FILE`.
///
/// A redirect makes this genuinely different from the [`Sound`] that was
/// walked: the name is the *target's*, the volume and pitch are the two
/// multiplied together (`MultipliedFloats`), `stream` is the **or** of both,
/// and `preload` and `attenuation_distance` come from the target alone while
/// `weight` comes from the redirect. That asymmetric mix is transcribed, not
/// tidied.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedSound {
    pub name: String,
    pub volume: f32,
    pub pitch: f32,
    pub weight: i32,
    pub stream: bool,
    pub preload: bool,
    pub attenuation_distance: i32,
}

impl ResolvedSound {
    /// `Sound.getPath()` — `FileToIdConverter("sounds", ".ogg")` — as the key
    /// the asset index uses, i.e. `<namespace>/sounds/<path>.ogg`.
    pub fn asset_path(&self) -> String {
        let (ns, path) = split_identifier(&self.name);
        format!("{ns}/sounds/{path}.ogg")
    }

    /// `sound == SoundManager.INTENTIONALLY_EMPTY_SOUND` — silence by design.
    /// See [`INTENTIONALLY_EMPTY_SOUND`]; there is no file behind it.
    pub fn is_intentionally_empty(&self) -> bool {
        self.name == INTENTIONALLY_EMPTY_SOUND
    }
}

/// Which `.ogg` files exist, for `SoundManager.validateSoundResource`.
///
/// Vanilla **drops** a `FILE` variant whose file is absent, with a warning.
/// That is not cosmetic: dropping it changes the event's total weight, so the
/// distribution over the surviving variants changes too. [`SoundFileSet::All`]
/// is the "no listing to check against" case, and it is a deliberate
/// divergence rather than an oversight — it keeps every declared variant, so a
/// caller with no asset index gets the file's own view of the world.
#[derive(Clone, Debug, Default)]
pub enum SoundFileSet {
    /// Accept every variant. See the type docs.
    #[default]
    All,
    /// Accept only variants whose `<ns>/sounds/<path>.ogg` is in the set.
    Only(HashSet<String>),
}

impl SoundFileSet {
    pub fn has(&self, asset_path: &str) -> bool {
        match self {
            SoundFileSet::All => true,
            SoundFileSet::Only(set) => set.contains(asset_path),
        }
    }
}

/// The one generator operation the pick needs — `RandomSource.nextInt(bound)`.
///
/// A trait rather than a concrete type because the interesting generator lives
/// in `rewo-world` (`particles::LegacyRandom`) and **`rewo-world` depends on
/// `rewo-data`, not the other way round**. A caller that already has one
/// implements this in three lines; [`LegacyRandom48`] is here so this crate's
/// own tests and a seed-only caller do not have to.
pub trait SoundRandom {
    fn next_int(&mut self, bound: i32) -> i32;
}

/// `LegacyRandomSource` restricted to `nextInt` — `java.util.Random`'s 48-bit
/// LCG, which is what `RandomSource.create(seed)` returns.
///
/// This is the third partial copy of that LCG in the workspace
/// (`rewo_world::particles::LegacyRandom` is the full one;
/// `rewo_gpu::entities` keeps its own for the same reason). The reason here is
/// the dependency direction stated on [`SoundRandom`], and the scope is one
/// primitive: the variant pick needs `nextInt` and nothing else, because every
/// `volume`/`pitch` the serializer builds is a `ConstantFloat` whose `sample`
/// never touches the generator.
#[derive(Clone, Debug)]
pub struct LegacyRandom48 {
    seed: u64,
}

const MULTIPLIER: u64 = 25_214_903_917;
const INCREMENT: u64 = 11;
const MODULUS_MASK: u64 = 281_474_976_710_655; // 2^48 - 1

impl LegacyRandom48 {
    /// `RandomSource.create(seed)` → `new LegacyRandomSource(seed)`.
    pub fn new(seed: i64) -> Self {
        Self {
            seed: ((seed as u64) ^ MULTIPLIER) & MODULUS_MASK,
        }
    }

    /// `BitRandomSource.next(int)`.
    pub fn next(&mut self, bits: u32) -> i32 {
        self.seed = self
            .seed
            .wrapping_mul(MULTIPLIER)
            .wrapping_add(INCREMENT)
            & MODULUS_MASK;
        (self.seed >> (48 - bits)) as i32
    }
}

impl SoundRandom for LegacyRandom48 {
    /// `BitRandomSource.nextInt(int)` — the power-of-two shortcut, then
    /// rejection sampling. Both branches are load-bearing: they draw a
    /// *different number of times* from the generator for the same bound
    /// class, so substituting a plain `next(31) % bound` diverges as soon as
    /// an event has a non-power-of-two total weight, which most do.
    fn next_int(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "bound must be positive");
        if bound & (bound - 1) == 0 {
            return ((bound as i64).wrapping_mul(self.next(31) as i64) >> 31) as i32;
        }
        loop {
            let sample = self.next(31);
            let modulo = sample % bound;
            if sample.wrapping_sub(modulo).wrapping_add(bound - 1) >= 0 {
                return modulo;
            }
        }
    }
}

/// The merged `sounds.json` registry — `SoundManager.registry`.
#[derive(Clone, Debug, Default)]
pub struct SoundsIndex {
    events: HashMap<String, WeighedSoundEvents>,
}

impl SoundsIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// `SoundManager.getSoundEvent`.
    pub fn get(&self, event: &str) -> Option<&WeighedSoundEvents> {
        self.events.get(event)
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// `SoundManager.getAvailableSounds`.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.events.keys().map(String::as_str)
    }

    /// `Preparations.handleRegistration` for one entry.
    ///
    /// Call it once per entry per pack, in pack order (vanilla walks
    /// `getResourceStack`, which is bottom-up: the built-in pack first, then
    /// each pack layered over it). See rule 4 in the module docs for what
    /// `replace` does and what it costs the subtitle.
    pub fn handle_registration(
        &mut self,
        event: &str,
        registration: &SoundEventRegistration,
        files: &SoundFileSet,
    ) {
        let misses = !self.events.contains_key(event);
        if misses || registration.replace {
            self.events.insert(
                event.to_string(),
                WeighedSoundEvents {
                    subtitle: registration.subtitle.clone(),
                    sounds: Vec::new(),
                },
            );
        }
        // `unwrap` cannot fire: the branch above guarantees the key exists.
        let target = self.events.get_mut(event).expect("just inserted");
        for sound in &registration.sounds {
            // Only FILE variants are validated. An `event` variant naming a
            // missing target is legal here and weighs 0 at pick time.
            if sound.ty == SoundType::File {
                let (ns, path) = split_identifier(&sound.name);
                if !files.has(&format!("{ns}/sounds/{path}.ogg")) {
                    log::debug!(
                        "rewo-data: {} does not exist, cannot add it to event {event}",
                        sound.name
                    );
                    continue;
                }
            }
            target.sounds.push(sound.clone());
        }
    }

    /// Parse one `sounds.json` and register every entry under `namespace`.
    pub fn load_document(
        &mut self,
        namespace: &str,
        json: &str,
        files: &SoundFileSet,
    ) -> Result<(), String> {
        for (key, registration) in parse_document(json)? {
            self.handle_registration(&format!("{namespace}:{key}"), &registration, files);
        }
        Ok(())
    }

    /// `WeighedSoundEvents.getWeight()` — the sum a pick rolls against.
    pub fn total_weight(&self, event: &WeighedSoundEvents) -> i32 {
        self.total_weight_at(event, 0)
    }

    fn total_weight_at(&self, event: &WeighedSoundEvents, depth: u32) -> i32 {
        if depth > MAX_REDIRECT_DEPTH {
            return 0;
        }
        // `int sum` in Java, so overflow wraps rather than panicking. No real
        // pack comes close, but a hostile one should not abort a reload.
        event
            .sounds
            .iter()
            .fold(0i32, |acc, s| acc.wrapping_add(self.weight_of(s, depth)))
    }

    fn weight_of(&self, sound: &Sound, depth: u32) -> i32 {
        match sound.ty {
            SoundType::File => sound.weight,
            // Rule 3: the *target's* total, and 0 when it is not registered.
            SoundType::Event => self
                .events
                .get(&sound.name)
                .map_or(0, |t| self.total_weight_at(t, depth + 1)),
        }
    }

    /// `WeighedSoundEvents.getSound(RandomSource)` for a named event.
    ///
    /// `None` is vanilla's `SoundManager.EMPTY_SOUND` — reached by an
    /// unregistered event, an event with no surviving variants, and a total
    /// weight of 0. All three are silence, and a caller that substituted a
    /// default would play a *wrong* sound, which is harder to notice than a
    /// missing one (the rule M64 states for an unknown registry id).
    ///
    /// [`INTENTIONALLY_EMPTY_SOUND`] is the exception: it short-circuits to
    /// `Some`, before the registry is consulted, exactly as
    /// `AbstractSoundInstance.resolve` does.
    pub fn get_sound(
        &self,
        event: &str,
        rng: &mut impl SoundRandom,
    ) -> Option<ResolvedSound> {
        if event == INTENTIONALLY_EMPTY_SOUND {
            return Some(ResolvedSound {
                name: INTENTIONALLY_EMPTY_SOUND.to_string(),
                volume: 1.0,
                pitch: 1.0,
                weight: 1,
                stream: false,
                preload: false,
                attenuation_distance: DEFAULT_ATTENUATION_DISTANCE,
            });
        }
        let events = self.events.get(event)?;
        self.pick(events, rng, 0)
    }

    /// [`Self::get_sound`] driven by a `ClientboundSoundPacket` seed — the
    /// whole point of the seed being on the wire.
    pub fn get_sound_seeded(&self, event: &str, seed: i64) -> Option<ResolvedSound> {
        self.get_sound(event, &mut LegacyRandom48::new(seed))
    }

    fn pick(
        &self,
        event: &WeighedSoundEvents,
        rng: &mut impl SoundRandom,
        depth: u32,
    ) -> Option<ResolvedSound> {
        if depth > MAX_REDIRECT_DEPTH {
            return None;
        }
        let weight = self.total_weight_at(event, depth);
        // Vanilla is `if (!this.list.isEmpty() && weight != 0)`. The `<= 0`
        // here is a deliberate widening of `!= 0`: an `int` sum can wrap
        // negative, and vanilla would then hand it to `nextInt`, which throws
        // `IllegalArgumentException("Bound must be positive")` and takes the
        // resource reload down with it. No real pack gets near 2^31 of
        // weight, so the two agree in practice; where they differ, a
        // malformed pack should silence one event rather than kill the load.
        if event.sounds.is_empty() || weight <= 0 {
            return None;
        }
        let mut index = rng.next_int(weight);
        for sound in &event.sounds {
            index -= self.weight_of(sound, depth);
            // `< 0`, never `<= 0` — see the module docs.
            if index < 0 {
                return self.resolve(sound, rng, depth);
            }
        }
        None
    }

    fn resolve(
        &self,
        sound: &Sound,
        rng: &mut impl SoundRandom,
        depth: u32,
    ) -> Option<ResolvedSound> {
        match sound.ty {
            SoundType::File => Some(ResolvedSound {
                name: sound.name.clone(),
                volume: sound.volume,
                pitch: sound.pitch,
                weight: sound.weight,
                stream: sound.stream,
                preload: sound.preload,
                attenuation_distance: sound.attenuation_distance,
            }),
            SoundType::Event => {
                let target = self.events.get(&sound.name)?;
                let inner = self.pick(target, rng, depth + 1)?;
                Some(ResolvedSound {
                    name: inner.name,
                    volume: inner.volume * sound.volume,
                    pitch: inner.pitch * sound.pitch,
                    // The redirect's own weight, not the target's — see the
                    // `ResolvedSound` docs.
                    weight: sound.weight,
                    stream: inner.stream || sound.stream,
                    preload: inner.preload,
                    attenuation_distance: inner.attenuation_distance,
                })
            }
        }
    }
}

/// `GsonHelper.fromJson(GSON, reader, Map<String, SoundEventRegistration>)`.
///
/// **One bad entry loses the whole document.** Vanilla's `prepare` wraps the
/// parse of an entire resource in `catch (RuntimeException)` and logs
/// `"Invalid sounds.json in resourcepack"`, so a single negative weight
/// discards every event in that file rather than that one entry — which is
/// why every validation below is an `Err` and not a skip.
///
/// The map is an [`IndexMap`] because gson's `LinkedTreeMap` is
/// insertion-ordered. Within one document the keys are distinct so the order
/// does not change the result, but it makes a diagnostic reproducible.
pub fn parse_document(
    json: &str,
) -> Result<IndexMap<String, SoundEventRegistration>, String> {
    let raw: IndexMap<String, serde_json::Value> =
        serde_json::from_str(json).map_err(|e| format!("sounds.json: {e}"))?;
    let mut out = IndexMap::with_capacity(raw.len());
    for (key, value) in raw {
        let reg = parse_registration(&key, &value)?;
        out.insert(key, reg);
    }
    Ok(out)
}

/// `SoundEventRegistrationSerializer.deserialize`.
fn parse_registration(
    key: &str,
    value: &serde_json::Value,
) -> Result<SoundEventRegistration, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("sounds.json: entry {key} is not an object"))?;
    let replace = match object.get("replace") {
        None | Some(serde_json::Value::Null) => false,
        Some(v) => v
            .as_bool()
            .ok_or_else(|| format!("sounds.json: {key}: replace is not a boolean"))?,
    };
    let subtitle = match object.get("subtitle") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(
            v.as_str()
                .ok_or_else(|| format!("sounds.json: {key}: subtitle is not a string"))?
                .to_string(),
        ),
    };
    let mut sounds = Vec::new();
    // `if (object.has("sounds"))` — an entry with no array is legal and empty,
    // which is a registered-but-silent event rather than an error.
    if let Some(array) = object.get("sounds") {
        let array = array
            .as_array()
            .ok_or_else(|| format!("sounds.json: {key}: sounds is not an array"))?;
        for element in array {
            sounds.push(parse_sound(key, element)?);
        }
    }
    Ok(SoundEventRegistration {
        sounds,
        replace,
        subtitle,
    })
}

/// One element of a `sounds` array — rule 1 in the module docs.
fn parse_sound(key: &str, element: &serde_json::Value) -> Result<Sound, String> {
    if let Some(name) = element.as_str() {
        return Ok(Sound::file(identifier(name)));
    }
    let object = element
        .as_object()
        .ok_or_else(|| format!("sounds.json: {key}: a sound is neither a string nor an object"))?;
    let name = object
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("sounds.json: {key}: a sound has no name"))?;
    let ty = match object.get("type") {
        None | Some(serde_json::Value::Null) => SoundType::File,
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| format!("sounds.json: {key}: type is not a string"))?;
            SoundType::by_name(s)
                .ok_or_else(|| format!("sounds.json: {key}: Invalid type: {s}"))?
        }
    };
    let volume = get_f32(object, "volume", 1.0, key)?;
    // `Validate.isTrue(volume > 0.0F, "Invalid volume")` — strictly greater,
    // so an explicit `"volume": 0` is a malformed file rather than a muted
    // variant.
    if !(volume > 0.0) {
        return Err(format!("sounds.json: {key}: Invalid volume"));
    }
    let pitch = get_f32(object, "pitch", 1.0, key)?;
    if !(pitch > 0.0) {
        return Err(format!("sounds.json: {key}: Invalid pitch"));
    }
    let weight = get_i32(object, "weight", 1, key)?;
    if weight <= 0 {
        return Err(format!("sounds.json: {key}: Invalid weight"));
    }
    let preload = get_bool(object, "preload", false, key)?;
    let stream = get_bool(object, "stream", false, key)?;
    let attenuation_distance =
        get_i32(object, "attenuation_distance", DEFAULT_ATTENUATION_DISTANCE, key)?;
    Ok(Sound {
        name: identifier(name),
        volume,
        pitch,
        weight,
        ty,
        stream,
        preload,
        attenuation_distance,
    })
}

fn get_f32(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    default: f32,
    key: &str,
) -> Result<f32, String> {
    match object.get(field) {
        None | Some(serde_json::Value::Null) => Ok(default),
        Some(v) => v
            .as_f64()
            .map(|f| f as f32)
            .ok_or_else(|| format!("sounds.json: {key}: {field} is not a number")),
    }
}

fn get_i32(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    default: i32,
    key: &str,
) -> Result<i32, String> {
    match object.get(field) {
        None | Some(serde_json::Value::Null) => Ok(default),
        Some(v) => v
            .as_i64()
            .map(|i| i as i32)
            .ok_or_else(|| format!("sounds.json: {key}: {field} is not an integer")),
    }
}

fn get_bool(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    default: bool,
    key: &str,
) -> Result<bool, String> {
    match object.get(field) {
        None | Some(serde_json::Value::Null) => Ok(default),
        Some(v) => v
            .as_bool()
            .ok_or_else(|| format!("sounds.json: {key}: {field} is not a boolean")),
    }
}

/// `Identifier.parse` — the default namespace is `minecraft`.
fn identifier(raw: &str) -> String {
    if raw.contains(':') {
        raw.to_string()
    } else {
        format!("minecraft:{raw}")
    }
}

/// The inverse, for building a resource path.
fn split_identifier(id: &str) -> (&str, &str) {
    match id.split_once(':') {
        Some((ns, path)) => (ns, path),
        None => ("minecraft", id),
    }
}

// ---------------------------------------------------------------------------
// The asset store
// ---------------------------------------------------------------------------

/// `<config>/EwoClient/shared/assets` — where the launcher puts the objects
/// Mojang's asset index names. Mirrors `crate::DataPaths::for_version`'s
/// resolution so the two agree about where `EwoClient` lives.
pub fn shared_assets_dir() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared");
    p.push("assets");
    Some(p)
}

/// Read every namespace's `sounds.json` out of the asset store and merge them.
///
/// `index_id` is the version manifest's `assetIndex.id` (`"32"` for 26.2), so
/// the file read is `<assets_root>/indexes/<index_id>.json`.
///
/// The `.ogg` listing comes from the same index, so
/// `validateSoundResource` is real here rather than assumed: a variant whose
/// file the store does not carry is dropped, exactly as vanilla drops it, and
/// the event's weights change with it.
pub fn load_from_asset_store(
    assets_root: &Path,
    index_id: &str,
) -> Result<SoundsIndex, String> {
    let index_path = assets_root.join("indexes").join(format!("{index_id}.json"));
    let index = crate::read_json_file(&index_path)?;
    let objects = index
        .get("objects")
        .and_then(|o| o.as_object())
        .ok_or_else(|| format!("{}: no objects", index_path.display()))?;

    let mut files = HashSet::new();
    // `<namespace>/sounds.json` per namespace, in a stable order so a
    // multi-namespace store merges the same way twice.
    let mut documents: Vec<(String, String)> = Vec::new();
    for (key, value) in objects {
        if key.ends_with(".ogg") {
            files.insert(key.clone());
        }
        if let Some(namespace) = key.strip_suffix(&format!("/{SOUNDS_PATH}")) {
            let hash = value
                .get("hash")
                .and_then(|h| h.as_str())
                .ok_or_else(|| format!("{}: {key} has no hash", index_path.display()))?;
            documents.push((namespace.to_string(), hash.to_string()));
        }
    }
    documents.sort();

    let files = SoundFileSet::Only(files);
    let mut out = SoundsIndex::new();
    for (namespace, hash) in &documents {
        let path = object_path(assets_root, hash);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        out.load_document(namespace, &text, &files)?;
    }
    log::info!(
        "rewo-data: {} sound event(s) from {} sounds.json",
        out.len(),
        documents.len()
    );
    Ok(out)
}

/// The `assetIndex.id` for a version, read from the per-version manifest the
/// launcher caches at `<config>/EwoClient/shared/versions/<v>/<v>.json`.
///
/// Not a constant, and not guessed from the directory listing. The store holds
/// several indexes at once (a machine with 1.8 and 26.x installed has
/// `1.8.json`, `29.json`, `30.json`, `32.json`), so "the newest file" is a
/// heuristic that silently reads another version's sounds the moment a newer
/// one is downloaded. The manifest says which one this version means, and
/// Mojang's manifests are immutable per version, so the answer cannot rot.
pub fn asset_index_id(version: &str) -> Option<String> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared");
    p.push("versions");
    p.push(version);
    p.push(format!("{version}.json"));
    let json = crate::read_json_file(&p).ok()?;
    json.get("assetIndex")?
        .get("id")?
        .as_str()
        .map(str::to_string)
}

/// [`load_from_asset_store`] for a version, resolving both paths itself.
///
/// `Err` covers every "this machine has no unpacked assets" case, which is the
/// common one on a fresh checkout. A caller should log it and carry on with an
/// empty index: a client with no `sounds.json` is **silent, not broken**, and
/// every event then resolves to "unknown event" rather than to a wrong sound.
pub fn load_for_version(version: &str) -> Result<SoundsIndex, String> {
    let root = shared_assets_dir().ok_or("no config dir")?;
    let id = asset_index_id(version)
        .ok_or_else(|| format!("no assetIndex.id for {version} in the shared version manifest"))?;
    load_from_asset_store(&root, &id)
}

/// `<assets_root>/objects/<hash[0..2]>/<hash>` — Mojang's object layout.
pub fn object_path(assets_root: &Path, hash: &str) -> PathBuf {
    assets_root
        .join("objects")
        .join(&hash[..hash.len().min(2)])
        .join(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A generator that hands back a scripted sequence, so a walk can be
    /// driven to an exact variant without going through the LCG. The bound is
    /// recorded, because "what did the pick roll against" is half of what
    /// these tests are asserting.
    struct Rolls {
        values: Vec<i32>,
        bounds: Vec<i32>,
    }

    impl Rolls {
        fn new(values: &[i32]) -> Self {
            Self {
                values: values.iter().rev().copied().collect(),
                bounds: Vec::new(),
            }
        }
    }

    impl SoundRandom for Rolls {
        fn next_int(&mut self, bound: i32) -> i32 {
            self.bounds.push(bound);
            self.values.pop().expect("a roll was not scripted")
        }
    }

    fn index(json: &str) -> SoundsIndex {
        let mut idx = SoundsIndex::new();
        idx.load_document("minecraft", json, &SoundFileSet::All)
            .expect("parse");
        idx
    }

    // -- parsing -----------------------------------------------------------

    /// Rule 1: a bare string is a whole `Sound`, not just a name — and the
    /// defaults it carries are the serializer's, including attenuation 16.
    #[test]
    fn a_string_variant_is_a_file_sound_with_every_default() {
        let d = parse_document(r#"{"e":{"sounds":["block/stone/break1"]}}"#).unwrap();
        let s = &d["e"].sounds[0];
        assert_eq!(s.name, "minecraft:block/stone/break1");
        assert_eq!(s.ty, SoundType::File);
        assert_eq!(s.volume, 1.0);
        assert_eq!(s.pitch, 1.0);
        assert_eq!(s.weight, 1);
        assert!(!s.stream);
        assert!(!s.preload);
        assert_eq!(s.attenuation_distance, 16);
    }

    /// The object form reads every field, and an absent one takes the same
    /// default the string form bakes in.
    #[test]
    fn an_object_variant_reads_its_fields_and_defaults_the_rest() {
        let d = parse_document(
            r#"{"e":{"sounds":[
                 {"name":"a","volume":0.5,"pitch":1.5,"weight":4,
                  "stream":true,"attenuation_distance":32},
                 {"name":"b"}]}}"#,
        )
        .unwrap();
        let a = &d["e"].sounds[0];
        assert_eq!(a.volume, 0.5);
        assert_eq!(a.pitch, 1.5);
        assert_eq!(a.weight, 4);
        assert!(a.stream);
        assert_eq!(a.attenuation_distance, 32);
        let b = &d["e"].sounds[1];
        assert_eq!(b.volume, 1.0);
        assert_eq!(b.weight, 1);
        assert!(!b.stream);
        assert_eq!(b.attenuation_distance, 16);
    }

    /// Rule 2, at the parse layer: `type` is read, and an unknown one is a
    /// `requireNonNull` throw rather than a silent fall back to `file`.
    #[test]
    fn the_type_field_is_read_and_an_unknown_type_rejects_the_document() {
        let d = parse_document(r#"{"e":{"sounds":[{"name":"a","type":"event"}]}}"#).unwrap();
        assert_eq!(d["e"].sounds[0].ty, SoundType::Event);
        assert!(parse_document(r#"{"e":{"sounds":[{"name":"a","type":"nope"}]}}"#).is_err());
    }

    /// Every `Validate.isTrue` in the serializer, and the fact that failing
    /// one discards the **whole document** rather than the entry.
    #[test]
    fn an_invalid_number_rejects_the_whole_document_not_just_the_entry() {
        for bad in [
            r#"{"good":{"sounds":["a"]},"bad":{"sounds":[{"name":"b","volume":0}]}}"#,
            r#"{"good":{"sounds":["a"]},"bad":{"sounds":[{"name":"b","pitch":-1}]}}"#,
            r#"{"good":{"sounds":[{"name":"b","weight":0}]}}"#,
        ] {
            assert!(parse_document(bad).is_err(), "accepted {bad}");
        }
        // The control: the same shapes with legal values parse.
        assert!(parse_document(
            r#"{"e":{"sounds":[{"name":"b","volume":0.0001,"pitch":2,"weight":1}]}}"#
        )
        .is_ok());
    }

    /// An entry with no `sounds` array is legal — a registered event with no
    /// variants, which resolves to silence rather than to an error.
    #[test]
    fn an_entry_without_a_sounds_array_registers_an_empty_event() {
        let idx = index(r#"{"e":{"subtitle":"subtitles.e"}}"#);
        let e = idx.get("minecraft:e").expect("registered");
        assert!(e.sounds.is_empty());
        assert_eq!(e.subtitle.as_deref(), Some("subtitles.e"));
        assert_eq!(idx.get_sound_seeded("minecraft:e", 0), None);
    }

    /// The default namespace is applied to both the event key and the
    /// variant names, so nothing downstream has to guess.
    #[test]
    fn identifiers_are_fully_qualified_on_both_sides() {
        let idx = index(r#"{"block.stone.break":{"sounds":["block/stone/break1","ns:x"]}}"#);
        assert!(idx.get("minecraft:block.stone.break").is_some());
        assert!(idx.get("block.stone.break").is_none());
        let s = &idx.get("minecraft:block.stone.break").unwrap().sounds;
        assert_eq!(s[0].name, "minecraft:block/stone/break1");
        assert_eq!(s[1].name, "ns:x");
    }

    // -- merging -----------------------------------------------------------

    /// Rule 4: without `replace` a second registration **appends**, and its
    /// subtitle is dropped.
    #[test]
    fn a_second_registration_appends_and_its_subtitle_is_discarded() {
        let mut idx = SoundsIndex::new();
        idx.load_document(
            "minecraft",
            r#"{"e":{"subtitle":"first","sounds":["a"]}}"#,
            &SoundFileSet::All,
        )
        .unwrap();
        idx.load_document(
            "minecraft",
            r#"{"e":{"subtitle":"second","sounds":["b"]}}"#,
            &SoundFileSet::All,
        )
        .unwrap();
        let e = idx.get("minecraft:e").unwrap();
        assert_eq!(e.sounds.len(), 2);
        assert_eq!(e.subtitle.as_deref(), Some("first"));
    }

    /// …and with `replace` it resets: the earlier variants are gone and the
    /// later subtitle wins. The difference between this test and the one
    /// above is the entire meaning of the flag.
    #[test]
    fn replace_resets_the_variant_list_and_takes_the_new_subtitle() {
        let mut idx = SoundsIndex::new();
        idx.load_document(
            "minecraft",
            r#"{"e":{"subtitle":"first","sounds":["a"]}}"#,
            &SoundFileSet::All,
        )
        .unwrap();
        idx.load_document(
            "minecraft",
            r#"{"e":{"replace":true,"subtitle":"second","sounds":["b"]}}"#,
            &SoundFileSet::All,
        )
        .unwrap();
        let e = idx.get("minecraft:e").unwrap();
        assert_eq!(e.sounds.len(), 1);
        assert_eq!(e.sounds[0].name, "minecraft:b");
        assert_eq!(e.subtitle.as_deref(), Some("second"));
    }

    /// `validateSoundResource` drops a FILE variant with no file — and the
    /// weight sum moves with it, which is the half that matters.
    #[test]
    fn a_missing_ogg_drops_its_variant_and_lowers_the_total_weight() {
        let json = r#"{"e":{"sounds":["a","b"]}}"#;
        let all = index(json);
        assert_eq!(all.total_weight(all.get("minecraft:e").unwrap()), 2);

        let mut only = SoundsIndex::new();
        let files = SoundFileSet::Only(["minecraft/sounds/a.ogg".to_string()].into());
        only.load_document("minecraft", json, &files).unwrap();
        let e = only.get("minecraft:e").unwrap();
        assert_eq!(e.sounds.len(), 1);
        assert_eq!(only.total_weight(e), 1);
    }

    /// …but an `event` variant is never validated as a file, because it does
    /// not name one. Reading it as a filename would drop it here.
    #[test]
    fn an_event_variant_survives_a_file_set_that_contains_nothing() {
        let mut idx = SoundsIndex::new();
        idx.load_document(
            "minecraft",
            r#"{"t":{"sounds":["a"]},"e":{"sounds":[{"name":"t","type":"event"}]}}"#,
            &SoundFileSet::Only(["minecraft/sounds/a.ogg".to_string()].into()),
        )
        .unwrap();
        assert_eq!(idx.get("minecraft:e").unwrap().sounds.len(), 1);
    }

    // -- the walk ----------------------------------------------------------

    /// The off-by-one, stated as the property: with equal weights, roll `i`
    /// selects variant `i`. Under `<= 0` every roll would land one late and
    /// variant 0 would be unreachable.
    #[test]
    fn roll_i_selects_variant_i_when_every_weight_is_one() {
        let idx = index(r#"{"e":{"sounds":["a","b","c","d"]}}"#);
        for (roll, want) in [(0, "a"), (1, "b"), (2, "c"), (3, "d")] {
            let mut rng = Rolls::new(&[roll]);
            let got = idx.get_sound("minecraft:e", &mut rng).unwrap();
            assert_eq!(got.name, format!("minecraft:{want}"), "roll {roll}");
            assert_eq!(rng.bounds, [4], "the bound is the total weight");
        }
    }

    /// A weighted set: the boundaries between variants sit exactly at the
    /// cumulative sums, and the bound rolled against is the sum of weights,
    /// not the number of variants.
    #[test]
    fn a_weighted_set_partitions_the_roll_at_its_cumulative_sums() {
        let idx = index(
            r#"{"e":{"sounds":[
                 {"name":"a","weight":3},{"name":"b","weight":1},{"name":"c","weight":2}]}}"#,
        );
        let expect = ["a", "a", "a", "b", "c", "c"];
        for (roll, want) in expect.iter().enumerate() {
            let mut rng = Rolls::new(&[roll as i32]);
            let got = idx.get_sound("minecraft:e", &mut rng).unwrap();
            assert_eq!(got.name, format!("minecraft:{want}"), "roll {roll}");
            assert_eq!(rng.bounds, [6]);
        }
    }

    /// Rule 3: a redirect contributes the **target's** total weight. Its own
    /// declared weight is inert for selection — the two rolls that would land
    /// on the redirect if its `weight: 5` counted instead land on `b`.
    #[test]
    fn a_redirect_contributes_the_targets_weight_not_its_own() {
        let idx = index(
            r#"{"t":{"sounds":["t1","t2"]},
                "e":{"sounds":[{"name":"t","type":"event","weight":5},"b"]}}"#,
        );
        // target weight 2 + b weight 1 = 3, not 5 + 1 = 6.
        assert_eq!(idx.total_weight(idx.get("minecraft:e").unwrap()), 3);
        let mut rng = Rolls::new(&[2]);
        assert_eq!(
            idx.get_sound("minecraft:e", &mut rng).unwrap().name,
            "minecraft:b"
        );
        assert_eq!(rng.bounds, [3]);
    }

    /// The redirect's payload: the target's name, the two volumes and
    /// pitches multiplied, `stream` or-ed, `preload`/attenuation from the
    /// target and `weight` from the redirect.
    #[test]
    fn a_redirect_multiplies_volume_and_pitch_and_mixes_the_rest_asymmetrically() {
        let idx = index(
            r#"{"t":{"sounds":[{"name":"t1","volume":0.5,"pitch":2.0,
                               "attenuation_distance":48,"preload":true}]},
                "e":{"sounds":[{"name":"t","type":"event","volume":0.5,"pitch":1.5,
                                "weight":7,"stream":true}]}}"#,
        );
        // Two rolls: the outer walk, then the target's own.
        let mut rng = Rolls::new(&[0, 0]);
        let got = idx.get_sound("minecraft:e", &mut rng).unwrap();
        assert_eq!(got.name, "minecraft:t1");
        assert_eq!(got.volume, 0.25);
        assert_eq!(got.pitch, 3.0);
        assert_eq!(got.weight, 7, "the redirect's own weight");
        assert!(got.stream, "either side streaming makes it stream");
        assert!(got.preload, "from the target");
        assert_eq!(got.attenuation_distance, 48, "from the target");
    }

    /// A redirect to an unregistered event weighs 0, so it is never picked —
    /// the sibling absorbs every roll instead of one roll producing silence.
    #[test]
    fn a_redirect_to_a_missing_event_weighs_zero_and_is_never_picked() {
        let idx = index(r#"{"e":{"sounds":[{"name":"gone","type":"event"},"b"]}}"#);
        assert_eq!(idx.total_weight(idx.get("minecraft:e").unwrap()), 1);
        let mut rng = Rolls::new(&[0]);
        assert_eq!(
            idx.get_sound("minecraft:e", &mut rng).unwrap().name,
            "minecraft:b"
        );
        assert_eq!(rng.bounds, [1]);
    }

    /// Silence, in each of its three shapes — and none of them is a
    /// substituted default.
    #[test]
    fn an_unregistered_or_empty_or_zero_weight_event_is_none() {
        let idx = index(r#"{"empty":{"sounds":[]},"z":{"sounds":[{"name":"gone","type":"event"}]}}"#);
        assert_eq!(idx.get_sound_seeded("minecraft:nope", 1), None);
        assert_eq!(idx.get_sound_seeded("minecraft:empty", 1), None);
        assert_eq!(idx.get_sound_seeded("minecraft:z", 1), None);
    }

    /// A cyclic redirect terminates. Vanilla would recurse until the stack
    /// gave out; the bound turns a hostile pack into a quiet one.
    #[test]
    fn a_redirect_cycle_terminates_instead_of_recursing_forever() {
        let idx = index(
            r#"{"a":{"sounds":[{"name":"b","type":"event"}]},
                "b":{"sounds":[{"name":"a","type":"event"}]}}"#,
        );
        assert_eq!(idx.total_weight(idx.get("minecraft:a").unwrap()), 0);
        assert_eq!(idx.get_sound_seeded("minecraft:a", 1), None);
    }

    /// The resolved sound names its file the way the asset store keys it.
    #[test]
    fn a_resolved_sound_maps_to_its_ogg_path() {
        let idx = index(r#"{"e":{"sounds":["block/stone/break1"]}}"#);
        let got = idx.get_sound_seeded("minecraft:e", 0).unwrap();
        assert_eq!(got.asset_path(), "minecraft/sounds/block/stone/break1.ogg");
    }

    // -- the seeded pick ---------------------------------------------------

    /// `java.util.Random`'s LCG, pinned against values a JDK produced.
    ///
    /// These are the ground truth the whole seeded pick rests on: if this
    /// drifts, every packet picks a different variant than every vanilla
    /// client hears, and nothing else in the module would notice. The JDK is
    /// genuinely independent evidence here — vanilla's `BitRandomSource`
    /// *reimplements* `java.util.Random`'s formulas rather than delegating to
    /// them, and the two were measured to agree on Temurin 25 for exactly
    /// these calls.
    #[test]
    fn the_legacy_lcg_matches_java_util_random() {
        // new Random(0).nextInt(4) x5 — a power of two, so the shift path.
        let mut r = LegacyRandom48::new(0);
        let got: Vec<i32> = (0..5).map(|_| r.next_int(4)).collect();
        assert_eq!(got, [2, 3, 0, 2, 2]);
        // new Random(42).nextInt(3) x5 — not a power of two, so the
        // rejection path, which consumes the generator differently.
        let mut r = LegacyRandom48::new(42);
        let got: Vec<i32> = (0..5).map(|_| r.next_int(3)).collect();
        assert_eq!(got, [2, 0, 0, 2, 0]);
        // new Random(-1).nextInt(6) x4 — a negative seed sign-extends into
        // the 48-bit scramble, which is why `new` takes an `i64`.
        let mut r = LegacyRandom48::new(-1);
        let got: Vec<i32> = (0..4).map(|_| r.next_int(6)).collect();
        assert_eq!(got, [5, 5, 3, 5]);
    }

    /// The seed picks the variant, and it picks *this* one: `new
    /// Random(12345).nextInt(4)` is 1 on the JDK, so a four-variant event
    /// resolves to its second file. Asserting the literal rather than
    /// recomputing it here is the difference between pinning the pick and
    /// restating the implementation.
    #[test]
    fn the_packet_seed_determines_the_variant() {
        let idx = index(r#"{"e":{"sounds":["a","b","c","d"]}}"#);
        let pick = |seed| idx.get_sound_seeded("minecraft:e", seed).unwrap().name;
        assert_eq!(pick(12345), "minecraft:b");
        assert_eq!(pick(0), "minecraft:c");
        // Stable, which is what makes two players hear the same file.
        for seed in [0i64, 42, -1, i64::MAX] {
            assert_eq!(pick(seed), pick(seed), "seed {seed} is not stable");
        }
    }

    // -- the real file -----------------------------------------------------

    /// The derivation must agree with the literal the test below uses.
    ///
    /// That is the point of asserting it: `"32"` is written by hand in this
    /// module's doc comment and in the next test, and
    /// [`asset_index_id`] derives it from the launcher's per-version manifest.
    /// Two independent routes to the same string, so a wrong derivation cannot
    /// pass by matching a constant it read.
    #[test]
    fn the_asset_index_id_derived_from_the_manifest_is_the_one_the_tests_hardcode() {
        let Some(id) = asset_index_id("26.2") else {
            eprintln!("SKIP: no shared version manifest for 26.2");
            return;
        };
        assert_eq!(id, "32");
    }

    #[test]
    fn load_for_version_reaches_the_same_index_as_the_explicit_path() {
        let Some(root) = shared_assets_dir() else {
            eprintln!("SKIP: no config dir");
            return;
        };
        if !root.join("indexes/32.json").exists() {
            eprintln!("SKIP: no asset index at {}", root.display());
            return;
        }
        let by_version = load_for_version("26.2").expect("load_for_version");
        let by_path = load_from_asset_store(&root, "32").expect("load_from_asset_store");
        assert_eq!(by_version.len(), by_path.len());
        // A missing version is an error, never a silently-empty index.
        assert!(load_for_version("0.0-not-a-version").is_err());
    }

    /// Pins against the **real** 26.2 `sounds.json`. The asset store is the
    /// user's own download, so a machine without it skips loudly rather than
    /// asserting on an empty index.
    #[test]
    fn the_real_sounds_json_resolves_the_events_the_registry_names() {
        let Some(root) = shared_assets_dir() else {
            eprintln!("SKIP: no config dir");
            return;
        };
        if !root.join("indexes/32.json").exists() {
            eprintln!("SKIP: no asset index at {}", root.display());
            return;
        }
        let idx = load_from_asset_store(&root, "32").expect("load");
        // 26.2's file carries one entry per registered sound event, which is
        // M64's count. A drift here means the two halves disagree about what
        // a sound *is*, which is exactly the join this module exists to make.
        assert_eq!(idx.len(), 1968);

        // A four-variant block sound: every roll lands on a real file.
        let e = idx.get("minecraft:block.stone.break").expect("stone break");
        let total = idx.total_weight(e);
        assert!(total >= 4, "stone break has {total} weight");
        for seed in 0..16i64 {
            let got = idx
                .get_sound_seeded("minecraft:block.stone.break", seed)
                .expect("a variant");
            assert!(
                got.name.starts_with("minecraft:"),
                "{} is not qualified",
                got.name
            );
        }

        // Rule 2 against the real data: 26.2 has 61 `type: event` variants,
        // and every one of them must resolve through to a file rather than
        // being looked up as `sounds/<event name>.ogg`.
        let mut redirects = 0usize;
        for name in idx.names() {
            let e = idx.get(name).unwrap();
            redirects += e
                .sounds
                .iter()
                .filter(|s| s.ty == SoundType::Event)
                .count();
        }
        assert_eq!(redirects, 61, "`type: event` variant count");

        // And a named one, resolved end to end. `music_disc.*` events
        // redirect to the `music_disc/*` files.
        let mut found = None;
        for name in idx.names() {
            let e = idx.get(name).unwrap();
            if e.sounds.iter().any(|s| s.ty == SoundType::Event) {
                found = Some(name.to_string());
                break;
            }
        }
        let name = found.expect("a redirecting event");
        let got = idx
            .get_sound_seeded(&name, 7)
            .unwrap_or_else(|| panic!("{name} resolved to nothing"));
        assert_ne!(got.name, name, "a redirect must land on a different name");
    }

    /// Silence-on-purpose short-circuits before the registry, so it answers
    /// `Some` even from an index that has never heard of it — which is what
    /// keeps it distinguishable from an event that is merely missing.
    #[test]
    fn intentionally_empty_resolves_without_a_sounds_json_entry() {
        let idx = index(r#"{"e":{"sounds":["a"]}}"#);
        assert!(idx.get(INTENTIONALLY_EMPTY_SOUND).is_none());
        let got = idx.get_sound_seeded(INTENTIONALLY_EMPTY_SOUND, 0).unwrap();
        assert!(got.is_intentionally_empty());
        // …and an ordinary unregistered event still answers `None`.
        assert_eq!(idx.get_sound_seeded("minecraft:empty", 0), None);
        assert!(!idx
            .get_sound_seeded("minecraft:e", 0)
            .unwrap()
            .is_intentionally_empty());
    }

    /// Every event the report registers has an entry in `sounds.json` — with
    /// exactly one deliberate exception, asserted as an equality rather than
    /// tolerated by an allow-list, so a second gap would still fail.
    #[test]
    fn the_registry_and_sounds_json_name_the_same_events() {
        let Some(root) = shared_assets_dir() else {
            eprintln!("SKIP: no config dir");
            return;
        };
        let Some(paths) = crate::DataPaths::for_version("26.2") else {
            eprintln!("SKIP: no config dir");
            return;
        };
        if !root.join("indexes/32.json").exists() || !paths.registries_json().exists() {
            eprintln!("SKIP: no asset store or datagen report");
            return;
        }
        let idx = load_from_asset_store(&root, "32").expect("load");
        let registry = crate::sound_events::SoundEvents::load(&paths.registries_json())
            .expect("registry");
        let mut missing = Vec::new();
        for id in 0..registry.len() as i32 {
            let name = registry.name(id).expect("dense ids");
            if idx.get(name).is_none() {
                missing.push(name.to_string());
            }
        }
        assert_eq!(
            missing,
            [INTENTIONALLY_EMPTY_SOUND],
            "the registry and sounds.json disagree beyond the one event that \
             is silent by design"
        );
        // The other direction, and it is **not** the mirror image. The file
        // also describes an event the registry does not name, which vanilla
        // notices and shrugs at: `SoundManager.apply` logs "Not having sound
        // event for: {}" at debug and keeps it, because a resource pack is
        // allowed to define sounds no `SoundEvent` refers to. The two
        // one-element gaps are why both sides count 1,968 while neither is a
        // subset of the other — a same-size check alone would have missed
        // both. Pinned by name, like `sound_events.rs` pins its ids: a jar
        // bump that adds or removes one should land here.
        let mut unregistered: Vec<&str> = idx
            .names()
            .filter(|n| registry.id_of(n).is_none())
            .collect();
        unregistered.sort_unstable();
        assert_eq!(unregistered, ["minecraft:entity.chicken_picky.step"]);
        // Now resolve every registered event. Silence is *not* only the
        // deliberate one: seven vanilla events ship an explicitly empty
        // `"sounds": []`, so they are registered, described, and still
        // resolve to `EMPTY_SOUND` — which `SoundEngine.play` warns about as
        // "Unable to play empty soundEvent". They are enumerated rather than
        // counted, because "some events are silent" would pass against a
        // parser that had dropped variants it failed to read.
        let mut silent: Vec<&str> = Vec::new();
        for id in 0..registry.len() as i32 {
            let name = registry.name(id).expect("dense ids");
            match idx.get_sound_seeded(name, id as i64) {
                None => silent.push(name),
                Some(got) => assert_eq!(
                    got.is_intentionally_empty(),
                    name == INTENTIONALLY_EMPTY_SOUND,
                    "{name}"
                ),
            }
        }
        silent.sort_unstable();
        assert_eq!(
            silent,
            [
                "minecraft:block.fungus.fall",
                "minecraft:block.fungus.hit",
                "minecraft:entity.cod.ambient",
                "minecraft:entity.salmon.ambient",
                "minecraft:entity.snow_golem.ambient",
                "minecraft:entity.tropical_fish.ambient",
                "minecraft:music.nether.warped_forest",
            ]
        );
    }
}
