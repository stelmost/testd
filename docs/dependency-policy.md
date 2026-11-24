---
title: Dependency policy
description: 'Desktop integration library choices, advisory exceptions, and MSRV constraints.'
---

# Dependency policy

`cargo deny check` remains the dependency policy gate. Vulnerability advisories are not exempted.

## Desktop integration libraries

The native tray uses narrowly scoped libraries rather than restoring an application framework. Versions whose platform behavior is relied on are pinned; review that behavior, MSRV, and the target-filtered dependency graph before upgrading them.

| Surface | Choice | Reason |
|---|---|---|
| Windows browser launch | [`webbrowser` 1.2.7](https://crates.io/crates/webbrowser/1.2.7), Rust 1.85 | Resolves the HTTP browser command and preserves its arguments without falling back to the HTML file association |
| Windows/macOS error dialogs | [`native-dialog` 0.9.7](https://crates.io/crates/native-dialog/0.9.7), Rust 1.85 | Delegates MessageBox/NSAlert handling to a maintained fallible API; testd retains recovery text and private diagnostics |
| Linux browser launch and errors | Existing Gio/GTK | Selects the HTTP handler with a per-launch environment and keeps dialogs available without Zenity/KDialog/YAD |
| macOS browser launch | Existing NSWorkspace lookup and `open` | Selects the HTTP handler explicitly and reports lookup failure instead of silently choosing another browser |
| Login registration | `auto-launch` 0.5 plus macOS `plist` serialization | Keeps per-user identities, LaunchAgent behavior, existing opt-out state, and portable AppImage paths |
| Installed URL associations | `cargo-packager` metadata | Keeps declarations owned by the package; portable runtime registration still validates existing state |
| Early coordination | Existing Tokio/fs2 plus testd's authenticated protocol | Preserves configuration-directory identity, bounded activation, allowed actions, and shutdown policy |

For the currently locked release-target graph, Windows adds `webbrowser`, `native-dialog`, `formatx`, and `wfd`; macOS adds `native-dialog` and `formatx`; Linux adds no normal/build package. The all-target lockfile also includes unused backends, so its growth is not a count of shipped runtime dependencies.

Only a validated, credential-free file URI reaches browser-launch libraries. The authenticated loopback destination stays inside the owner-private handoff. A failed launch is reported for tray retry; no fallback passes the authenticated URL to a process, exposes a public credential endpoint, or relaxes file permissions.

`webbrowser` is Windows-only here. Its Linux fallbacks can select an HTML editor, and its macOS default lookup uses HTTPS with a Safari fallback. Those do not preserve testd's strict HTTP-handler policy. Its `hardened` feature rejects file URIs and is incompatible with this handoff. The pinned Windows implementation has a 512-UTF-16-unit association-command buffer; unusually long registered browser commands remain a native compatibility test case, not a silently supported claim.

`native-dialog` is Windows/macOS-only here because its Linux backend requires an external dialog program. `rfd` 0.18 requires Rust 1.92, above this workspace's 1.90 MSRV, and older GTK backends introduce their own GTK thread. Testd keeps GTK on the native event-loop thread and retains early macOS application initialization for errors before Tao starts.

No new IPC or URI-registration library is selected merely to shorten wrappers. `interprocess` is a maintained transport option but does not replace testd's authorization, startup queue, or resource ownership. The inspected `sysuri` release overwrites/removes associations without testd's ownership checks. These policies, private-file validation, and setup/daemon lifecycles remain application code.

Native browser selection, sandboxed file access, early dialogs, login, and package upgrade/uninstall behavior still require the [desktop and browser validation](https://github.com/stelmost/testd/blob/master/DEVELOPING.md#release-validation) gates. Library MSRVs and successful type checks are not native acceptance evidence.

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

Linux and Windows retain `auto-launch` 0.5 for querying and removing their existing per-user registration. Linux writes the same desktop-entry format with owner-private, no-replace creation so a permissive umask cannot create an entry the product later rejects. On macOS, the library still queries/disables the existing LaunchAgent, but testd creates it with the already selected `plist` serializer. The executable remains a raw filesystem path; XML escaping belongs only in serialization. The published `auto-launch` 0.6 still interpolates XML, so upgrading it would not fix paths containing `&`, `<`, or `>`.

The LaunchAgent keeps `dev.testd.host`, `ProgramArguments`, and `RunAtLoad`, along with validation of compatible/changed entries. Startup only queries login state; explicit opt-in creates it. Creation uses an owner-private temporary file and atomic no-replace persistence; it does not migrate to SMAppService or load a different login service. File-format tests run without AppKit, but do not replace an actual macOS logout/login observation.

## Notification MSRV

The daemon requires `notify-rust` 4.18 for notification action callbacks on macOS and Windows. That release requires Rust 1.89, within the workspace MSRV of Rust 1.90. Release-validation and CI checks use Rust 1.90.
