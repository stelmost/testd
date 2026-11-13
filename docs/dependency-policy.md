---
title: Dependency policy
description: 'Advisory exceptions and MSRV constraints accepted by the dependency gate.'
---

# Dependency advisory exceptions

`cargo deny check` remains the dependency policy gate. Vulnerability advisories are not exempted.

## GTK3 procedural macros

RUSTSEC-2024-0370 reports that `proc-macro-error` 1.0.4 is unmaintained; it does not report a vulnerability or a patched release. The locked crate is used at build time through `glib-macros` and `gtk3-macros` in the GUI's GTK3 dependency graph, not in the shipped runtime. The single advisory is temporarily exempted in `deny.toml`.

An unmaintained crate can still develop security problems. Reassess the exception when the native GTK dependencies change, remove it when those macros no longer require the crate, and continue enforcing any vulnerability advisory.

## D-Bus crate merger

The following informational `unmaintained` advisories are temporarily accepted:

| Advisory | Crate | Upstream replacement |
|---|---|---|
| RUSTSEC-2026-0370 | `zvariant` | `zbus::wire` in zbus 6 |
| RUSTSEC-2026-0371 | `zbus_names` | `zbus::names` in zbus 6 |
| RUSTSEC-2026-0372 | `zvariant_derive` | Derive macros in zbus 6 |

These advisories describe the standalone crates being merged into zbus, not reported exploitable vulnerabilities. Their records contain no patched standalone version.

`notify-rust` requires zbus 5 on Linux. Current upstream releases retain that requirement. Upgrading a transitive version cannot replace zbus 5 with the incompatible zbus 6 API. Notification actions remain enabled; tray single-instance activation no longer uses a Tauri plugin.

These three advisory IDs are listed explicitly in `deny.toml`; other advisory checks remain enforced except for the GTK3 maintenance notice above. The dependency entry in `crates/testd-daemon/Cargo.toml` carries the same rationale.

Remove these exceptions once notifications use zbus 6 and the standalone crates no longer appear in the resolved graph. Reassess the exceptions on every notification dependency update. Upstream migration: [zbus issue 2043](https://github.com/z-galaxy/zbus/issues/2043).

## Launch-at-login serialization

Linux and Windows retain `auto-launch` 0.5 and their existing per-user registration. On macOS, the library still queries/disables the existing LaunchAgent, but testd creates it with the already selected `plist` serializer. The executable remains a raw filesystem path; XML escaping belongs only in serialization. The published `auto-launch` 0.6 still interpolates XML, so upgrading it would not fix paths containing `&`, `<`, or `>`.

The LaunchAgent keeps `dev.testd.host`, `ProgramArguments`, and `RunAtLoad`, along with the existing opt-out marker and validation of compatible/changed entries. Creation uses an owner-private temporary file and atomic no-replace persistence; it does not migrate to SMAppService or load a different login service. File-format tests run without AppKit, but do not replace an actual macOS logout/login observation.

## Notification MSRV

The daemon requires `notify-rust` 4.18 for notification action callbacks on macOS and Windows. That release requires Rust 1.89, within the workspace MSRV of Rust 1.90. Release-validation and CI checks use Rust 1.90.
