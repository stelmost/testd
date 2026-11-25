---
title: Releasing
description: 'Release checklist, Homebrew definitions, and signing and notarization.'
---

# Releasing

A free release is a version tag, CLI archives, a pinned CLI installer, public tray bundles, workspace SBOMs, generated Homebrew definitions, and `SHA256SUMS`. The tag workflow uploads them and publishes the GitHub release automatically only after required unprivileged checks (including dependency policy/MSRV), minimum-runtime fixtures, website, artifacts, and SBOM gates pass. No commercial service or updater participates.

### Before tagging

- [ ] `python3 tests/run.py ci`
- [ ] `python3 tests/run.py site`
- [ ] `python3 scripts/release.py verify-version`
- [ ] Dispatch the full workflow on a non-tag branch at the frozen candidate SHA and retain the required job results and candidate artifact hashes
- [ ] Confirm the version tag is unused locally and on the release remote; `v0.1.0-alpha` is an older test tag and must not be moved
- [ ] Enable GitHub private vulnerability reporting for the public repository
- [ ] Retain required Debian 13 amd64, macOS 15 arm64, macOS 26 arm64, and Windows 10 22H2 amd64 VM signoffs and browser reports; both Macs must validate the same Mac 15-built ARM artifact; physical-hardware-specific checks are deferred
- [ ] `scripts/release-sbom.sh` and attach the SBOMs to the release
- [ ] Finalize the dated changelog entry; synchronize all Cargo package versions; packaging inherits the GUI package version, and refresh `Cargo.lock` when dependencies change
- [ ] Run `testd doctor --json` against a clean VM and keep the output
- [ ] Verify the documented fresh-install path reaches a trusted HTTPS name, including Linux port privileges

### Build

Linux and macOS CLI:

```sh
cargo build --release --bin testd
```

Linux x86_64 releases provide both GNU libc and static musl CLI archives. Build the latter with the Rust target and `musl-tools` installed:

```sh
rustup target add x86_64-unknown-linux-musl
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc CC_x86_64_unknown_linux_musl=musl-gcc \
  cargo build --release --bin testd --locked --target x86_64-unknown-linux-musl
python3 scripts/release.py package --version 0.1.0-alpha.1 --target x86_64-unknown-linux-musl \
  --build-dir target/x86_64-unknown-linux-musl/release --output dist --cli-only
```

CI verifies that the musl executable has no dynamic loader or shared-library dependencies and runs its version/help commands. Linux desktop packages use the GNU target. The CLI installer detects GNU libc and otherwise selects musl; `TESTD_LIBC=gnu` or `TESTD_LIBC=musl` overrides that selection.

Windows CLI and tray (PowerShell):

```powershell
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = '-C target-feature=+crt-static'
cargo build --release --bin testd --locked --target x86_64-pc-windows-msvc
cargo build --release -p testd-gui --locked --target x86_64-pc-windows-msvc
cargo packager --release --packages testd-gui --target x86_64-pc-windows-msvc --formats nsis `
  --binaries-dir target/x86_64-pc-windows-msvc/release `
  --out-dir target/x86_64-pc-windows-msvc/release/bundle
```

Windows release executables link the MSVC runtime statically. The CLI ZIP requires no separately installed Visual C++ redistributable. Keep `--target` explicit for Windows release builds.

Linux and macOS tray:

```sh
cargo build --release -p testd-gui --locked
cargo packager --release --packages testd-gui --formats default \
  --out-dir "$PWD/target/release/bundle"
```

Use the explicit release formats from the CI matrix, and a fresh output directory for each candidate. Package files are placed directly in `target/release/bundle/`; `scripts/release.py` stages their stable public names without collecting nested tool executables.

Artifacts: CLI `.tar.gz` (Linux/macOS) or `.zip` (Windows), `.dmg` containing `.app` (macOS), `.exe` (Windows NSIS), `.AppImage`/`.deb` (GNU Linux). CI requires macOS arm64, GNU and musl Linux x64 CLI archives, GNU Linux desktop packages, and Windows x64 builds; Intel macOS is best effort. Build one ARM artifact on macOS 15 for separate native validation on 15 and 26, not competing ARM bundles from different builders.

AppImage bundles the linked GTK libraries and preserves ordinary GTK backend selection, including an explicit `GDK_BACKEND`. Its startup hook saves the host environment for browser and application launches before the bundled runtime changes it. Validate the actual desktop/tray host, host-browser handoff and a normal quick launch without XWayland before claiming support; packaging alone is not native desktop evidence.

Windows ships only NSIS, configured in `crates/testd-gui/Cargo.toml` to install per user into `%LOCALAPPDATA%\Testd` without installer elevation. Resolver and trust changes retain their separate native authorization prompts. No MSI or machine-wide deployment is claimed. Preserve the actual installer/shortcut/URL-handler identity when validating upgrades; a matching visible product name alone is not evidence.

Removing the app does not remove the machine trust root or the DNS route. On Linux, macOS, and Windows, run `testd uninstall` before removing the app; ownership-checked cleanup refuses changed or unowned integration. Add `--purge` only to delete local configuration after system cleanup succeeds. macOS cleanup requires the logged-in desktop session and native authorization. Windows cleanup requests native administrator authorization through UAC.

### Sign

The first release is an unsigned alpha. Do not purchase signing certificates or Apple Developer Program membership for this alpha.

- macOS: Optional free ad-hoc sealing only when useful for bundle resource integrity; no Developer ID, notarization, or Gatekeeper approval claim
- Windows: Remains unsigned; no Authenticode or SmartScreen reputation claim
- Linux: Checksums and workspace SBOMs; no additional signing infrastructure

Validate any ad-hoc seal on the final app before producing its DMG, then generate Homebrew definitions and checksums from those final bytes. Bundle sealing and package creation do not replace native installation approval. Keep the privacy manifest from `packaging/macos/PrivacyInfo.xcprivacy`.

[Signing and notarization](#signing-and-notarization) is a credentialed reference outside this alpha. [Homebrew](#homebrew) and [Browser validation](release-validation.md#browser-validation) cover their separate checks.

The desktop app and host daemon optionally check for newer releases at startup and daily. Users can disable checks persistently in the dashboard. Releases are installed by the user: the archive, `brew upgrade`, or the system package manager. No running executable is replaced automatically.

A Linux container can build/test the CLI and native tray and run `cargo packager`, but
cannot produce notarized or Authenticode-signed artifacts.

### Publish

- [ ] Tag `vX.Y.Z` and push the tag after the pre-tag checks
- [ ] Let CI build CLI archives and tray bundles, generate the pinned installer, SBOMs, Homebrew definitions and checksums, upload all assets, and publish the GitHub release automatically
- [ ] Download the published assets and confirm they match the validated candidate
- [ ] Copy the generated `testd.rb` and `testd-gui.rb` into the Homebrew tap, then audit and install them locally
- [ ] Confirm the published release includes the changelog entry and retain the pre-tag validation results
- [ ] After the release assets are public, publish the site with the matching root `Cargo.toml` package version

The public workflow does not use trusted distribution signing. A tag-push run creates a draft only if the release does not already exist, uploads all assets, then publishes it. Failed uploads leave a new release as a draft. Prerelease tags retain their prerelease status. Manual workflow runs build candidate artifacts without publishing. A rerun can add missing assets to an existing release, including a public one, and publishes an existing draft after a successful upload, but does not replace existing assets. Do not rerun against a public release unless adding assets is intended. Back up any asset before explicitly replacing it, and do not move an existing release tag to different source.

### CLI installer

Each release attaches an `install.sh` whose default is that release's version. `TESTD_VERSION` overrides the default; an unset or empty value uses the pin. The installer downloads exact-version assets and verifies their checksums without a release-discovery request or GitHub token. The generated installer is included in `SHA256SUMS`, and release notes link to that asset.

The root `install.sh` defaults to `latest`, resolved through GitHub's `releases/latest` redirect before downloading exact-version assets. GitHub's latest release excludes prereleases; use an explicit `TESTD_VERSION` or a generated installer for those. No GitHub token is required or sent.

The product site reads the root `Cargo.toml` package version for its version label, download links, and `/update.json`. Vite replaces the `latest` default with that version in the served and built `/install.sh`, leaving the source unchanged. Release installer generation also leaves the source unchanged.

Deploy the site through the normal Pages Git integration after the matching release assets are public. Publishing the site advertises the Cargo version to installed hosts with update checks enabled; do not deploy a draft or unpublished version. Existing release assets and tags retain their original installer bytes.

### Rollback

- GitHub: mark the broken version as withdrawn and publish a fixed version. Keep the previous good version and its checksums available. Existing installs keep working until the user chooses to replace them.
- Installer and website: rebuild and deploy the site from the previous published release's Cargo version
- Homebrew: revert the formula commit and `brew update`.
- macOS and Windows bundles are versioned per release, so a broken version can be
  withdrawn without affecting the previous one.

## Homebrew

The formula ships the CLI and daemon; the cask ships the tray app.

### Tap layout

Homebrew wants a repository named `homebrew-<tap>`. For `brew tap stelmost/testd`:

```
homebrew-testd/
  Formula/testd.rb
  Casks/testd-gui.rb
```

`packaging/homebrew/*.rb.in` are the source templates. The release generator fills a version and real source/DMG digests and attaches installable `.rb` definitions to the public release. The cask requires the Apple Silicon DMG and includes Intel only when its final asset exists; an arm-only cask explicitly requires arm64.

### Local test before publishing

Run installation/audit checks only with owner authorization on separately restored, backed-up macOS 15 and 26 arm64 VMs using the same Mac 15-built ARM artifact. Retain separate results for each OS. Definition generation is not installation or signing approval.

```sh
# Definitions downloaded from the release (or generated locally from its final assets)
brew install --formula ./dist/testd.rb
brew install --cask ./dist/testd-gui.rb
brew test testd
brew audit --strict --online ./dist/testd.rb
brew uninstall --formula testd
brew uninstall --cask testd-gui
```

Verify actual installation and Gatekeeper behavior on an authorized clean Mac. An unsigned or unnotarized app may require a user override; successful ad-hoc resource sealing is not Gatekeeper approval or a trusted signed-install experience.

### Digests

After the version tag exists and the required final Apple Silicon DMG is in `dist/` (include the optional Intel DMG only when available):

```sh
python3 scripts/release.py homebrew --version 0.1.0 --artifacts dist --output dist
python3 scripts/release.py checksums dist
```

The source archive is fetched once and cached as `dist/testd-0.1.0-source.tar.gz`. The generator hashes that archive, the required Apple Silicon DMG, and any supplied Intel DMG; missing arm assets or invalid supplied assets fail rather than receiving placeholder checksums. It refuses to overwrite existing definitions or checksums and retains failed downloads as `.partial`. These digests detect changed bytes, not signing or native installation provenance. Use a new staging directory after signing or changing assets; retain the previous bytes.

### Formula notes

- The formula builds from the source tarball with `cargo install` and declares `depends_on "rust" => :build`.
- `service do` runs `testd host` as a user agent with `keep_alive true`. `brew services start testd` is the supported path; the tray app is the alternative.
- The formula's `test do` runs `testd --help` and `testd doctor --json`. `doctor` exits non-zero without a config, so the test tolerates that exit code.

### Cask notes

- `depends_on macos: ">= :ventura"`.
- `app "Testd.app"`.
- `zap trash: "~/.config/testd"` only after the root and the DNS route are removed; `testd uninstall` does both. Document that order in the release notes, not as a cask stanza that silently deletes state.

### Bumping a release

1. Synchronize package and bundle versions and finalize the changelog entry
2. Tag the release and generate definitions from its final source archive and DMGs
3. Run `brew audit --strict --online`, `brew test testd`, and fresh formula/cask installs
4. Publish the matching definitions to the tap

## Signing and notarization

The first release is an unsigned alpha: Windows stays unsigned, and Linux uses checksums/SBOMs without new signing infrastructure. Free macOS ad-hoc resource sealing may be used only for bundle integrity; it is not Developer ID signing, notarization, or Gatekeeper approval. No paid certificate or Apple Developer Program purchase is required or authorized for this alpha.

The credentialed macOS notarization and Windows Authenticode instructions below are reference material for separately approved trusted-signed delivery outside this alpha. They are not alpha prerequisites and do not authorize purchases. Source publication and CLI source builds do not require these credentials.

### macOS

#### Prerequisites

- Apple Developer Program membership (99 USD/year).
- A `Developer ID Application` certificate in the login keychain, with its private key. `security find-identity -v -p codesigning` lists it.
- An app-specific password for the Apple ID, or an App Store Connect API key.
- Xcode command line tools: `codesign`, `xcrun notarytool`, `stapler`, `spctl`.

Store the notary credentials once:

```sh
xcrun notarytool store-credentials testd-notary \
  --apple-id you@example.com \
  --team-id TEAMID1234 \
  --password "$APP_SPECIFIC_PASSWORD"
```

#### Build and sign

Use a clean worktree and fresh bundle directory. Build only the `.app` first so the disk image is created from the signed bundle, not from an earlier unsigned copy:

```sh
cargo build --release -p testd-gui --locked
cargo packager --release --packages testd-gui --formats app \
  --out-dir "$PWD/target/release/bundle"
APP=target/release/bundle/Testd.app
DMG=target/release/bundle/Testd-signed.dmg
```

Back up existing signed artifacts before replacing signatures. Sign nested binaries/frameworks first if any are added, then sign and verify the app. Create the disk image only afterward:

```sh
codesign --force --options runtime --timestamp \
  --sign "Developer ID Application: Your Name (TEAMID1234)" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
codesign -d --entitlements - "$APP"
hdiutil create -volname Testd -srcfolder "$APP" -format UDZO "$DMG"
codesign --force --timestamp \
  --sign "Developer ID Application: Your Name (TEAMID1234)" "$DMG"
```

The app is not sandboxed, embeds no webview, and uses no JIT; it requires no added entitlements. Runtime system changes still use the existing authorization helpers. Package metadata is in `crates/testd-gui/Cargo.toml`; do not assume the former Tauri signing environment variable configures cargo-packager.

#### Notarize

```sh
xcrun notarytool submit "$DMG" --keychain-profile testd-notary --wait
xcrun stapler staple "$DMG"
xcrun stapler validate "$DMG"
```

`--wait` prints the submission id and the result. Keep the id in the release notes. Notarize the `.dmg` after signing the `.app` inside it, or notarize both; stapling the `.dmg` is what users download.

#### Verify as a user would

```sh
spctl -a -vv -t exec "$APP"
```

Expect `accepted` and `source=Notarized Developer ID`. Also copy the `.dmg` to a clean account, mount it, and launch once: Gatekeeper must accept the first run without a right-click.

#### Common failures

| Symptom | Cause |
|---|---|
| `Invalid` with "The signature of the binary is invalid" | A nested binary was signed after the bundle, or not at all. Re-sign inside out. |
| `Invalid` with "hardened runtime is not enabled" | Missing `--options runtime`. |
| `Invalid` with "The executable does not have the hardened runtime enabled" on a dependency | A sidecar or dylib shipped unsigned. |
| `spctl` rejects but notarization passed | Staple was skipped; the app was moved out of the stapled dmg. |
| Submission stuck in `In Progress` for hours | Apple side; resubmit with `--wait` and the same profile. |

### Windows

- Sign the NSIS installer and the binaries with an Authenticode certificate (`signtool sign /fd sha256 /tr http://timestamp.digicert.com /td sha256`).
- Submit the installer to SmartScreen once per certificate; fresh certificates start without reputation and gather it per download.
- This repository does not ship an updater; see [Releasing](#releasing).

### What not to sign

The root certificate and its private key are local data, not artifacts. They never enter the release pipeline.
