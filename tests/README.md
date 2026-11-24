# Testing

`tests/run.py` is the single, OS-detecting test entry point. It works on Linux, macOS, and native Windows; use the Linux environment in WSL. Run it from a normal user terminal. Routine checks need no daemon, account, system integration, or disposable VM.

## Support matrix

The required first-release targets are exactly:

| Required OS | Architecture | Automated coverage |
|---|---|---|
| Windows 10 22H2 | amd64 | Hosted Server builds/fixtures do not validate Windows 10 |
| Linux | amd64 | Required Debian 13 userspace CI; native VM signoff is separate |
| macOS 15 | arm64 | Required `macos-15` CI; native VM signoff is separate |
| macOS 26 | arm64 | Required `macos-26` CI; native VM signoff is separate |

Linux uses Debian 13 as its native validation baseline. GNU libc and static musl CLI archives are required; desktop packages use GNU libc. Other Linux distributions and Intel macOS are best effort. Physical-hardware-specific checks are deferred.

Windows 10 22H2 VM signoff remains required. Debian containers share the Ubuntu runner's kernel and do not establish native kernel, resolver/trust, systemd-session, or desktop behavior. Required artifact builds are Windows x64, Linux x64, and macOS arm64 regardless of builder OS; Intel macOS artifacts are optional. Native signoff on both macOS 15 and 26 remains required, using one ARM artifact built on macOS 15 with the same source and artifact hashes on each OS. Missing native macOS 15 access is an access gate, not a hardware-specific deferral. Installation and signing approval remain separate from automated checks.

The release is an unsigned alpha. Optional free macOS ad-hoc resource sealing is integrity evidence, not Developer ID signing, notarization, or Gatekeeper approval. Windows remains unsigned; Linux uses checksums and SBOMs without new signing infrastructure. Record observed security prompts and installation results truthfully; no trusted-signing pass is implied.

## Prepare once

Install Python 3.12 or newer, Git, rustup, Bun, and the native Rust compiler prerequisites. Ensure the rustup shims and Bun are on `PATH`; on Unix, `source "$HOME/.cargo/env"` loads the usual Rust installation. Windows Rust builds require Visual Studio C++ tools and the Windows SDK; macOS requires Xcode command-line tools.

```sh
rustup toolchain install stable 1.90.0 --profile minimal
rustup component add --toolchain stable rustfmt clippy
cargo +stable install --locked cargo-deny
bun install --frozen-lockfile
python3 tests/run.py prepare
```

Use `python` instead of `python3` on native Windows. Python uses only its standard library: no virtual environment or pip packages are required. `prepare` lists missing tools, toolchains, and website dependencies together; it installs nothing. Cargo-nextest is not required. PowerShell behavior fixtures are required on Windows; when PowerShell is unavailable on Unix, Windows-only checks are explicitly inapplicable, not passes.

Windows fixtures need both file and directory symlink creation in the account's temporary directory. `prepare` probes this without changing system settings. For `WinError 1314`, enable Windows Developer Mode where supported, or grant a dedicated test account `SeCreateSymbolicLinkPrivilege` and start a new logon session. Use a symlink-capable filesystem. Keep routine checks unelevated; do not bypass this prerequisite by relabelling skipped safety checks as passes.

GUI compilation is opt-in locally. Install the [Linux GTK/AppIndicator development packages](../.github/actions/linux-dependencies/action.yml), macOS Xcode tooling, or Windows Visual Studio C++ tools and SDK before using `--gui`. No embedded WebKit or WebView2 runtime is required. `prepare --gui` also checks Linux `pkg-config` libraries. Tool detection does not establish that a native compiler, GUI, default-browser handoff, or installation works.

## Run

```sh
python3 tests/run.py --plan
python3 tests/run.py
python3 tests/run.py --gui
```

The default `check` profile runs formatting, Clippy, Rust 1.90 type-checking, stable Rust tests and doctests, Python harness/release fixtures, both Bun browser-server and dashboard fixtures, the full dependency audit, version verification, isolated website checks, a native candidate CLI build, and unprivileged CLI/daemon host checks. Rust checks cover all workspace features; GUI is excluded unless requested. Ignored Rust tests run, and independent stages continue after a failure.

| Profile | Scope |
|---|---|
| `check` or no profile | Routine non-GUI checks above |
| `check --gui` | The same checks including native GUI compilation and tests, not actual tray observations |
| `ci` | Includes GUI; excludes the website build because CI's separate `site` job runs it once |
| `site` | Website type-check, isolated build, and link/asset/anchor validation |
| `contracts` | Rust tests, including ignored cases, on Rust 1.90; `--gui` is optional |
| `audit` | Full-workspace cargo-deny policy |
| `all` | `check`, plus only explicitly selected system, artifact, desktop, browser, and manual stages |
| `host` | Standalone candidate CLI/daemon checks; tray/login evidence can remain incomplete |
| `artifacts`, `browser`, `desktop`, `service` | Protected candidate/system or interactive checks below |
| `prepare` | Prerequisite detection without execution or product-state changes |

`--plan` previews coordinated profiles without commands, report creation, or tool installation. An ordinary `all` run does not mark unselected release checks as passes or create blockers for missing assets nobody requested. Its report states the selected scope.

`--bin /path/to/testd` selects an identified CLI instead of building one. Automatic builds use stable Rust's explicit native target, preserve any existing candidate in the report, and reject `CARGO_BUILD_TARGET`; use `--bin` for separately verified cross-target candidates. In a Git worktree, set `CARGO_TARGET_DIR` to the main checkout's `target/` to share the build cache. A build fingerprint or CLI hash is not release-artifact provenance.

Host fixtures use isolated homes and `.testdcheck`, bind fixed loopback ports, and refuse busy ports. Do not initialize testd first, compete with a real daemon, or stop an unrelated service to make a check pass. Windows also needs a free UDP port 53.

## Reports

Every execution creates a new persisted report directory. The default is `~/.local/share/testd/release-checks/<UTC timestamp>-<id>` or `%LOCALAPPDATA%/testd/release-checks/<UTC timestamp>-<id>`. The runner prints the path. Set `--report /path/to/new-report` to choose it; existing paths are refused, and coordinated or sensitive runs reject temporary storage.

- `report.json`: Source/OS/Python metadata, selected scope, stage outcomes, and nested command/check evidence for coordinated profiles; standalone suites retain their checks at the top level
- `run.log`: Live UTF-8 runner progress, stage boundaries, and final summary
- `<stage>/<number>-<command>.log`: Full combined raw stdout/stderr; command arguments, exit codes, durations, and log paths are indexed in `report.json`
- Fixture files: Retained host homes, PKI, browser profiles/evidence, candidate backups, and scoped system snapshots where applicable
- `ci-summary.json`: CI-only allowlisted source/platform/scope metadata and stage statuses/durations, suitable for reviewed CI publication

The console is quiet by default and shows a bounded output tail on command failure. `--verbose` mirrors complete stdout/stderr into the console and `run.log`; full command logs are retained in either mode. CI enables it so command output survives in job logs. There are no duplicate boundary logs or per-stage report JSON files. Native Cargo supplies test/doctest output; no custom rustdoc or target-runner interception is installed.

`--include-git-diff` retains separate binary patches for tracked staged and unstaged changes, including mode bits, and checks that tracked changes remained stable during capture. It does not collect untracked file contents. Reports, patches, profiles, logs, and fixture keys can contain secrets: keep them private until reviewed. Typed input, sudo prompts, and GUI actions are not recorded automatically.

| Exit | Meaning |
|---|---|
| `0` | Applicable selected checks completed without failures |
| `1` | At least one failure |
| `2` | Incomplete selected coverage without failures |

Ignored, skipped, inapplicable, blocked, and unrun checks are never passes. A completed unprivileged profile is not native UI, browser trust, installer, provider, or release approval. Review individual outcomes and candidate provenance, not only the exit code.

Interpret earlier reports by their recorded source revision and selected scope. A newly required OS does not retroactively turn older best-effort or syntax-only Mac 26 observations into native release approval; retain those results unchanged and require matching candidate provenance for current signoff.

## System and interactive checks

System, artifact, browser, and desktop checks require a clean disposable machine, an interactive terminal, a new persisted report, and a persisted full-system snapshot. Scoped runner backups do not cover the full OS, user profile, browser stores, package installation, login items, or shared CA-store effects. Unix system checks need `sudo -v` in the same terminal. macOS resolver/trust, browser-system, and desktop checks must start in the logged-in desktop's terminal (`launchctl managername` reports `Aqua`); SSH cannot present the authorization required to remove system trust. Approve the native trust dialogs during the run. The standalone `service` suite uses a private home, high ports, and a uniquely named owned systemd or launchd job; it requires the real user session but no resolver/trust changes or elevated privileges. Do not use elevated privileges for routine `check` or `ci`.

```sh
python3 tests/run.py host --bin /path/to/candidate --system --report /path/to/new-system-report
python3 tests/run.py desktop --app /path/to/candidate-app --bin /path/to/candidate --report /path/to/new-ui-report
python3 tests/run.py artifacts --artifacts /path/to/host-assets --require-checksums --report /path/to/new-archive-report
```

For aggregation, select the sensitive scope explicitly:

```sh
python3 tests/run.py all --gui --system --interactive --app /path/to/candidate-app \
  --artifacts /path/to/host-assets --no-previous-release --report /path/to/new-report
```

`all --system` confirms the disposable host and full-system backup once. Each stage still checks its prerequisites, ownership, and authorization. `--interactive` requires `--system` and adds real desktop/browser and manual evidence prompts. `--app` and `--browser-engine` require that interactive scope; `--artifacts` requires system authorization because it executes extracted candidates. Use `--previous-bin` rather than `--no-previous-release` when a real previous CLI exists; candidate assets must include `SHA256SUMS`.

macOS system/desktop checks use installation-owned resolver files and certificate receipts. Cleanup refuses changed or unowned integration, including changed certificate trust entries. The runner snapshots resolver receipts and verifies that no certificate receipts remain after product cleanup; it creates and removes a missing `/etc/resolver` only with recorded directory ownership. macOS service supervision uses the packaged launchd template in a unique job outside login-agent directories. Windows product-managed system/desktop checks remain blocked until native ownership and cleanup are proven. Windows has isolated PowerShell fixtures and native directory/ACL regressions; they do not establish NRPT, root-store, UAC, or installer behavior. Windows has no shipped service feature. Actual network exposure and real providers still require separate explicit approval.

A failed or incomplete sensitive stage blocks later sensitive operations unless its cleanup was verified. The system self-test cannot verify shared trust-store output/hooks and records incomplete coverage. Artifact smoke checks retain their isolated home and report real uninstall/purge as blocked. Use separate restored machines for other checks and actual installation/uninstall observations; aggregation is not one-click release approval.

For interrupted or deferred cleanup, stop every candidate app, daemon, and provider first. Pass the recorded stage directory or retained `self-test-state` directory to `python3 tests/run.py --restore-report /path/to/recorded-state`. Recovery uses the retained snapshot manifests, not stage report JSON. It refuses changed or unowned state and cannot replace full-system recovery or shared trust-store comparison.

## Real-browser checks

Install Playwright where Bun can resolve it and install the selected browser on the disposable machine. Linux Chromium/Firefox also need NSS `certutil` for an isolated profile. The runner installs neither tools nor browsers. Block external egress at the VM boundary; the fixture restricts requests and DNS results to loopback but does not configure a firewall.

```sh
python3 tests/run.py browser --bin /path/to/candidate --system --browser-engine chromium \
  --edge-port 8443 --report /path/to/new-browser-report
```

Linux and macOS `--system` create a private fixture home, start their own candidate daemon with real providers disabled, apply installation-owned resolver/CA integration through `doctor --fix`, drive a fresh browser profile, stop owned processes, and compare scoped state. The high edge port avoids privileged-port requirements. Conflicting pre-existing integration is refused. Unverified shutdown defers system cleanup; shared trust effects remain incomplete until independently compared or the VM is restored.

Without `--system`, configure the same candidate's `.test` OS DNS and trust separately. The runner verifies the default `testd/pki/root.crt`, OS resolver/trust status, and free fixture ports; it never adopts or restarts an existing daemon. This preconfigured mode is also available on Windows but still requires separately proven, recoverable setup.

The browser report records engine/user agent, candidate/root hashes, certificate details, loopback responses, WebSocket/SSE/upload/cookie checks, and matching leaf fingerprints before and after an owned daemon restart. Independent TLS probes use OS DNS and the default platform trust store with hostname verification. HTTPS errors are not bypassed; DNS is not overridden. Profiles use direct connections without inherited proxies/trust overrides. Linux imports only the public fixture root into its isolated profile after the OS trust check. An engine run does not validate installed branded browsers, their certificate viewers, tray UI, or another platform.

## Coverage and remaining evidence

| Contract | Automated coverage | Still needs native/manual evidence |
|---|---|---|
| Configuration, private state, local defaults, DNS-only setup | `tests/cli.rs`, core config tests | Installed packages, permissions, actual GUI setup |
| Authentication, name ownership, disconnect, revocation, rollback | Engine, protocol, registry, and `product_flow.rs` tests | Platform workflows and local-user authority boundaries |
| DNS answers and malformed/unknown names | DNS unit tests and real loopback UDP fixtures | OS resolver routing |
| TLS validation, pairing, proxy routing, forwarding, and failures | Edge, PKI, `native_tls.rs`, and product-flow tests | Installed OS/browser trust |
| Preserve unowned integration and remove only proven-owned state | Setup ownership/resolver/trust fixtures | Shared trust hooks, concurrent privileged changes, native macOS cleanup and Windows ownership validation |
| Dashboard, quick launches, lifecycle, and local request protections | Engine/launch tests, `tests/host/`, Bun dashboard fixtures | Actual tray/browser, local-user dashboard authorization |
| Shares, provider exits, and revocation | Fake-provider lifecycle and reconciliation tests | Real providers, network-pause controls, crash recovery |
| Packaging, rollback, and recovery reports | Python release/harness fixtures and protected artifact checks | Provenance, signing, installed upgrade/uninstall on each platform |
| Browser fixture protocols and reports | Bun fixture tests and opt-in Playwright | Each claimed OS/browser combination |

On macOS, native port-443 validation requires the logged-in user's launchd GUI session. Verify the CLI and actual app with their default loopback HTTPS address, hostname-verified TLS, refusal from a separate LAN machine, no remaining `dev.testd.socket.*` jobs, and listener release after exit. The ordinary Rust suite covers descriptor transfer and exact job configuration without requiring launchd access.

`native_tls.rs` runs the real CLI against an isolated component host: matching and mismatched pairing codes, no credential issuance during pairing, identity replacement, plaintext downgrade attempts, mutual-TLS application requests and upgrades, and rollback retaining TLS identity. Its resolver cannot touch the system, and candidate homes are private fixtures. It does not validate firewall configuration or actual network continuity.

`product_flow.rs` composes registration, DNS, and TLS on ephemeral loopback sockets. Only its test client trusts the root, and its resolver wrapper forbids system integration. It is component evidence, not a full installed daemon, browser, or GUI. Dashboard and setup authorization have component coverage; native multi-user permission checks and Windows private-file ACL validation remain separate evidence. Explicit network-exposure/pause controls remain unfinished; local defaults and TLS do not establish network continuity or exposure policy.

CI requires the unprivileged profile on macOS 15 arm64, macOS 26 arm64, and Debian 13 amd64 userspace. Ubuntu 24.04, Intel macOS 15, Windows Server 2025, and Fedora 44 userspace are best effort: their failed or incomplete checks remain visible but do not block required artifact builds. Both containers share the Ubuntu kernel and have no actual desktop/systemd session; hosted Windows Server is not Windows 10 22H2 validation. A separate website job avoids repeated cross-platform builds. Python 3.12 fake-state jobs retain minimum-runtime coverage; both macOS 15 and 26 fixture jobs are required. Manual/tag runs require Windows x64, Linux x64, and one macOS arm64 unsigned bundle built on macOS 15, plus SBOMs after required test gates. The same ARM artifact needs separate native signoff on 15 and 26; Intel bundles are optional. Only version-tag pushes can write GitHub Releases; manual dispatch cannot, even on a tag ref. Artifact creation is not installation or signing approval.

## Files and manual contracts

- `run.py`: Portable entry point, candidate/fixture execution, logs, and recovery
- `release/`: Shared coordination/system/service/browser helpers, Python regression tests, and Bun fixtures
- `host/`: Native CLI/daemon fixtures and [desktop checklist](host/gui-checklist.md)
- `check-links.ts`: Built website link/asset/anchor validation, also used by `bun run check:links`
- `ci-changes.sh`: CI job-input classification
- `tray-debug.ps1`: Read-only Windows tray/build/install diagnostics
- `manual/`: [Browsers](manual/browser.md), [system integration](manual/system-integration.md), [installation](manual/installation.md), and [network/sharing](manual/network-and-sharing.md) contracts

Runtime elevation, icon/build, packaging, release, and SBOM helpers remain in `scripts/`: they are product/build operations, not test entry points.
