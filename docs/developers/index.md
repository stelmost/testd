---
title: Developing testd
description: 'Support matrix, prerequisites, building, icons, and the documentation website.'
---

# Developing testd

This section covers building, testing, validating, and releasing testd.

- [Testing](testing.md): hosted CI matrix, local suite reproduction, and targeted checks
- [Release validation](release-validation.md): non-CI VM validation, host testing, and browser validation
- [Releasing](releasing.md): release checklist, Homebrew definitions, and signing/notarization
- [Dependency policy](dependency-policy.md): desktop integration library choices, advisory exceptions, and MSRV constraints

## Support matrix

The required first-release targets are exactly:

| Required OS | Architecture | Required artifact target |
|---|---|---|
| Windows 10 22H2 | amd64 | `x86_64-pc-windows-msvc` |
| Linux | amd64 | `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl` (CLI only) |
| macOS 15 | arm64 | `aarch64-apple-darwin` |
| macOS 26 | arm64 | `aarch64-apple-darwin`, shared with macOS 15 |

Linux uses Debian 13 as its native validation baseline. Other Linux distributions and Intel macOS are best effort. Physical-hardware-specific checks are deferred.

Required automated runtime gates are `macos-15` arm64, `macos-26` arm64, and Debian 13 amd64 userspace. The Debian container is not native kernel, system, or desktop evidence. Hosted Windows Server is not Windows 10 validation; Windows 10 22H2 VM signoff remains required. All four unique artifact targets are required regardless of builder OS; `x86_64-apple-darwin` is optional.

Native signoff on both macOS 15 and 26 remains required, using one ARM artifact built on macOS 15 and checking identical source/artifact hashes on each OS. Missing native macOS 15 access is an access gate, not a hardware-specific deferral; Mac 26 cannot close it. Native installation, browser/tray, resolver/trust cleanup, and approval of any claimed trusted signing remain separate from automated gates.

## Prerequisites

- Rust toolchain from `rust-toolchain.toml`; MSRV 1.90
- Python 3.12 or newer for release tooling and reports; Bun for browser fixtures
- `cargo-cyclonedx` for workspace SBOM generation
- `cargo-deny` for dependency checks; tests use native Cargo
- `cargo-packager` 0.11.8 for tray artifacts: `cargo install cargo-packager --locked --version 0.11.8`
- Native tray prerequisites: Xcode command-line tools on macOS, GTK3/AppIndicator and D-Bus development libraries on Linux, MSVC and the Windows SDK on Windows; no embedded WebKit or WebView2

## Build

From the repository root:

```sh
cargo build --release -p testd      # CLI and daemon
cargo build --release -p testd-gui  # native tray
cargo packager --release --packages testd-gui --formats default \
  --out-dir "$PWD/target/release/bundle"
```

The workspace includes `testd-gui` under `crates/`. On a machine without native GTK development libraries, add `--exclude testd-gui` to workspace test and clippy commands. CI installs the native tray prerequisites and checks the full workspace. Packaging consumes the already built executable; it does not compile another runtime or mutate the binary. Explicitly select `app,dmg` on macOS, `appimage,deb` on Linux, or `nsis` on Windows for release artifacts.

[Desktop integration library choices](dependency-policy.md#desktop-integration-libraries) define the target-specific browser/dialog dependencies, retained GTK/Gio and macOS lookup, login serialization, and security checks that must not disappear during dependency updates.

## Icons

`crates/testd-gui/icons/` is generated; its sources live in `assets/brand/`. Regenerate the bundle and tray icons with:

```sh
python3 scripts/build-icons.py
```

The script uses only the Python standard library. `testd-app.png` supplies sizes below 128px, `testd-app-large.png` supplies 128px and above, and the two 32px masks supply the tray variants. `testd-app-large.png` is a 1024px render of `testd-slim.svg`; refresh it when the SVG changes.

## Website

The product site lives under `sites/`; the documentation Markdown lives under `docs/` at the repository root. The site runs in a container, so no local `bun install` is needed to develop, build, or check it.

```sh
./sites/bin/site.sh dev      # product at http://127.0.0.1:5173/, docs at /docs/
./sites/bin/site.sh build    # dist/, with the docs under dist/docs
./sites/bin/site.sh preview  # serves dist/ at http://127.0.0.1:4173/
./sites/bin/site.sh check
./sites/bin/site.sh links
./sites/bin/site.sh shell
./sites/bin/site.sh clean
# or
bun run dev
```

The script runs the official Bun image (`oven/bun:1.4.4`) with `docs/`, `sites/`, `scripts/`, and the read-only test helpers mounted, installs dependencies into the `testd-site-node-modules` volume, and runs the stock `vitepress` and `vite` CLI entrypoints with Bun. It prefers Podman and falls back to Docker; set `TESTD_SITE_RUNTIME` to pin one. The product server is the public origin and proxies `/docs/` to VitePress on loopback.

`--host 0.0.0.0` (or `TESTD_SITE_HOST`) serves the site to the network. `--port <port>` overrides the published port for `dev` or `preview`; `TESTD_SITE_PORT` and `TESTD_SITE_PREVIEW_PORT` set the defaults, and `TESTD_SITE_IMAGE` or `TESTD_SITE_VOLUME` overrides the image or dependency volume. Remove the volumes with `./sites/bin/site.sh clean`.

The build and check scripts remain the local path used by CI; they need `bun install` first and run outside the container:

```sh
bun install
bun run build     # dist/, with the docs under dist/docs
bun run preview   # serves the built tree at http://localhost:4173/
bun run check
bun run check:links
```

`bun run build` writes the product site first and the docs second; deploy `dist/` as one static tree. `bun run preview -- --host 0.0.0.0` exposes the built tree on every interface.

`master` publishes `dist/` to <https://testd.dev> through the Cloudflare Pages Git integration. The project settings and the Workers alternative are in [sites/README.md](https://github.com/stelmost/testd/blob/master/sites/README.md#publish).
