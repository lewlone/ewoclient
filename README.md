# EwoClient

A custom Minecraft Java Edition launcher with a "Velvet & Pearl" boudoir aesthetic. Native Rust + Skia, custom-frame window, target rendering 500fps on a 500Hz OLED.

> ⚠️ Personal project, work in progress. Visual replica is feature-complete on Windows; real launcher functionality (Microsoft auth + JVM spawn) is in active development. **The launch button does not yet actually launch Minecraft.**

## Screenshots

![Main menu](style/mainMenu.png)
![Instances](style/Instances.png)
![Settings](style/SettingsGraphics.png)
![Launching](style/Launching.png)

## What's done

- Custom-frame window (Win11 rounded corners, drag, resize)
- Skia GL backend with the prototype's full chrome stack (3-layer shadow, inset rim, inner berry glow)
- Animated backdrop: wine radial → velvet folds (turbulence + displacement) → caustics → bokeh → pearl dust (gradient halos) → petals → vignette
- Glass panel primitive (refract blur + 4 animated rim lights + breath)
- All widgets: vbtn (with click ripple + cursor specks), vslider, vdrop (portaled, with row stagger + scroll), vstatus, pbar (4 states + 3 error variants), vtoggle, vghost_btn, vpathfield, meta-pill
- All screens: main menu (with hover-glow stagger), instances (with sort, rename, mods toggle, delete), settings (5 tabs), launching (with synthetic log)
- Modal system (new-instance + about)
- Persistence (instances + settings + auth state via TOML at `%APPDATA%/EwoClient/`)
- Dev overlay (`--dev`) with token tweaks, FPS HUD, sim-error pill
- Microsoft OAuth + Xbox Live + XSTS chain (pending Mojang Launcher Program approval to call `login_with_xbox`)

## What's next

- Mojang Launcher Program approval (in progress — until then, `login_with_xbox` rejects the app)
- Phase B: real version manifest + library/asset downloads
- Phase C: JVM spawn + game launch
- Phase D: Fabric / Forge / Quilt loader support

See `CLAUDE.md` for the full design + roadmap.

## Build

```
cargo run --release -p ewo-launcher           # normal launch
cargo run --release -p ewo-launcher -- --dev  # with dev overlay
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

## License

Source: [MIT](LICENSE).

Bundled fonts are SIL Open Font License (Fraunces, Newsreader, JetBrains Mono) — see `LICENSE` for attribution details.
