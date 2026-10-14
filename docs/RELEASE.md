# Cutting an EwoClient release

The procedure for turning `main` into a bundle someone else can run. Written
2026-10-15; nothing has been released with it yet. Releases are the owner's
call: an agent prepares and checks a bundle, it never publishes one.

## 1. What must be green first

Run on the owner's machine (the self-hosted gates need the local 26.2 assets
and the GPU):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features rewo-app/gates -- -D warnings
REWO_REQUIRE_ASSETS=1 cargo test --workspace --no-fail-fast --features rewo-app/gates
python tools/gates.py
python tools/render_check.py
python tools/check_mixin_targets.py
powershell ingame-mod/build.ps1 -NoDeploy
```

`REWO_REQUIRE_ASSETS=1` turns every `SKIP:` into a failure, so a green run
means every test ran. CI (`ci.yml`) covers the asset-free half; `gates.yml`
runs the rest on the self-hosted runner on push.

## 2. Build the bundle

```powershell
.\package.ps1 -NoShortcut -Zip -LoaderRepo C:\path\to\EwoLoaderV1
```

- Builds the legit HUD jar from this checkout (`ingame-mod\build.ps1
  -NoDeploy`), bundles the EwoLoader manifests with that jar substituted for
  the dev drop's, builds `ewolauncher.exe`, `rewo.exe` and `ewo_jni.dll` in
  release with **default features** (no `gates`, no `pvp`, no `audio`, no
  `profile`), and stages `dist\EwoClient\`.
- Writes `dist\EwoClient\BUILD.txt` (version, commit, whether the tree was
  dirty). **Release only from a clean tree**: a dirty bundle cannot be rebuilt
  from its commit.
- Fails unless `tools\check_bundle.py` passes: the binaries, fonts, icon and
  licence are present; every loader manifest is self-contained (no `file://`,
  no absolute path) and every bundled jar matches its manifest's sha1 and
  size; `rewo.exe` carries no gate code.
- `-Zip` writes `dist\EwoClient-<version>-<commit>-windows-x64.zip`.

The version is `[workspace.package] version` in `Cargo.toml`. Bump it in its
own commit before packaging a release that is not the first.

## 3. What a person still has to check

No gate reaches these. Each needs the real launcher, a real account or a real
device:

- **Fresh-machine run.** Unzip on a machine (or Windows user profile) that has
  never had EwoClient: the launcher opens, signs in, downloads 26.2, and an
  Ewo instance launches with the HUD (the bundled loader and HUD jar, not a
  dev path). Nothing in `%APPDATA%\EwoClient\dev` should be needed.
- **The in-game HUD** in a running Minecraft: widgets draw, the overlay opens
  and closes, the ABI handshake passes (a mismatch logs and leaves the HUD off).
- **A Native (Rewo) instance** joins an offline server and a Frogsy server.
- **Rewo audio** (`--features audio` is not in the default bundle): the
  listening pass in `REWO_AUDIO_PLAN.md` §4.
- **Pixel parity** of the launcher against `style/*.png` (owner's eye).

The running list of what is waiting on a person, with dates, is the latest
`docs/OVERNIGHT-*.md` "Needs a playtest" section plus `docs/AUDIT-2026-10.md`
§11's "Still open".

## 4. Not part of a release

- The PvP build (`build.ps1 -Pvp`, `--features pvp`) is never bundled.
- Nothing is auto-updated or announced: the bundle is a folder to unzip
  ("OFFLINE FIRST. NOTHING PHONES HOME.").
- The social API stays off unless the user sets `EWO_BOT_API_BASE`.
