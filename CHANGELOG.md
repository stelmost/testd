# Changelog

Format: Keep a Changelog. Versions follow SemVer. The CLI, daemon, and tray ship together.

## [Unreleased]

### Added

- Native TLS 1.3 registration with explicit host pairing, independently derived verification codes, and saved per-endpoint trust
- Owner-authenticated dashboard and setup requests, including event streams, without cross-port cookie credentials
- Mutually authenticated host-to-leaf application forwarding with connection-scoped in-memory leaf keys; dev servers can remain local HTTP

### Changed

- Dashboard administration requires an owner-scoped per-run credential, separate from registration and control tokens; tray and CLI links bootstrap origin-scoped sessions, and bare loopback connections reveal no live state
- Setup uses a separate ephemeral credential before reading secrets or invoking privileged actions; daemon restart invalidates dashboard sessions
- The tray app opens the dashboard in a window instead of the browser, and lists `testd.test → dashboard` as the last row of the name list
- First-run setup is a local page served by the daemon on an ephemeral loopback port and shown in a dialog; it shares the dashboard stylesheet and talks HTTP instead of Tauri commands

### Fixed

- Quick launches save the project directory and show captured output, failed probes, and unsuccessful command exits
- Process cleanup includes descendants after their parent exits; service termination stops wrapped commands, and daemon shutdown stops owned shares
- Tailscale startup no longer deletes or adopts an unowned route merely because it points at the local edge; existing or ambiguous `:443` handlers block a new share
- Busy registration reads no longer refuse live HTTPS names, and trusted external fronts preserve the browser's HTTPS scheme
- Specific network intake binds keep a loopback dashboard and local launches; certificate status refreshes while the dashboard stays open
- macOS resolver setup leaves conflicting files untouched, tray startup failures show recovery instructions, and setup repair buttons keep their labels
- Name-grant deletion and registration share a lock, preventing a waiting registration from restoring withdrawn access
- Private state is replaced atomically; failed quick-launch edits preserve live definitions, and unreadable launch files stop startup rather than being overwritten
- Windows credentials, control data, and PKI keys use protected owner-scoped ACLs from creation; owner-rights entries are accepted only for verified owners, while unsafe existing files and unverifiable parents are refused before use
- Windows resolver and root removal require unchanged administrator-owned records; CLI uninstall remains blocked pending native validation
- Windows elevation uses the system PowerShell and verifies a single in-memory helper-source snapshot before execution without exceeding the native command-line limit; helper and certificate staging files close before the elevated process reads them
- macOS certificate actions preserve other installations' roots, and Windows resolver setup refuses conflicting unowned rules
- Failed startup releases background listeners, and the setup dialog accepts another attempt after a start failure
- Wrapped CLI commands stop their process trees; quick launches keep Stop responsive during rebinding and stalled host requests
- TLS claims and provider aliases retain their upstream identity through rollback; failed rebinds preserve the previous forwarder, and grant withdrawal closes active leaf streams
- WebSocket upgrades retain their connection-limit permits, IPv6 targets use brackets, and silent authentication peers no longer block failover
- Dashboard and tray links include the configured HTTPS port, and quick-launch command parsing preserves Windows paths

### Removed

- The redundant `testd deregister` CLI command; registrations still end with their owning connection, and protocol-level deregistration remains available to that owner
- Unused Caddy-trust and prefix-scope helpers, and the unused second product-site implementation

## [0.1.0-alpha] - 2026-11-03

First public release. The CLI, daemon, and tray ship as one version; desktop bundles are unsigned and installed by hand.

### Added

- Host daemon with a built-in HTTPS edge, on-demand leaf certificates, and an HTTP/1.1 proxy; no Caddy in the runtime path
- `.test` names served by a built-in DNS server with lazy OS resolver setup on macOS, Linux, and Windows; the TLD is fixed
- Access by request on first run: a leaf without a credential asks the host, the operator approves or denies with `testd requests`, `testd approve`, `testd deny`, or the dashboard, and the tray raises a notification while a request waits
- Per-name grants: a token covers exactly the names the operator approved, and publishing another name raises its own request
- `testd run` with `name:port` entries, token auth, reconnect, and process wrapping
- Named profiles in `~/.config/testd/leaf.toml` and one-off `--host`/`--hosts` selection
- Loopback dashboard with live names, shares, quick launches, access requests, issued tokens, and certificate controls; `testd dashboard` prints its authenticated URL, and tray and CLI links carry the per-run token
- Supervised quick launches: probe before registering, detect the child's listening port, rebind live, and stop the process group
- Tray app with a setup wizard, health notices, quick launches, certificate management, and launch at login; its menu lists live names with open and stop beneath each, and saved quick launches with start or stop
- `testd share` through spawned `cloudflared`, `ngrok`, or `tailscale serve`; the daemon owns the tunnel for the registration's lifetime
- `testd init`, `doctor [--json] [--fix]`, `url`, `uninstall`, and `export-caddyfile`
- `doctor` checks the resolver, the root trust, and the edge, repairs them with `--fix` (macOS DNS cache and mDNSResponder, Windows NRPT rules and the machine root store), and reports when an external front owns TLS
- Host opt-ins: `[host] allow_remote_targets` for explicit remote targets, `[host] trust_forwarded_for` for a trusted front proxy, and `[host] edge_enabled = false` to drop only the built-in TLS listener while plain HTTP routing stays up
- Homebrew formula and architecture-aware cask, launchd and systemd units, and an NSIS installer; building from source needs Rust 1.90
- Tag-triggered release automation: CLI archives, tray bundles, workspace SBOMs, generated Homebrew definitions, checksums, and a draft GitHub release
- Cross-platform release validation for the host, browser, desktop, and artifact suites, including fixture self-tests for the privileged resolver and trust integration

### Security

- One operator secret admits registrations; issued leaf tokens are stored with mode `0600`, shown only by ID, and revocable, and revoking one closes the connections it admitted and releases its names
- Secret files use `0600` from creation and `O_NOFOLLOW`; root CA installation never stages a certificate in a user-writable temp file, and Windows embeds the audited helper instead of writing a mutable script
- The HTTPS edge mints certificates only for registered names and the `testd` dashboard name, not arbitrary SNI values
- Edge connections are bounded per source, the TLS handshake is timed out, and the certificate cache is bounded; intake frames, HTTP headers, and setup reads and writes have limits
- Remote leaves can only target their own address unless `[host] allow_remote_targets` is set, and client-supplied `X-Forwarded-For` is ignored unless a front proxy is trusted
- The dashboard binds to loopback only, and its mutations and `/api/local/*` require the per-run control token
- Remote registration is plain TCP and desktop bundles are unsigned: use a trusted network or a separately secured transport, and verify `SHA256SUMS` before installing
