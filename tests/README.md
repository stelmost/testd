# Testing

`tests/run.py` is the single, OS-detecting test entry point. It works on Linux, macOS, and native Windows; use the Linux environment in WSL. Run it from a normal user terminal. Routine checks need no daemon, account, system integration, or disposable VM.

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

GUI compilation is opt-in locally. Install the [Linux GTK/WebKit development packages](../.github/actions/linux-dependencies/action.yml), macOS Xcode tooling, or Windows WebView2 prerequisites before using `--gui`. `prepare --gui` also checks Linux `pkg-config` libraries. Tool detection does not establish that a native compiler, GUI, or installation works.

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

## System and interactive checks

Use a clean disposable machine, an interactive terminal, a new persisted report, and a persisted full-system snapshot. Scoped runner backups do not cover the full OS, user profile, browser stores, package installation, login items, or shared CA-store effects. Linux system checks need `sudo -v` in the same terminal. Do not use elevated privileges for routine `check` or `ci`.

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

macOS and Windows product-managed system/desktop checks and macOS service supervision remain blocked until ownership and cleanup are proven. Windows has no shipped service feature. Unsupported resolver probes are not part of the runnable suite. Actual network exposure and real providers still require separate explicit approval.

A failed or incomplete sensitive stage blocks later sensitive operations unless its cleanup was verified. The system self-test cannot verify shared trust-store output/hooks and records incomplete coverage. Artifact smoke checks retain their isolated home and report real uninstall/purge as blocked. Use separate restored machines for other checks and actual installation/uninstall observations; aggregation is not one-click release approval.

For interrupted or deferred cleanup, stop every candidate app, daemon, and provider first. Pass the recorded stage directory or retained `self-test-state` directory to `python3 tests/run.py --restore-report /path/to/recorded-state`. Recovery uses the retained snapshot manifests, not stage report JSON. It refuses changed or unowned state and cannot replace full-system recovery or shared trust-store comparison.

## Real-browser checks

Install Playwright where Bun can resolve it and install the selected browser on the disposable machine. Linux Chromium/Firefox also need NSS `certutil` for an isolated profile. The runner installs neither tools nor browsers. Block external egress at the VM boundary; the fixture restricts requests and DNS results to loopback but does not configure a firewall.

```sh
python3 tests/run.py browser --bin /path/to/candidate --system --browser-engine chromium \
  --edge-port 8443 --report /path/to/new-browser-report
```

Linux `--system` creates a private fixture home, starts its own candidate daemon with real providers disabled, applies fixture-owned resolver/CA integration through `doctor --fix`, drives a fresh browser profile, stops owned processes, and compares scoped state. The high edge port avoids privileged-port requirements. Conflicting pre-existing integration is refused. Unverified shutdown defers system cleanup; shared trust effects remain incomplete until independently compared or the VM is restored.

Without `--system`, configure the same candidate's `.test` OS DNS and trust separately. The runner verifies the default `testd/pki/root.crt`, OS resolver/trust status, and free fixture ports; it never adopts or restarts an existing daemon. This preconfigured mode is available on macOS/Windows but still requires separately proven, recoverable setup.

The browser report records engine/user agent, candidate/root hashes, certificate details, loopback responses, WebSocket/SSE/upload/cookie checks, and matching leaf fingerprints before and after an owned daemon restart. Independent TLS probes use OS DNS and the default platform trust store with hostname verification. HTTPS errors are not bypassed; DNS is not overridden. Profiles use direct connections without inherited proxies/trust overrides. Linux imports only the public fixture root into its isolated profile after the OS trust check. An engine run does not validate installed branded browsers, their certificate viewers, tray UI, or another platform.

## Coverage and remaining evidence

| Contract | Automated coverage | Still needs native/manual evidence |
|---|---|---|
| Configuration, private state, local defaults, DNS-only setup | `tests/cli.rs`, core config tests | Installed packages, permissions, actual GUI setup |
| Authentication, name ownership, disconnect, revocation, rollback | Engine, protocol, registry, and `product_flow.rs` tests | Platform workflows and local-user authority boundaries |
| DNS answers and malformed/unknown names | DNS unit tests and real loopback UDP fixtures | OS resolver routing |
| TLS validation, pairing, proxy routing, forwarding, and failures | Edge, PKI, `native_tls.rs`, and product-flow tests | Installed OS/browser trust |
| Preserve unowned integration and remove only proven-owned state | Setup ownership/resolver/trust fixtures | Shared trust hooks, concurrent privileged changes, complete macOS/Windows ownership |
| Dashboard, quick launches, lifecycle, and local request protections | Engine/launch tests, `tests/host/`, Bun dashboard fixtures | Actual tray/browser, local-user dashboard authorization |
| Shares, provider exits, and revocation | Fake-provider lifecycle and reconciliation tests | Real providers, network-pause controls, crash recovery |
| Packaging, rollback, and recovery reports | Python release/harness fixtures and protected artifact checks | Provenance, signing, installed upgrade/uninstall on each platform |
| Browser fixture protocols and reports | Bun fixture tests and opt-in Playwright | Each claimed OS/browser combination |

`native_tls.rs` runs the real CLI against an isolated component host: matching and mismatched pairing codes, no credential issuance during pairing, identity replacement, plaintext downgrade attempts, mutual-TLS application requests and upgrades, and rollback retaining TLS identity. Its resolver cannot touch the system, and candidate homes are private fixtures. It does not validate firewall configuration or actual network continuity.

`product_flow.rs` composes registration, DNS, and TLS on ephemeral loopback sockets. Only its test client trusts the root, and its resolver wrapper forbids system integration. It is component evidence, not a full installed daemon, browser, or GUI. Explicit network-exposure/pause controls and dashboard authorization remain unfinished; tests of local defaults do not establish them.

CI provisions the unprivileged profile on Ubuntu 24.04, macOS 15 Apple Silicon/Intel, Windows Server 2025, and Fedora 44 userspace. The Fedora container shares the Ubuntu kernel and has no actual desktop/systemd session; Windows Server is not Windows 11 desktop evidence. A separate website job avoids repeated cross-platform builds, and Python 3.12 fake-state jobs retain minimum-runtime coverage. Manual/tag runs also create unsigned packages/SBOMs after test gates; only tags publish releases. Artifact creation is not installation or signing approval.

## Files and manual contracts

- `run.py`: Portable entry point, candidate/fixture execution, logs, and recovery
- `release/`: Shared coordination/system/service/browser helpers, Python regression tests, and Bun fixtures
- `host/`: Native CLI/daemon fixtures and [desktop checklist](host/gui-checklist.md)
- `check-links.ts`: Built website link/asset/anchor validation, also used by `bun run check:links`
- `ci-changes.sh`: CI job-input classification
- `tray-debug.ps1`: Read-only Windows tray/build/install diagnostics
- `manual/`: [Browsers](manual/browser.md), [system integration](manual/system-integration.md), [installation](manual/installation.md), and [network/sharing](manual/network-and-sharing.md) contracts

Runtime elevation, icon/build, packaging, release, and SBOM helpers remain in `scripts/`: they are product/build operations, not test entry points.
