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

**EwoLoader** — a friendly fork of `fabric-loader` (sibling repo) that ships 16 user-toggleable mods plus infrastructure, with per-instance toggles wired end to end.

**An in-game GUI.** A Rust/Skia HUD rendered over the running game from a `cdylib` on its own GL context: FPS/coords/ping/keystrokes/armor/potions/target, a draggable editor, a custom crosshair, an SMTC media controller, a 3D skin viewer, and a multi-tab dashboard — plus client *modules* (12 legit ones by default; a separate `--features pvp` build carries the packet-touching assist set).

**Accounts, profiles and social.** Multi-account, hot-swappable client profiles, a remappable keybind registry, and friends/presence integrated with the user's own Minecraft network.

**[Rewo](REWO_PLAN.md)** — a from-scratch native Minecraft client, in the same workspace under `crates/rewo-*`. It speaks the vanilla protocol (26.2 / protocol 776) and renders with raw Vulkan via `ash`, no JVM and no mod loader. It plays online: signed chat, real skins, OptiFine CEM packs rendered natively, a server-exact client light engine, vanilla's lightmap and day/night sky, per-biome colour, real Nether/End dimensions, 88 mob models with their exact animations, the combat and block-entity arcs, weather and particles — and the whole GUI half: the inventory with its 3D player preview, the first-person hand, 25 container screens, item tooltips and durability bars, the recipe book end to end, and chat — the wrapped, fading HUD box, the screen you type into, a command line with local Brigadier parsing, autocomplete, syntax highlighting and usage hints for every vanilla argument type, and translated text (join and death messages and command feedback read as English rather than as `multiplayer.player.joined`). Verified headlessly by 33 serverless gate commands, each fail-closed and most with Vulkan validation on, plus a `live --render-check` that drives the *windowed* client against a real server.

## What's next

- **Audio is the largest single gap and needs a decision**: the packets, the 1,968-entry sound registry and the weighted-variant index all decode, and nothing makes a sound. It needs a crate (cpal / rodio / kira) and it is the first Rewo milestone that cannot be verified headlessly end to end, so what the gate asserts has to be settled up front.
- The **chat decoration** — `boundChatType.decorate`, which turns a message into `<Steve> hi`. M125 shipped the translatable resolution underneath it and verified both halves of its old blocker (the `chat_type` registry is a synchronised datapack registry; the language table is loaded), so this is scheduling rather than a wall. The **styled chat pipeline** is the larger sibling: the chat store is plain-text, so a `/msg`'s grey italic and clickable text both wait on it.
- [`REWO_FEATURE_SURVEY.md`](REWO_FEATURE_SURVEY.md) is the roadmap for features rather than milestones — audit its items against the crates first, since five are already at vanilla parity.
- Hyprland verification for the launcher; a formal pixel-parity pass vs `style/*.png`

([`REWO_PACKET_COVERAGE.md`](REWO_PACKET_COVERAGE.md) is at 118 handled / 0 ignored / 23 absent — 11 not-applicable plus 12 needing a subsystem Rewo lacks, so picking work there means choosing a subsystem rather than a packet. Its table is machine-checked by a unit test in `ids.rs`.)

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
