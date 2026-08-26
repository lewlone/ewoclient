# EwoClient

A custom Minecraft Java Edition launcher with a "Velvet & Pearl" boudoir aesthetic. Native Rust + Skia, custom-frame window, target rendering 500fps on a 500Hz OLED — plus an in-game GUI, a friendly fork of the Fabric loader, and a from-scratch native Minecraft client.

> ⚠️ Personal project. Windows is the developed-and-tested target; Linux/Wayland is written but has never been run on real hardware. macOS and X11 are out of scope.

## Screenshots

![Main menu](style/mainMenu.png)
![Instances](style/Instances.png)
![Settings](style/SettingsGraphics.png)
![Launching](style/Launching.png)

## What's done

**The launcher.** A pixel-faithful native port of the CSS prototype — custom-frame transparent window, the full animated backdrop (velvet folds with turbulence + displacement, caustics, bokeh, pearl dust, petals), the glass-panel primitive, every widget and screen, modals, TOML persistence, and a `--dev` overlay with live token tweaks and an FPS HUD.

**It actually launches Minecraft.** Microsoft OAuth → Xbox Live → XSTS → Minecraft Services (the app is on Mojang's allowlist and sign-in is verified live), the real version manifest with sha1-verified library/asset downloads into Mojang's own disk layout, JRE detection with automatic Adoptium fetch, and JVM spawn with stdout/stderr streaming into the launching screen.

**EwoLoader** — a friendly fork of `fabric-loader` (sibling repo) that ships 17 user-toggleable mods plus infrastructure, with per-instance toggles wired end to end.

**An in-game GUI.** A Rust/Skia HUD rendered over the running game from a `cdylib` on its own GL context: FPS/coords/ping/keystrokes/armor/potions/target, a draggable editor, a custom crosshair, an SMTC media controller, a 3D skin viewer, and a multi-tab dashboard — plus client *modules* (12 legit ones by default; a separate `--features pvp` build carries the packet-touching assist set).

**Accounts, profiles and social.** Multi-account, hot-swappable client profiles, a remappable keybind registry, and friends/presence integrated with the user's own Minecraft network.

**[Rewo](REWO_PLAN.md)** — a from-scratch native Minecraft client, in the same workspace under `crates/rewo-*`. It speaks the vanilla protocol (26.2 / protocol 776) and renders with raw Vulkan via `ash`, no JVM and no mod loader. It plays online: signed chat, real skins, OptiFine CEM packs rendered natively, a server-exact client light engine, vanilla's lightmap and day/night sky, per-biome colour, real Nether/End dimensions, 88 mob models with their exact animations, the combat and block-entity arcs, weather and particles — and the whole GUI half: the inventory with its 3D player preview, the first-person hand, 25 container screens, item tooltips and durability bars, the recipe book end to end, and chat — the wrapped, fading HUD box, the screen you type into, a command line with local Brigadier parsing, autocomplete, syntax highlighting and usage hints for every vanilla argument type, translated text (join and death messages and command feedback read as English rather than as `multiplayer.player.joined`), and styled text (a message is a list of styled spans from the wire to the glyph, so a server's colours survive the wrap and bold / italic / underline / strikethrough / obfuscated all draw). Chat is now decorated the way its chat type says (`<Steve> hi`, and a `/msg` in its grey italic), its links are clickable, the command line reports its parse errors, and both scoreboard surfaces draw — the sidebar and the **tab list** you hold Tab for, which since M155 carries its players' 8x8 faces and a `RenderType::HEARTS` health column with vanilla's blink clock. The dark-GUI arc is closed too: the advancements tree (M177–M180, clicks included), the written-book reader with clickable page text, the sign editor, the volume sliders, the survival gauges and the leash rope all draw and respond like their vanilla counterparts. Verified headlessly by **43** serverless gate commands, each fail-closed and most with Vulkan validation on, plus a `live --render-check` that drives the *windowed* client against a real server.

## What's next

- **M152–M157 (2026-08-14) closed every item the plan's handoff was carrying.** M152 decoded `update_recipes` and with it the smithing table's shift-click — the last quick-move decline in the container arc. M153 turned the mixer's stereo-attenuation divergence into an owned decision the plan had asked for and nobody had made. M154 fixed a live bug found while scoping the options work: the client passed a literal `1.0` for `getMusicVolume()` under a comment asserting no biome declares the attribute, and `pale_garden` declares **0.0** — so music should be silent there and was not. M155 closed the tab list's two gaps and found the "structural" reason given for one of them does not survive the code. M156 moved the static audio decode onto a worker, **measured at 20.1 ms against a 50 ms tick** for the largest static asset. M157 made the two real client options real — and the third the plan named was never an option at all.

- **Audio is wired into the client and nobody has listened to it yet.** `crates/rewo-audio` carries the quantisation, the buffer library, the mixer, an SPSC command ring, a cpal sink, (M143) the backend the sound engine drives and (M144) the incremental Ogg stream behind music and the ambient beds; `rewo-net` carries the listener transform, the music fade and the `ChannelSink` seam. **M145–M146** then shipped music proper — the seven tracks, the `audio/background_music` attribute, and the whole of `MusicManager` — and **M147** shipped the bug that had been hiding inside it. **The listening pass is the outstanding work and it is a human's.** No gate in this project opens an audio device — an absent, muted, exclusive-mode or unplugged one all look identical from inside the process — so everything a machine can check passes, and that is *not* the same claim.

  **M147 is the clearest evidence for that sentence in the whole project.** M146 landed with 3137 tests, 34 gates and 45 render-check witnesses green — and no music played anywhere in the Overworld, because the Overworld's music lives on the **dimension type** rather than on any biome, and the survey that concluded otherwise had read the writers through `head -20`. The entire suite agreed with itself. Running the client for eight seconds found it.

  ```
  cargo build -p rewo-app --features audio    # a default build links NO audio stack
  rewo live --audio                           # or REWO_AUDIO=1
  cargo run -p rewo-audio --example listen    # one sound, no server
  ```

  The feature is **off by default**, so a default build links **zero** audio-stack crates (measured with `cargo tree`) and `rewo live` without it is silent by construction. `soundshot` is the one gate that grades any of it, and it fail-closes on two different locks accordingly: **28** witnesses in a default build, **48** under `--features audio`. And M139's loopback oracle now measures the mixer against vanilla's own OpenAL Soft rather than against its own declarations — the distance curve is exact, the pan is not, and one divergence it found is a real behavioural difference rather than an approximation. See [`REWO_AUDIO_PLAN.md`](REWO_AUDIO_PLAN.md).
- **M141** transcribed the ten tickable sound ramps — the per-tick volume/pitch/position curves for bees, elytra, guardians, minecarts, riders, sniffers and the ambient loops — and wired the engine to drive them; **M141d** then fed them their velocity and **M141e–h** built every ordinary trigger, so an elytra, a spawning minecart, a spawning bee, a guardian's beam, a digging sniffer and a mounted rider each start, ramp and stop on their own — seven of the ten. **M142** then built that subsystem — vanilla's three `AmbientSoundHandler`s and the biome/dimension `AmbientSounds` attribute they read — so the world sounds like itself: diving starts an underwater bed that fades out when you surface, a bubble column says which way it drags, and each biome's loop crossfades into the next. Its own finding is that the class name is not the feature: the underwater *handler* plays only the three rare stings, the loop is minted by a rising edge elsewhere, and every piece of that handler's state (a tick delay, four named chance constants) is dead code. Three findings, all of the kind that punish a careful reader: `MinecartSoundInstance` shadows `AbstractSoundInstance.pitch`, so the pitch ramp it declares is dead code and the transcription that reads the class on its own gives every minecart ride a twenty-second glissando vanilla does not have; and a remote entity's `getDeltaMovement()` is a *decaying echo of the last motion packet* rather than a velocity, so a bee gliding steadily past you has its buzz fade to silence while it is visibly moving; and `canPlaySound()` is a per-class override that six of the ten sounds declare and four decline, so binding the elytra to its player — which every other tickable wants — would let a silenced player silence their own wings.
- **M149** shipped the End flash end to end — the schedule, the clock map under it, and all three consumers (the lightmap brightens, a quad draws in the sky, and a sound queues thirty ticks behind), which also gives **all eleven tickable-sound ramp variants a construction site** at last. It began as a milestone and it is the clearest example of a milestone whose scope was mis-stated in the plan that proposed it. It was listed as the last unconstructed *audio* ramp; `EndFlashState` has **three** consumers and two are visual — the lightmap brightens, a flash quad draws in the sky, and only then does a sound queue 30 ticks behind. The schedule and the `default_clock` field it is ticked from are shipped and gated; the clock map and all three consumers are named rather than half-started. Both halves of the feature were already in the tree and read by nothing: `Skybox::has_end_flashes()` since M16 with zero production readers, and `default_clock` spelled in our own bundled fixtures since M16 while the parser read past it. Its findings are the invertible kind — the **first 600 ticks of any clock never flash** (Java's `flashSeed` field defaults to 0 and the draw is guarded on a *change*, so modelling it as "not yet computed" invents a flash vanilla lacks), tick 0's intensity is genuinely `NaN` and survives only because `Mth.sin` is a table lookup, and `Mth.lerp(1.0, a, b)` is **not** a select, so at the flash's tail it cancels to exactly zero and the raw state and every renderer disagree about whether there is a flash at all.
- The chat arc is closed: **M127** shipped the decoration, **M128** clickable chat, **M129** the disconnect reason, **M130** the title's and death screen's style flags (and found the text pass had been handed sRGB bytes by nine of twenty-two callers), **M132** the scoreboard sidebar, **M133** the recipe book's widget tooltips and **M134** the command line's exception messages.
- [`REWO_FEATURE_SURVEY.md`](REWO_FEATURE_SURVEY.md) is the roadmap for features rather than milestones — audit its items against the crates first, since five are already at vanilla parity.
- Hyprland verification for the launcher; a formal pixel-parity pass vs `style/*.png`

([`REWO_PACKET_COVERAGE.md`](REWO_PACKET_COVERAGE.md) is at 124 handled / 0 ignored / 17 absent — classes A and B are empty, so picking work there means choosing a subsystem rather than a packet. Its table is machine-checked by a unit test in `ids.rs`.)

See [`CLAUDE.md`](CLAUDE.md) for the full design + roadmap, and [`REWO_PLAN.md`](REWO_PLAN.md) for the native client.

## Build

```
cargo run --release -p ewo-launcher           # normal launch
cargo run --release -p ewo-launcher -- --dev  # with dev overlay
cargo run --release -p rewo-app -- live --host … --port …   # the native client
```

Requires Rust stable 1.80+. First build is slow because Skia compiles its own C++ stack (prebuilts are used; ~30s on a recent machine).

## Stack

- **Rust** (single-threaded, no async runtime)
- **Skia** (`skia-safe` 0.78, GL backend) — all 2D rendering
- **winit** 0.30 — windowing, custom frame
- **glutin** 0.32 — GL context
- **taffy** 0.7 — layout primitives
- **ureq** + **tiny_http** + **opener** — Microsoft auth chain (loopback OAuth flow)
- Variable fonts: Fraunces (display), Newsreader (body), JetBrains Mono (mono)
- **ash** 1.3 — raw Vulkan, for the Rewo native client (`crates/rewo-*`)

## License

Source: [MIT](LICENSE).

Bundled fonts are SIL Open Font License (Fraunces, Newsreader, JetBrains Mono) — see `LICENSE` for attribution details.
