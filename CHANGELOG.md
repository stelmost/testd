# Changelog

Format: Keep a Changelog. Versions follow SemVer. The CLI, daemon, and tray ship together.

## [Unreleased]

## [0.1.0-alpha.5] - 2026-11-26

### Fixed

- The website installer, download links, and update feed now use the Cargo package version; the source installer resolves the latest stable release unless an explicit version is supplied
- Tag releases now publish automatically after all release assets upload successfully; failed uploads leave new releases as drafts
- Windows CI classifies POSIX-only SSH and release-publication fixtures as inapplicable, and Fedora CI installs the SHA-256 fallback tool required by the SSH fixtures

## [0.1.0-alpha.3] - 2026-11-25

### Fixed

- macOS certificate repair now requests administrator authorization for an existing untrusted root owned by this installation, restores missing trust, and refreshes its ownership receipt; unowned certificates and externally changed trust settings remain untouched

## [0.1.0-alpha.2] - 2026-11-24

### Added

- SSH apps can install a missing or mismatched remote testd helper after dashboard approval, verify the matching release checksum, and preserve the previous binary in a persistent backup; automatic installation supports Linux x86_64 and macOS x86_64/arm64
- Optional update checks at host startup and daily, with available releases in the tray and dashboard, a persistent off switch, and an explicit download action

### Fixed

- Quick-launch stdout and stderr now update live in the dashboard, label each stream, and preserve the output panel's expansion, scroll position, and focus
- Firefox HTTPS connectivity is checked separately from system certificate trust, with certificate import guidance when the connection cannot be confirmed
- Supervised launches no longer open console windows on Windows
- The CLI installer uses a pinned release and verifies its checksums without depending on unauthenticated GitHub API requests

## [0.1.0-alpha.1] - 2026-11-24

First public alpha. The CLI, daemon, and tray ship as one version; desktop bundles are unsigned for distribution and installed by hand.

### Added

- Host daemon with a built-in HTTPS edge, on-demand leaf certificates, and an HTTP/1.1 proxy (including WebSockets and IPv6 targets); no external reverse proxy in the runtime path
- `.test` names served by a built-in DNS server with lazy OS resolver setup on macOS, Linux, and Windows; conflicting unowned macOS resolver files and Windows NRPT rules are left untouched, and the TLD is fixed
- Native TLS 1.3 registration with explicit host pairing, independently derived verification codes, and saved per-endpoint trust
- Mutually authenticated host-to-leaf application forwarding with connection-scoped in-memory leaf keys; dev servers can remain local HTTP, while host-to-leaf traffic uses mutual TLS (mTLS)
- Access by request on first run: a leaf without a credential asks the host, the operator approves or denies with `testd requests`, `testd approve`, `testd deny`, or the dashboard, and the tray raises a notification while a request waits
- Per-name grants: a token covers exactly the names the operator approved, publishing another name raises its own request, and revoking a grant closes active leaf streams
- `testd run` with `name:port` entries, token auth, reconnect, failover, and wrapped process-tree cleanup
- Named profiles in `~/.config/testd/leaf.toml` and one-off `--host`/`--hosts` selection
- Loopback dashboard with live names, shares, quick launches, access requests, issued tokens, and certificate controls; `testd dashboard` prints its authenticated URL, tray and CLI links include the configured HTTPS port, and bare loopback connections reveal no live state
- Supervised quick launches: probe before registering, detect the child's listening port, rebind live, save the project directory, show captured output and exit failures, and stop the process group cleanly
- Native Rust tray app (Tao and tray-icon/muda) with a browser-based setup wizard, health notices, quick launches, certificate management, and launch at login; setup and the dashboard open in the default browser via owner-private handoff files, and its menu lists live names (open/stop) and saved quick launches (start/stop)
- `testd share` through `cloudflared`, `ngrok`, or `tailscale serve`. All shares use the daemon's plain HTTP routing listener and a claim-bound alias, and Tailscale refuses to overwrite or adopt existing or ambiguous `:443` handlers. A fresh daemon does not restore aliases; after a crash, an empty share list does not prove provider cleanup.
- `testd init`, `doctor [--json] [--fix]`, `url`, `uninstall`, and `export-caddyfile`
- `doctor` checks the resolver, root trust, and edge, repairs them with `--fix` (macOS DNS cache and mDNSResponder, Windows NRPT rules and the machine root store), preserves other installations' macOS roots, and reports when an external front owns TLS
- Host opt-ins: `[host] allow_remote_targets` for explicit remote targets, `[host] trust_forwarded_for` for a trusted front proxy (preserving the browser's HTTPS scheme), and `[host] edge_enabled = false` to drop only the built-in TLS listener while plain HTTP routing stays up
- Packaging and installers: Homebrew formula and architecture-aware cask, launchd and systemd units, cargo-packager desktop bundles, per-user Windows NSIS installer, and static-CRT Windows CLI ZIPs; building from source needs Rust 1.90
- Distribution-neutral Linux x86_64 CLI archives for GNU libc (`x86_64-unknown-linux-gnu`) and static musl (`x86_64-unknown-linux-musl`); the CLI installer detects libc and supports a `TESTD_LIBC` override
- Tag-triggered release automation: CLI archives, tray bundles, workspace SBOMs, generated Homebrew definitions, checksums, and a draft GitHub release
- Cross-platform release validation for the host, browser, desktop, and artifact suites, including fixture self-tests for the privileged resolver and trust integration

### Security

- One operator secret admits registrations; issued leaf tokens are stored with owner-only permissions, shown only by ID, and revocable, and revoking one closes the connections it admitted and releases its names
- Owner-authenticated dashboard and setup requests, including event streams, without cross-port cookie credentials: dashboard administration requires an owner-scoped per-run credential separate from registration and control tokens, setup uses its own ephemeral credential, and daemon restart invalidates dashboard sessions
- Private state is written atomically: Unix secret files use mode `0600` and `O_NOFOLLOW`, and Windows credentials, control data, and PKI keys use protected owner-scoped ACLs from creation while refusing unsafe existing files and unverifiable parents
- Windows privilege elevation uses system PowerShell and verifies a single in-memory helper-source snapshot before execution; Windows CLI uninstall verifies recorded ownership before removing resolver, trust, or local state
- The HTTPS edge mints certificates only for registered names and the `testd` dashboard name, not arbitrary SNI values
- Edge connections are bounded per source (including WebSocket upgrades), the TLS handshake is timed out, and the certificate cache is bounded; intake frames, HTTP headers, and setup reads and writes have limits
- Remote leaves can only target their own address unless `[host] allow_remote_targets` is set, and client-supplied `X-Forwarded-For` is ignored unless a front proxy is trusted
- Desktop bundles are unsigned for distribution; any optional macOS ad-hoc resource seal checks bundle integrity only, not trusted signing, notarization, or Gatekeeper approval. Verify `SHA256SUMS` before installing.
