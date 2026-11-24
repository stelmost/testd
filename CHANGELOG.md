# Changelog

Format: Keep a Changelog. Versions follow SemVer. The CLI, daemon, and tray ship together.

## [Unreleased]

## [0.1.0-alpha.1] - 2026-11-24

First public alpha. The CLI, daemon, and tray ship as one version; desktop bundles are unsigned for distribution and installed by hand.

### Added

- Host daemon with a built-in HTTPS edge, on-demand leaf certificates, and an HTTP/1.1 proxy; no Caddy in the runtime path
- `.test` names served by a built-in DNS server with lazy OS resolver setup on macOS, Linux, and Windows; the TLD is fixed
- Access by request on first run: a leaf without a credential asks the host, the operator approves or denies with `testd requests`, `testd approve`, `testd deny`, or the dashboard, and the tray raises a notification while a request waits
- Per-name grants: a token covers exactly the names the operator approved, and publishing another name raises its own request
- `testd run` with `name:port` entries, token auth, reconnect, and process wrapping
- Named profiles in `~/.config/testd/leaf.toml` and one-off `--host`/`--hosts` selection
- Loopback dashboard with live names, shares, quick launches, access requests, issued tokens, and certificate controls; `testd dashboard` prints its authenticated URL, and tray and CLI links carry the owner-scoped per-run credential
- Supervised quick launches: probe before registering, detect the child's listening port, rebind live, and stop the process group
- Tray app with a setup wizard, health notices, quick launches, certificate management, and launch at login; its menu lists live names with open and stop beneath each, and saved quick launches with start or stop
- `testd share` through `cloudflared`, `ngrok`, or `tailscale serve`. All shares use the daemon's plain HTTP routing listener and a claim-bound alias. A fresh daemon does not restore aliases; after a crash, an empty share list does not prove provider cleanup.
- `testd init`, `doctor [--json] [--fix]`, `url`, `uninstall`, and `export-caddyfile`
- `doctor` checks the resolver, the root trust, and the edge, repairs them with `--fix` (macOS DNS cache and mDNSResponder, Windows NRPT rules and the machine root store), and reports when an external front owns TLS
- Host opt-ins: `[host] allow_remote_targets` for explicit remote targets, `[host] trust_forwarded_for` for a trusted front proxy, and `[host] edge_enabled = false` to drop only the built-in TLS listener while plain HTTP routing stays up
- Homebrew formula and architecture-aware cask, launchd and systemd units, and an NSIS installer; building from source needs Rust 1.90
- Tag-triggered release automation: CLI archives, tray bundles, workspace SBOMs, generated Homebrew definitions, checksums, and a draft GitHub release
- Cross-platform release validation for the host, browser, desktop, and artifact suites, including fixture self-tests for the privileged resolver and trust integration
- Native TLS 1.3 registration with explicit host pairing, independently derived verification codes, and saved per-endpoint trust
- Owner-authenticated dashboard and setup requests, including event streams, without cross-port cookie credentials
- Mutually authenticated host-to-leaf application forwarding with connection-scoped in-memory leaf keys; dev servers can remain local HTTP, while host-to-leaf traffic uses mutual TLS (mTLS)

### Changed

- Dashboard administration requires an owner-scoped per-run credential, separate from registration and control tokens; tray and CLI links bootstrap origin-scoped sessions, and bare loopback connections reveal no live state
- Setup uses a separate ephemeral credential before reading secrets or invoking privileged actions; daemon restart invalidates dashboard sessions
- The native Rust tray uses Tao and tray-icon/muda; setup and the dashboard open in the default browser through owner-private handoff files, without administrative credentials in opener arguments
- First-run setup navigates its own tab into the dashboard; closing a tab leaves the tray running with Continue setup and Exit controls
- Tray packaging uses pinned cargo-packager tooling; package identity, per-user Windows installation, and public artifact names are retained

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
- Windows CLI uninstall checks the entire installation's recorded resolver and trust state before removing it; purge retains local data if ownership or cleanup cannot be verified
- Windows release executables link the MSVC runtime statically, so the CLI ZIP needs no separate Visual C++ redistributable
- Windows elevation uses the system PowerShell and verifies a single in-memory helper-source snapshot before execution without exceeding the native command-line limit; helper and certificate staging files close before the elevated process reads them
- macOS certificate actions preserve other installations' roots, and Windows resolver setup refuses conflicting unowned rules
- Failed startup releases background listeners; setup rejects concurrent finishes, retains earlier recovery backups, and accepts another attempt after a start failure
- Wrapped CLI commands stop their process trees; quick launches keep Stop responsive during rebinding and stalled host requests
- TLS claims and provider aliases retain their upstream identity through rollback; failed rebinds preserve the previous forwarder, and grant withdrawal closes active leaf streams
- WebSocket upgrades retain their connection-limit permits, IPv6 targets use brackets, and silent authentication peers no longer block failover
- Dashboard and tray links include the configured HTTPS port, and quick-launch command parsing preserves Windows paths

### Removed

- The Tauri framework, webview runtime, desktop plugins, capabilities, and cargo-tauri build path
- The redundant `testd deregister` CLI command; registrations still end with their owning connection, and protocol-level deregistration remains available to that owner
- Unused Caddy-trust and prefix-scope helpers, and the unused second product-site implementation

### Security

- One operator secret admits registrations; issued leaf tokens are stored with owner-only permissions, shown only by ID, and revocable, and revoking one closes the connections it admitted and releases its names
- Unix secret files use mode `0600` and `O_NOFOLLOW`; Windows private state uses protected owner-scoped ACLs and verifies the helper-source snapshot before privileged execution
- The HTTPS edge mints certificates only for registered names and the `testd` dashboard name, not arbitrary SNI values
- Edge connections are bounded per source, the TLS handshake is timed out, and the certificate cache is bounded; intake frames, HTTP headers, and setup reads and writes have limits
- Remote leaves can only target their own address unless `[host] allow_remote_targets` is set, and client-supplied `X-Forwarded-For` is ignored unless a front proxy is trusted
- Dashboard reads, mutations, and event streams require an owner-scoped per-run credential, separate from registration and control tokens; setup uses its own ephemeral credential
- Desktop bundles are unsigned for distribution; any optional macOS ad-hoc resource seal checks bundle integrity only, not trusted signing, notarization, or Gatekeeper approval. Verify `SHA256SUMS` before installing.
