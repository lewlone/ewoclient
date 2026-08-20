//! `rewo dimensioncheck` — M16's permanent **serverless** dimension oracle.
//!
//! The live gate (`rewo play --dimension-check`) is authoritative for the
//! *transition*; this command is authoritative for everything that can be
//! decided without a socket, and it is deliberately not a re-run of the unit
//! tests:
//!
//! 1. **The captured registry is the ground truth.** A real 26.2 vanilla server's
//!    Configuration `registry_data` packet is read back out of a recording and
//!    parsed by the **production** parser — the same
//!    `dimension_parse::parse_dimension_registry_packet` a live Configuration
//!    calls. Its raw wire order gives the holder ids; nothing here assumes an
//!    order, it *reads* one and then asserts it.
//! 2. **The bundled transcription must equal it, entry for entry.** The built-in
//!    `data/minecraft/dimension_type/*.json` transcriptions in
//!    `rewo_net::dimension_parse::builtin` — the fixtures every dimension unit
//!    test is written against — are encoded to wire bytes and parsed through the
//!    same entry point. A transcription that has drifted from what a server
//!    actually sends fails here, which is the failure the unit tests cannot see
//!    (they grade the transcription against itself).
//! 3. **The decompiled datagen JSON grades both, and grades [`EXPECT`].** The
//!    real 26.2 `data/minecraft/dimension_type/*.json` files are read off disk
//!    by [`crate::dimension_json`] — a reader that shares no code with the
//!    production NBT parser — and every client-consumed raw field is extracted
//!    and compared. `has_day_timeline` is *resolved* through the shipped
//!    `data/minecraft/tags/timeline/*.json` files rather than assumed, and it is
//!    derived without reference to `has_fixed_time`: the two are independent
//!    members of `DimensionType`. A capture, a fixture and a hand-written table
//!    that all agreed *and were all wrong* still fail here.
//! 4. **An independent expectation grades all three.** [`EXPECT`] is a
//!    hand-written table, so it catches a JSON reader that agreed with a parser
//!    because both mis-read the same field; the JSON in turn catches an `EXPECT`
//!    that went stale. Neither can be quietly dropped.
//! 5. **The world binding is exercised, not restated.** Each captured definition
//!    is pushed through `World::for_dimension` and then through the production
//!    `rewo_mesh::face_shade_code` / `mesh_column`: the assertion is that a
//!    registry entry reaches the vertex bytes — shape, sky channel, cardinal
//!    shade — not that two constants are equal.
//! 6. **The mesh pool's generation fence** is driven across two *different*
//!    dimension worlds, which is what the fence exists for.
//!
//! What this command deliberately does **not** own, and says so in its output:
//! the paced live transition (owned by `rewo play --dimension-check`), the GPU
//! sky/lightmap matrix (`skyshot` / `lightmapshot`), and the transition's own
//! discard/reset witnesses (owned by `rewo-net`'s `WorldTransition` unit tests,
//! which can see the world being discarded — nothing outside the transition
//! can).
//!
//! It fails closed: any missing input, any missing property, exit nonzero.

use std::path::PathBuf;

use crate::dimension_json;
use clap::Args as ClapArgs;
use rewo_data::assets::{RenderKind, TintSource};
use rewo_mesh::pool::{MeshPool, MeshTables};
use rewo_net::dimension_parse::{self, BUILTIN_ORDER};
use rewo_world::dimension::{CardinalLightType, DimensionShape, DimensionTypeDef, Skybox};
use rewo_world::World;

#[derive(ClapArgs)]
pub struct DimensioncheckArgs {
    /// Assert every owned property. The oracle asserts unconditionally and a
    /// failure exits nonzero with or without this flag, so it only labels the
    /// run — the same convention `meshshot` uses.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// Recording holding a captured Configuration `registry_data` packet.
    /// Defaults to the canonical soak recording for `--version`.
    #[arg(long)]
    replay: Option<PathBuf>,
    /// The decompiled `data/minecraft` directory holding the datagen
    /// `dimension_type/*.json` and `tags/timeline/*.json`. Defaults to
    /// `%APPDATA%/EwoClient/rewo/<version>/decompiled/data/minecraft`.
    #[arg(long)]
    decompiled: Option<PathBuf>,
    #[arg(long, default_value = "26.2")]
    version: String,
}

// ---------------------------------------------------------------- expectation

/// One dimension-type entry as transcribed from the bundled
/// `data/minecraft/dimension_type/*.json`, with every codec default written out
/// explicitly. This is the oracle's *independent* expectation: it is not read
/// from the fixtures and not read from the capture, so it grades both.
///
/// It is itself graded — by [`crate::dimension_json`], against the real datagen
/// files on disk — so it cannot go stale, and it stays in the chain so that a
/// JSON reader and a network parser that mis-read the *same* field are still
/// caught by a value a human wrote down.
pub struct Expect {
    pub name: &'static str,
    pub min_y: i32,
    pub height: i32,
    /// `height / 16` — the section count a chunk body is decoded against, which
    /// is the number that mis-decoded every Nether chunk before M16.
    pub sections: usize,
    pub has_sky_light: bool,
    pub skybox: Skybox,
    pub ambient_light: f32,
    pub cardinal: CardinalLightType,
    /// The six shade factors in the **mesher's** face order
    /// `[up, down, north, south, west, east]`.
    pub cardinal_factors: [f32; 6],
    pub sky_color: Option<u32>,
    pub fog_color: Option<u32>,
    pub ambient_light_color: u32,
    pub sky_light_color: u32,
    pub sky_light_factor: f32,
    /// The `timelines` holder set contains `minecraft:day`.
    pub has_day_timeline: bool,
    pub has_fixed_time: bool,
    /// `default_clock`, as the identifier the file spells — `None` where the
    /// dimension declares none, which is the Nether and only the Nether.
    ///
    /// It decides which clock `Level.getDefaultClockTime()` reads, and so
    /// which clock the End's flash schedule runs on. `None` is not a stand-in
    /// for the Overworld's: `getClockTimeTicks` answers `.orElse(0L)`, a
    /// permanent zero.
    pub default_clock: Option<&'static str>,
}

/// The four built-ins, in [`BUILTIN_ORDER`].
///
/// `overworld_caves` differs from `overworld` only in `has_ceiling`, which the
/// client does not consume — it is here precisely so the oracle proves two
/// entries with identical *client-visible* content still occupy two distinct
/// registry slots, i.e. that selection is by raw id and never by value or name.
pub const EXPECT: [Expect; 4] = [
    Expect {
        name: "minecraft:overworld",
        min_y: -64,
        height: 384,
        sections: 24,
        has_sky_light: true,
        // No `skybox` field → the codec default.
        skybox: Skybox::Overworld,
        ambient_light: 0.0,
        cardinal: CardinalLightType::Default,
        cardinal_factors: [1.0, 0.5, 0.8, 0.8, 0.6, 0.6],
        sky_color: Some(0xFF78_A7FF),
        fog_color: Some(0xFFC0_D8FF),
        ambient_light_color: 0xFF0A_0A0A,
        // No `sky_light_color` / `sky_light_factor` override → the exact
        // `EnvironmentAttributes` defaults.
        // Literals from EnvironmentAttributes.java, deliberately independent
        // of the production constants graded by this table.
        sky_light_color: 0xFFFF_FFFF,
        sky_light_factor: 1.0,
        has_day_timeline: true,
        has_fixed_time: false,
        default_clock: Some("minecraft:overworld"),
    },
    Expect {
        name: "minecraft:overworld_caves",
        min_y: -64,
        height: 384,
        sections: 24,
        has_sky_light: true,
        skybox: Skybox::Overworld,
        ambient_light: 0.0,
        cardinal: CardinalLightType::Default,
        cardinal_factors: [1.0, 0.5, 0.8, 0.8, 0.6, 0.6],
        sky_color: Some(0xFF78_A7FF),
        fog_color: Some(0xFFC0_D8FF),
        ambient_light_color: 0xFF0A_0A0A,
        sky_light_color: 0xFFFF_FFFF,
        sky_light_factor: 1.0,
        has_day_timeline: true,
        has_fixed_time: false,
        default_clock: Some("minecraft:overworld"),
    },
    Expect {
        name: "minecraft:the_end",
        min_y: 0,
        height: 256,
        sections: 16,
        // The End *has* a sky light engine — it is the sky *factor* that is 0.
        has_sky_light: true,
        skybox: Skybox::End,
        ambient_light: 0.25,
        cardinal: CardinalLightType::Default,
        cardinal_factors: [1.0, 0.5, 0.8, 0.8, 0.6, 0.6],
        sky_color: Some(0xFF00_0000),
        fog_color: Some(0xFF18_1318),
        ambient_light_color: 0xFF3F_473F,
        sky_light_color: 0xFFAC_60CD,
        sky_light_factor: 0.0,
        has_day_timeline: false,
        has_fixed_time: true,
        // A DIFFERENT clock from the Overworld's — a vanilla server sends both
        // in every `set_time`, and the End's flash schedule runs on this one.
        default_clock: Some("minecraft:the_end"),
    },
    Expect {
        name: "minecraft:the_nether",
        min_y: 0,
        height: 256,
        sections: 16,
        has_sky_light: false,
        skybox: Skybox::None,
        ambient_light: 0.1,
        cardinal: CardinalLightType::Nether,
        cardinal_factors: [0.9, 0.9, 0.8, 0.8, 0.6, 0.6],
        // The Nether overrides neither: absence must stay absence, or the biome
        // layer's base colour reads as opaque black.
        sky_color: None,
        fog_color: None,
        ambient_light_color: 0xFF30_2821,
        sky_light_color: 0xFF7A_7AFF,
        sky_light_factor: 0.0,
        has_day_timeline: false,
        has_fixed_time: true,
        // The ONLY vanilla dimension that declares no clock, so
        // `getDefaultClockTime()` here is a permanent zero.
        default_clock: None,
    },
];

impl Expect {
    /// Grade one parsed definition against this expectation. `source` names
    /// where the definition came from, so a failure says which of the two
    /// inputs disagreed with the decompile.
    pub fn grade(&self, source: &str, holder: usize, d: &DimensionTypeDef) -> Result<(), String> {
        let fail = |what: &str, got: String, want: String| {
            Err(format!(
                "{source}[{holder}] {}: {what} is {got}, expected {want}",
                d.name
            ))
        };
        macro_rules! eq {
            ($what:literal, $got:expr, $want:expr) => {
                if $got != $want {
                    return fail($what, format!("{:?}", $got), format!("{:?}", $want));
                }
            };
        }
        eq!("registry name", d.name.as_str(), self.name);
        eq!(
            "shape",
            d.shape,
            DimensionShape {
                min_y: self.min_y,
                height: self.height
            }
        );
        eq!("section count", d.shape.section_count(), self.sections);
        eq!("has_skylight", d.has_sky_light, self.has_sky_light);
        eq!("skybox", d.skybox, self.skybox);
        eq!("ambient_light", d.ambient_light, self.ambient_light);
        eq!("cardinal_light", d.cardinal_light_type, self.cardinal);
        for face in 0..6 {
            eq!(
                "cardinal factor",
                d.cardinal_light.by_mesh_face(face),
                self.cardinal_factors[face]
            );
        }
        eq!("sky_color", d.sky_color, self.sky_color.map(|c| c as i32));
        eq!("fog_color", d.fog_color, self.fog_color.map(|c| c as i32));
        eq!(
            "ambient_light_color",
            d.ambient_light_color,
            self.ambient_light_color as i32
        );
        eq!(
            "sky_light_color",
            d.sky_light_color,
            self.sky_light_color as i32
        );
        eq!(
            "sky_light_factor",
            d.sky_light_factor,
            self.sky_light_factor
        );
        eq!(
            "has_day_timeline",
            d.has_day_timeline,
            self.has_day_timeline
        );
        eq!("has_fixed_time", d.has_fixed_time, self.has_fixed_time);
        eq!(
            "default_clock",
            d.default_clock.as_deref(),
            self.default_clock
        );
        Ok(())
    }
}

// ------------------------------------------------------------------- command

pub fn run(args: DimensioncheckArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[dimensioncheck] mode: {mode} (the oracle asserts unconditionally — a failure \
         exits nonzero with or without --check)"
    );

    // -- 1. the decompiled datagen JSON, read independently ------------------
    let data_root = args
        .decompiled
        .clone()
        .unwrap_or_else(|| dimension_json::default_data_root(&args.version));
    let json = dimension_json::load(&data_root, &BUILTIN_ORDER)?;
    println!(
        "[dimensioncheck] decompiled JSON: {} entries from {}",
        json.len(),
        data_root.join("dimension_type").display()
    );
    for d in &json {
        println!(
            "[dimensioncheck]   {:<26} timelines {:?} -> {:?} (day={}); defaulted: {}",
            d.name,
            d.timelines_raw,
            d.timeline_ids,
            d.has_day_timeline,
            if d.defaulted.is_empty() {
                "none".to_string()
            } else {
                d.defaulted.join(", ")
            }
        );
    }
    // The hand-written table is graded by the files *before* it is allowed to
    // grade anything else, so a stale EXPECT can never certify a capture.
    for (holder, expect) in EXPECT.iter().enumerate() {
        expect.grade("decompiled-json", holder, &json[holder].to_def())?;
    }

    // -- 2. the captured registry ------------------------------------------
    let replay = match args.replay.clone() {
        Some(p) => p,
        None => default_replay(&args.version).ok_or_else(|| {
            format!(
                "no captured configuration: {} does not exist. Record one with \
                 `rewo net --record`, or pass --replay <recording>. This gate fails \
                 closed rather than grading the bundled transcription against itself.",
                default_replay_path(&args.version).display()
            )
        })?,
    };
    let captured = captured_registry(&replay)?;
    println!(
        "[dimensioncheck] captured registry: {} entries from {}",
        captured.len(),
        replay.display()
    );

    // -- 3. the bundled transcription, through the same production parser ---
    let bundled =
        dimension_parse::parse_dimension_registry_packet(&dimension_parse::builtin_registry_body())
            .map_err(|e| format!("bundled built-in transcription failed to parse: {e}"))?
            .ok_or("bundled built-in body is not a dimension_type registry")?;

    // The raw order is *read* from the capture, then asserted — the holder id a
    // login/respawn packet names is this vector's index and nothing else.
    let captured_names: Vec<&str> = captured.iter().map(|d| d.name.as_str()).collect();
    if captured_names != BUILTIN_ORDER {
        return Err(format!(
            "captured raw registry order is {captured_names:?}, expected {BUILTIN_ORDER:?}"
        ));
    }
    if captured.len() != EXPECT.len() {
        return Err(format!(
            "captured registry has {} entries, expected {}",
            captured.len(),
            EXPECT.len()
        ));
    }
    if bundled.len() != captured.len() {
        return Err(format!(
            "bundled transcription has {} entries, capture has {}",
            bundled.len(),
            captured.len()
        ));
    }

    if json.len() != captured.len() {
        return Err(format!(
            "decompiled JSON has {} entries, capture has {}",
            json.len(),
            captured.len()
        ));
    }

    // -- 4. grade both against the JSON and the expectation, and each other --
    for (holder, expect) in EXPECT.iter().enumerate() {
        // The datagen files first: they are the only input here that is neither
        // hand-written nor produced by the code under test.
        json[holder].diff("captured", holder, &captured[holder])?;
        json[holder].diff("bundled", holder, &bundled[holder])?;
        expect.grade("captured", holder, &captured[holder])?;
        expect.grade("bundled", holder, &bundled[holder])?;
        if captured[holder] != bundled[holder] {
            return Err(format!(
                "holder {holder}: the captured entry and the bundled transcription differ\n\
                 captured: {:?}\nbundled:  {:?}",
                captured[holder], bundled[holder]
            ));
        }
    }
    // Two entries whose client-visible content is identical must still be two
    // slots: if selection ever collapsed onto value or name, this would be the
    // first thing to break.
    let mut caves_as_overworld = captured[1].clone();
    caves_as_overworld.name.clone_from(&captured[0].name);
    if caves_as_overworld != captured[0] {
        return Err(
            "overworld and overworld_caves differ in client-visible content — \
                    the decompile changed and EXPECT is stale"
                .into(),
        );
    }
    if captured_names[0] == captured_names[1] {
        return Err("the two overworld slots share a name".into());
    }

    print_matrix(&captured);

    // -- 5..7. the properties that actually bind the registry to the client --
    let world_report = check_world_binding(&captured)?;
    let mesh_report = check_mesh_binding(&captured)?;
    let fence_report = check_generation_fence(&captured)?;

    println!(
        "[dimensioncheck] world binding: {} dimensions, {} shape/section-index probes, \
         {} light probes (unloaded + loaded-sparse + loaded-lit)",
        world_report.dimensions, world_report.shape_probes, world_report.light_probes
    );
    println!(
        "[dimensioncheck] mesh binding: {} dimensions meshed, {} vertices graded, \
         shade codes {:?}, max sky nibble per dimension {:?}",
        mesh_report.dimensions, mesh_report.vertices, mesh_report.shade_codes, mesh_report.max_sky,
    );
    println!(
        "[dimensioncheck] generation fence: submit(g0)={} resubmit(g0)={} submit(g1)={}; \
         drained {} outputs, generations {:?}",
        fence_report.first,
        fence_report.resubmit,
        fence_report.next_generation,
        fence_report.drained,
        fence_report.generations
    );

    // -- delegations, named rather than silently skipped ---------------------
    println!(
        "[dimensioncheck] delegated: the paced live transition (level key, respawn \
         boundary, column discard, settled corrections) -> `rewo play --dimension-check`; \
         the transition's own discard/reset witnesses -> rewo-net `WorldTransition` unit \
         tests (only the transition can see the world it discards); the GPU sky and \
         lightmap matrix -> `rewo skyshot --check` / `rewo lightmapshot --check`"
    );
    println!(
        "DIMENSIONCHECK: CHECK OK — {} captured registry entries in raw order {:?} equal \
         both the bundled built-in transcription and the decompiled datagen JSON read from \
         {}, field for field (shape, section count, skylight, skybox, ambient scalar, \
         cardinal type + all six factors, sky/fog/ambient/sky-light colours + factor, fixed \
         time, and a day timeline resolved through the shipped tags/timeline tag files); the \
         independent EXPECT table agrees with all three; every entry propagates through \
         World::for_dimension to the vertical shape, the sky channel and the cardinal shade \
         codes the mesher packs; and the mesh pool's generation fence separates two \
         dimension worlds",
        captured.len(),
        captured_names,
        data_root.display(),
    );
    Ok(())
}

// ------------------------------------------------------------------- capture

fn default_replay_path(version: &str) -> PathBuf {
    let mut p = dirs::config_dir().unwrap_or_default();
    p.push("EwoClient");
    p.push("rewo");
    p.push(version);
    p.push("m1-soak.rewo");
    p
}

fn default_replay(version: &str) -> Option<PathBuf> {
    let p = default_replay_path(version);
    p.exists().then_some(p)
}

/// Pull the one Configuration `registry_data` packet that carries
/// `minecraft:dimension_type` out of a recording and parse it with the
/// production parser.
///
/// The packet is found by *content* (the registry identifier the body starts
/// with), not by packet id, so this needs no version tables and cannot silently
/// pick a different registry: `parse_dimension_registry_packet` returns
/// `Ok(None)` for every other registry and `Err` for a dimension_type body it
/// cannot decode.
fn captured_registry(path: &std::path::Path) -> Result<Vec<DimensionTypeDef>, String> {
    let packets = rewo_net::record::read_all(path)?;
    let mut found: Option<Vec<DimensionTypeDef>> = None;
    for p in &packets {
        if p.state != rewo_data::packets::State::Configuration {
            continue;
        }
        if let Some(defs) = dimension_parse::parse_dimension_registry_packet(&p.body)? {
            if found.is_some() {
                return Err(format!(
                    "{}: more than one minecraft:dimension_type registry_data packet",
                    path.display()
                ));
            }
            found = Some(defs);
        }
    }
    found.ok_or_else(|| {
        format!(
            "{}: no Configuration minecraft:dimension_type registry_data packet — \
             this recording cannot ground the dimension matrix",
            path.display()
        )
    })
}

fn print_matrix(defs: &[DimensionTypeDef]) {
    println!(
        "[dimensioncheck] holder  name                       min_y height sec sky skybox    \
         ambient cardinal  up/down  sky_color  fog_color  amb_color  skylight_color/factor  day fixed"
    );
    for (holder, d) in defs.iter().enumerate() {
        let col = |c: Option<i32>| match c {
            Some(v) => format!("{:08x}", v as u32),
            None => "--------".into(),
        };
        println!(
            "[dimensioncheck] {holder:>6}  {:<26} {:>5} {:>6} {:>3} {:>3} {:<9} {:>7.2} \
             {:<9} {:.1}/{:.1}  {}   {}   {:08x}   {:08x}/{:.1}          {}   {}",
            d.name,
            d.shape.min_y,
            d.shape.height,
            d.shape.section_count(),
            if d.has_sky_light { "yes" } else { "no" },
            d.skybox.name(),
            d.ambient_light,
            d.cardinal_light_type.name(),
            d.cardinal_light.up,
            d.cardinal_light.down,
            col(d.sky_color),
            col(d.fog_color),
            d.ambient_light_color as u32,
            d.sky_light_color as u32,
            d.sky_light_factor,
            if d.has_day_timeline { "yes" } else { "no " },
            if d.has_fixed_time { "yes" } else { "no" },
        );
    }
}

// -------------------------------------------------------------- world binding

struct WorldReport {
    dimensions: usize,
    shape_probes: usize,
    light_probes: usize,
}

/// Every captured definition, pushed through `World::for_dimension` and then
/// *queried* — the shape a chunk body is decoded against, the sky channel, the
/// cardinal table, and the three read paths that a no-skylight dimension has to
/// override (unloaded column, loaded-but-sparse cell, loaded cell).
fn check_world_binding(defs: &[DimensionTypeDef]) -> Result<WorldReport, String> {
    let (mut shape_probes, mut light_probes) = (0usize, 0usize);
    for (holder, d) in defs.iter().enumerate() {
        let name = &d.name;
        let mut w = World::for_dimension(d);
        if w.shape != d.shape {
            return Err(format!("{name}: World::for_dimension shape {:?}", w.shape));
        }
        if w.has_sky_light() != d.has_sky_light {
            return Err(format!("{name}: World has_sky_light {}", w.has_sky_light()));
        }
        if w.cardinal_light_type() != d.cardinal_light_type
            || w.cardinal_light() != d.cardinal_light
        {
            return Err(format!("{name}: World cardinal light did not propagate"));
        }

        // Section indexing: the exact boundaries. One below the floor and one
        // at the ceiling must be out of range, the floor is section 0 and the
        // top block is the last section — this is the arithmetic that
        // mis-addressed every Nether chunk when the shape was stale.
        let top = d.shape.min_y + d.shape.height;
        for (y, want) in [
            (d.shape.min_y - 1, None),
            (d.shape.min_y, Some(0usize)),
            (top - 1, Some(d.shape.section_count() - 1)),
            (top, None),
        ] {
            shape_probes += 1;
            if d.shape.section_index(y) != want {
                return Err(format!(
                    "{name}: section_index({y}) = {:?}, expected {want:?}",
                    d.shape.section_index(y)
                ));
            }
        }

        // A sky-lit column, inserted into a world whose dimension may have no
        // sky channel at all. `Column::empty_lit` fills the sky nibbles with 15
        // on purpose: it is exactly the stale full-bright value a Nether read
        // must not be able to return.
        w.ensure_column(0, 0);
        let want_sky = if d.has_sky_light { 15u8 } else { 0 };
        let probes: [(i32, i32, i32, &str); 4] = [
            (8, d.shape.min_y + 8, 8, "loaded, inside the column"),
            (8, top - 1, 8, "loaded, top block"),
            // Outside every loaded column: the sparse/edge read.
            (10_000, d.shape.min_y + 8, 10_000, "unloaded column"),
            // Loaded column, but a y the column has no section for.
            (8, top + 64, 8, "loaded column, above the world"),
        ];
        for (x, y, z, what) in probes {
            light_probes += 1;
            let (block, sky) = w.light_at(x, y, z);
            if sky != want_sky {
                return Err(format!(
                    "{name} (holder {holder}): light_at({x},{y},{z}) [{what}] sky = {sky}, \
                     expected {want_sky} — a dimension with has_skylight={} must {}",
                    d.has_sky_light,
                    if d.has_sky_light {
                        "keep its sky channel"
                    } else {
                        "have no sky channel at all, including at sparse and unloaded reads"
                    }
                ));
            }
            let bright = w.brightness_at(x, y, z);
            if bright != block.max(sky) {
                return Err(format!(
                    "{name}: brightness_at({x},{y},{z}) = {bright}, but light_at is \
                     ({block},{sky})"
                ));
            }
        }

        // The mesh worker's view must carry the same contract, or a Nether
        // column would be meshed with Overworld shade and sky light.
        let snap = w.snapshot_3x3(0, 0);
        if snap.shape != d.shape
            || snap.has_sky_light() != d.has_sky_light
            || snap.cardinal_light() != d.cardinal_light
        {
            return Err(format!("{name}: snapshot_3x3 lost the dimension contract"));
        }
        if snap.light_at(8, d.shape.min_y + 8, 8).1 != want_sky {
            return Err(format!("{name}: snapshot_3x3 sky channel disagrees"));
        }
    }
    Ok(WorldReport {
        dimensions: defs.len(),
        shape_probes,
        light_probes,
    })
}

// --------------------------------------------------------------- mesh binding

struct MeshReport {
    dimensions: usize,
    vertices: usize,
    shade_codes: Vec<Vec<u8>>,
    max_sky: Vec<u8>,
}

/// A single opaque cube state, enough to make the mesher emit all six faces.
const STONE: u32 = 1;

fn cube_tables() -> MeshTables {
    let mut render = vec![RenderKind::Invisible; 2];
    render[STONE as usize] = RenderKind::Cube {
        faces: [0; 6],
        raw_faces: [0; 6],
        tint: [TintSource::None; 6],
    };
    MeshTables {
        render,
        models: Vec::new(),
        fluid: Vec::new(),
    }
}

/// Mesh one floating cube in each dimension's world and read the vertex bytes
/// back: the shade codes must be exactly the codes the dimension's cardinal
/// table resolves to, and a dimension with no sky light must not be able to put
/// a nonzero sky nibble into a vertex even though the column it is meshing was
/// filled with sky 15.
fn check_mesh_binding(defs: &[DimensionTypeDef]) -> Result<MeshReport, String> {
    let tables = cube_tables();
    let mut shade_codes = Vec::new();
    let mut max_sky = Vec::new();
    let mut vertices = 0usize;

    for d in defs {
        let name = &d.name;
        // The production seam the mesher itself calls, driven by a world built
        // from the captured registry entry.
        let w0 = World::for_dimension(d);
        let mut want_codes: Vec<u8> = (0..6)
            .map(|face| {
                let code = rewo_mesh::face_shade_code(&w0, face);
                if rewo_mesh::FACE_SHADE[code as usize] != d.cardinal_light.by_mesh_face(face) {
                    panic!("{name}: face {face} shade code {code} is not the dimension factor");
                }
                code
            })
            .collect();
        want_codes.sort_unstable();
        want_codes.dedup();

        let mut w = World::for_dimension(d);
        w.ensure_column(0, 0);
        // Float it clear of the column floor/ceiling so all six faces are
        // visible in every shape.
        let y = d.shape.min_y + 32;
        w.set_block(8, y, 8, STONE);
        let mesh = rewo_mesh::mesh_column(&w, &tables.render, &tables.models, &tables.fluid, 0, 0)
            .ok_or_else(|| format!("{name}: the cube column meshed to nothing"))?;
        if mesh.vertices.len() != 24 {
            return Err(format!(
                "{name}: one cube produced {} vertices, expected 24 (6 faces x 4)",
                mesh.vertices.len()
            ));
        }
        let mut got: Vec<u8> = mesh.vertices.iter().map(|v| v.shade_code()).collect();
        got.sort_unstable();
        got.dedup();
        if got != want_codes {
            return Err(format!(
                "{name}: meshed shade codes {got:?}, expected {want_codes:?}"
            ));
        }
        let sky = mesh
            .vertices
            .iter()
            .map(|v| v.sky_light())
            .max()
            .unwrap_or(0);
        if !d.has_sky_light && sky != 0 {
            return Err(format!(
                "{name}: has_skylight=false but a vertex carries sky light {sky} — the \
                 column's stored full-bright sky nibbles reached the GPU"
            ));
        }
        if d.has_sky_light && sky == 0 {
            return Err(format!(
                "{name}: has_skylight=true but every vertex has sky light 0 — the probe \
                 is vacuous, so the no-skylight assertion above proves nothing"
            ));
        }
        vertices += mesh.vertices.len();
        shade_codes.push(got);
        max_sky.push(sky);
    }
    Ok(MeshReport {
        dimensions: defs.len(),
        vertices,
        shade_codes,
        max_sky,
    })
}

// ----------------------------------------------------------- generation fence

struct FenceReport {
    first: bool,
    resubmit: bool,
    next_generation: bool,
    drained: usize,
    generations: Vec<u64>,
}

/// The production mesh pool's staleness fence, driven across two genuinely
/// different dimension worlds: identity is `(generation, cx, cz)`, so the same
/// column may re-enter under a new generation while the old world's job is
/// still in flight, and every output names the generation it was baked from so
/// the caller can drop the stale one.
fn check_generation_fence(defs: &[DimensionTypeDef]) -> Result<FenceReport, String> {
    let overworld = defs
        .iter()
        .find(|d| d.name == "minecraft:overworld")
        .ok_or("no overworld entry to fence against")?;
    let nether = defs
        .iter()
        .find(|d| d.name == "minecraft:the_nether")
        .ok_or("no nether entry to fence against")?;

    let mut ow = World::for_dimension(overworld);
    ow.ensure_column(0, 0);
    ow.set_block(8, overworld.shape.min_y + 32, 8, STONE);
    let mut nz = World::for_dimension(nether);
    nz.ensure_column(0, 0);
    nz.set_block(8, nether.shape.min_y + 32, 8, STONE);

    let mut pool = MeshPool::new(cube_tables())?;
    let first = pool.submit(0, &ow, 0, 0);
    let resubmit = pool.submit(0, &ow, 0, 0);
    let next_generation = pool.submit(1, &nz, 0, 0);
    if !first {
        return Err("the pool refused the first submit".into());
    }
    if resubmit {
        return Err("the pool accepted the same (generation, column) twice".into());
    }
    if !next_generation {
        return Err(
            "the pool refused a new generation for a column still in flight — a \
             dimension change could never re-mesh (0,0)"
                .into(),
        );
    }

    // Drain both. Bounded: a hung pool must fail, not spin.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut outputs = Vec::new();
    while outputs.len() < 2 {
        if let Some(out) = pool.try_recv() {
            outputs.push(out);
            continue;
        }
        if std::time::Instant::now() > deadline {
            return Err(format!(
                "mesh pool produced {} of 2 outputs in 30s",
                outputs.len()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let mut generations: Vec<u64> = outputs.iter().map(|o| o.generation).collect();
    generations.sort_unstable();
    if generations != vec![0, 1] {
        return Err(format!(
            "outputs carry generations {generations:?}, expected [0, 1] — a result that \
             cannot name its world cannot be fenced"
        ));
    }
    // The stale one is identifiable, and the two worlds really did produce
    // different geometry contracts (else the fence would be untestable).
    for out in &outputs {
        let mesh = out
            .mesh
            .as_ref()
            .ok_or_else(|| format!("generation {} meshed to nothing", out.generation))?;
        let sky = mesh
            .vertices
            .iter()
            .map(|v| v.sky_light())
            .max()
            .unwrap_or(0);
        let want_zero = out.generation == 1; // the Nether submit
        if want_zero && sky != 0 {
            return Err("the Nether generation's mesh carries sky light".into());
        }
        if !want_zero && sky == 0 {
            return Err("the Overworld generation's mesh carries no sky light".into());
        }
    }
    Ok(FenceReport {
        first,
        resubmit,
        next_generation,
        drained: outputs.len(),
        generations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled() -> Vec<DimensionTypeDef> {
        dimension_parse::parse_dimension_registry_packet(&dimension_parse::builtin_registry_body())
            .expect("bundled body parses")
            .expect("bundled body is the dimension_type registry")
    }

    /// The four real datagen files, read off disk by the independent JSON
    /// oracle. Fails closed: there is no "the files were absent so we skipped"
    /// arm, because a check that can vanish is not a check.
    fn decompiled_json() -> Vec<dimension_json::JsonDimension> {
        dimension_json::load(&dimension_json::default_data_root("26.2"), &BUILTIN_ORDER)
            .expect("the decompiled 26.2 dimension_type JSON must be readable")
    }

    /// The bundled transcription is graded against the **actual decompiled
    /// JSON files**, field by field, by a reader that shares no code with the
    /// parser that produced the transcription: `dimension_json` walks
    /// `serde_json` and never touches `rewo_proto::nbt`. A fixture that drifted
    /// from the shipped datagen fails here even though it still parses.
    #[test]
    fn the_bundled_transcription_matches_the_decompiled_json_files() {
        let defs = bundled();
        let json = decompiled_json();
        assert_eq!(defs.len(), json.len());
        for (holder, j) in json.iter().enumerate() {
            j.diff("bundled", holder, &defs[holder]).unwrap();
        }
    }

    /// The hand-written [`EXPECT`] table is graded by the same files, so it
    /// cannot go stale unnoticed — and it stays in the loop so that a JSON
    /// reader and a parser that mis-read the *same* field still fail.
    #[test]
    fn the_expectation_table_matches_the_decompiled_json_files() {
        let json = decompiled_json();
        assert_eq!(EXPECT.len(), json.len());
        for (holder, expect) in EXPECT.iter().enumerate() {
            expect
                .grade("decompiled-json", holder, &json[holder].to_def())
                .unwrap();
        }
    }

    /// The JSON oracle must be able to *fail*: a transcription that differs
    /// from the files in any consumed field is rejected, and the diagnostic
    /// names the field and the file.
    #[test]
    fn the_json_oracle_rejects_a_drifted_transcription() {
        let json = decompiled_json();
        let mut defs = bundled();
        // Nether graded against the Overworld's file.
        assert!(json[0].diff("bundled", 0, &defs[3]).is_err());
        // One field at a time, on the entry that owns it.
        defs[3].shape = DimensionShape::OVERWORLD;
        let err = json[3].diff("bundled", 3, &defs[3]).unwrap_err();
        assert!(err.contains("min_y"), "{err}");
        assert!(err.contains("the_nether.json"), "{err}");
        let mut drifted = bundled();
        drifted[3].has_day_timeline = true;
        assert!(json[3].diff("bundled", 3, &drifted[3]).is_err());
        let mut drifted = bundled();
        drifted[3].sky_color = Some(0);
        let err = json[3].diff("bundled", 3, &drifted[3]).unwrap_err();
        assert!(err.contains("sky_color"), "{err}");
    }

    /// The one property the fixtures cannot prove about themselves: the day
    /// timeline is resolved out of the shipped `tags/timeline/*.json`, and it
    /// is independent of `has_fixed_time`.
    #[test]
    fn the_day_timeline_is_resolved_from_the_decompiled_tag_files() {
        let json = decompiled_json();
        assert_eq!(json[0].timelines_raw, vec!["#minecraft:in_overworld"]);
        assert!(json[0].has_day_timeline && !json[0].has_fixed_time);
        for holder in [2usize, 3] {
            assert!(
                json[holder].has_fixed_time && !json[holder].has_day_timeline,
                "{}",
                json[holder].name
            );
        }
        // And the derivation really did expand the tags rather than name them.
        assert!(json[0].timeline_ids.contains(&"minecraft:day".to_string()));
        assert!(json[0]
            .timeline_ids
            .contains(&"minecraft:villager_schedule".to_string()));
        // The bundled fixtures and the production parser agree with that.
        let defs = bundled();
        for (holder, j) in json.iter().enumerate() {
            assert_eq!(defs[holder].has_day_timeline, j.has_day_timeline);
            assert_eq!(defs[holder].has_fixed_time, j.has_fixed_time);
        }
    }

    /// Fail closed on the JSON side too: a missing decompile is an error, not a
    /// silently skipped comparison.
    #[test]
    fn a_missing_decompile_is_an_error() {
        let missing = std::env::temp_dir().join("rewo-dimensioncheck-no-decompile");
        let _ = std::fs::remove_dir_all(&missing);
        assert!(dimension_json::load(&missing, &BUILTIN_ORDER).is_err());
    }

    /// A wrong value must fail the grader, not be tolerated — the oracle's own
    /// anti-vacuity check.
    #[test]
    fn the_grader_rejects_a_wrong_entry() {
        let defs = dimension_parse::parse_dimension_registry_packet(
            &dimension_parse::builtin_registry_body(),
        )
        .unwrap()
        .unwrap();
        // Nether graded against the Overworld's expectation.
        assert!(EXPECT[0].grade("bundled", 3, &defs[3]).is_err());
        let mut bad = defs[3].clone();
        bad.shape = DimensionShape::OVERWORLD;
        assert!(EXPECT[3].grade("bundled", 3, &bad).is_err());
        let mut bad = defs[3].clone();
        bad.has_day_timeline = true;
        assert!(EXPECT[3].grade("bundled", 3, &bad).is_err());
    }

    /// The world/mesh/fence properties, on the bundled definitions, so they run
    /// in `cargo test` with no recording on disk.
    #[test]
    fn the_bundled_definitions_bind_to_world_and_mesh() {
        let defs = dimension_parse::parse_dimension_registry_packet(
            &dimension_parse::builtin_registry_body(),
        )
        .unwrap()
        .unwrap();
        let w = check_world_binding(&defs).unwrap();
        assert_eq!(w.dimensions, 4);
        assert_eq!(w.shape_probes, 16);
        assert_eq!(w.light_probes, 16);
        let m = check_mesh_binding(&defs).unwrap();
        assert_eq!(m.vertices, 96);
        // The Nether — holder 3 in the real synced order — is the one dimension
        // whose faces move off their historical codes, and the only one whose
        // vertices may not carry sky light.
        assert_eq!(defs[3].name, "minecraft:the_nether");
        assert_eq!(m.shade_codes[3], vec![2, 3, 4, 5, 6]);
        assert_eq!(m.max_sky[3], 0);
        assert_eq!(m.shade_codes[0], vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(m.max_sky[0], 15);
        check_generation_fence(&defs).unwrap();
    }

    /// Fail closed: a recording with no dimension_type registry is an error,
    /// never a silently skipped check.
    #[test]
    fn a_recording_without_the_registry_is_an_error() {
        let dir = std::env::temp_dir().join("rewo-dimensioncheck-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty.rewo");
        let rec = rewo_net::record::Recorder::create(&path).unwrap();
        rec.finish().unwrap();
        assert!(captured_registry(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
