---
title: Release validation
description: 'Non-CI VM validation, host testing, and real browser checks.'
---

# Release validation

This is the runbook for checks **outside hosted CI**: actual OS resolver/trust changes, service supervision, real browsers, interactive desktop/login behavior, and installation/uninstall. Obtain owner authorization and use individual suites on separately restored VMs, not one unattended `all --system --interactive` run. Incomplete artifact or system cleanup can block later stages of `all`.

## What can currently run

| Check | Debian 13 amd64 VM | macOS 15 and 26 arm64 VMs | Windows 10 22H2 amd64 VM |
|---|---|---|---|
| Resolver/trust installation and owned cleanup | `host --system`; shared trust effects still require independent comparison | `host --system`; full-system trust comparison still required | Manual `SYS-16`; automated system checks remain blocked |
| Actual service supervision | `service` with a real systemd user session | `service` with a logged-in launchd GUI session | Not applicable: no CLI service shipped |
| Real browser | Fixture-owned setup with `browser --system` | Fixture-owned setup with `browser --system` | Preconfigured mode only; separately proven DNS/trust setup required |
| Tray, setup, login, GUI workflow | `desktop`, plus separate logout/login evidence | `desktop --app candidate.app --bin candidate`, plus separate login evidence | Desktop runner blocked |
| Archive/package smoke | `artifacts`; real uninstall remains incomplete | `artifacts`; not complete installed-app evidence | `artifacts`; not complete installed-app evidence |
| Full installation/upgrade/uninstall | Manual checklist with owned integration and full-system comparison | Manual checklist with owned integration and full-system comparison | Manual checklist with owned integration and full-system comparison |
| Signing/notarization | Unsigned alpha; checksums and SBOMs | Optional ad-hoc resource integrity only; not trusted or notarized | Unsigned alpha; no Authenticode claim |

A VM does not bypass a blocked runner. Do not remove its guards, force system checks through direct scripts, or mark blocked platform coverage passed. Required native signoff uses Debian 13 amd64, macOS 15 arm64, macOS 26 arm64, and Windows 10 22H2 amd64. Run the Mac column separately on 15 and 26 using the same Mac 15-built ARM artifact and record its source/hash in each report. Other versions/architectures are best effort. Missing native Mac 15 access remains an access gate, not a deferred hardware-specific check.

## Prepare and save the VM baseline

1. Create a disposable VM with a real desktop and a fresh local test user. Use Debian 13 amd64, macOS 15 arm64, macOS 26 arm64, and Windows 10 22H2 amd64 for required signoff. Ubuntu, Fedora, Intel macOS, and other targets are best effort. Containers, WSL, Windows Server, and a different Mac major version do not substitute for the required OS checks.
2. Before changing the guest, persist a complete restorable backup of its disks and VM configuration outside the guest and outside temporary storage. Check that your VM manager can restore it. Do not rely solely on an in-guest snapshot or `system-before/` report.
3. Install Python 3.12 or newer, Git, and the suite-specific tools below. Install Bun for browser fixtures and the selected real browsers with clean profiles and no accounts. Install native GUI runtime dependencies for the candidate package; source builds additionally need the [build prerequisites](index.md#prerequisites). Use only synthetic projects and credentials.
4. Copy the exact candidate assets and a checkout of their source revision into the VM. Record the run URL, source SHA, OS image/version/architecture, tool/browser versions, and artifact SHA-256 digests. Do not execute candidate binaries on your everyday host merely to check their version.
5. Save a second full-system backup as the **prepared baseline**, still without testd user data, resolver/trust integration, running daemon, or login entry. Preserve installed tools/browsers in this baseline so repeated runs use the same environment.
6. Before each sensitive suite, restore that prepared baseline, verify the guest is clean, and choose a new persisted report path. Block external network egress for browser tests after installing dependencies; keep loopback working. Disable unnecessary shared host folders and real providers. Use a dedicated private evidence-transfer location only.
7. After each suite, stop candidate apps, daemons, browser fixtures, and providers. Retain the report and before/after evidence outside the VM before restoring it. Restore the baseline before the next suite, browser, package, upgrade, or seeded-conflict case. A report left only on a reverted guest is lost evidence.

On Linux, the resolver and service suites need a running systemd-resolved and systemd user manager. Provision tools **only inside the backed-up disposable guest**:

```sh
# Debian 13 (required), Ubuntu 24.04 (best effort)
sudo apt-get update
sudo apt-get install -y python3 git curl openssl systemd-resolved ca-certificates libnss3-tools libcap2-bin

# Fedora 44 (best effort): use this instead of the apt commands
sudo dnf install -y python3 git curl openssl systemd-resolved ca-certificates nss-tools libcap
```

From a terminal in the guest's logged-in desktop session, check the prerequisites without changing them:

```sh
python3 --version
systemctl is-active systemd-resolved
resolvectl status
systemctl --user show-environment
```

If the resolver or user manager is unavailable, prepare a supported clean guest first. On Fedora, record `getenforce` and retain SELinux enforcement; do not switch to permissive to obtain a pass. Do not stop unrelated listeners or overwrite conflicting DNS configuration to obtain a pass. Run suite commands directly in an interactive terminal as the test user, not under `sudo python`, through a pipe, or with redirected stdin. Cache sudo only where shown below. On Windows use `python` or `py -3` instead of `python3`.

## Prepare the candidate and per-run paths

Download only the matching `bundles-TARGET` artifact from the successful manual CI run. Required targets are `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl` (CLI only), and `aarch64-apple-darwin`; `x86_64-apple-darwin` is optional and may be absent. Use the GNU Linux artifact for native desktop validation. Keep each target's files in its own candidate directory. Manual builds do not provide the release job's `SHA256SUMS`; generate a local manifest from the downloaded bytes before transferring them:

```sh
python3 scripts/release.py checksums /absolute/path/to/new-candidate-directory
```

This manifest detects later changes; it is not a published release checksum, signature, or proof of source provenance. Preserve it with the run metadata. Do not regenerate an existing manifest to conceal changed files. For published releases, use their original manifest instead.

Inside the VM, extract the CLI archive into a dedicated candidate directory. Set `CLI` to that executable and, for desktop testing, `APP` to the actual executable from the selected package or verified source build. The app and CLI must match the intended source revision. These POSIX-shell variables are recreated after each baseline restore:

```sh
CLI='/absolute/path/to/candidate/testd'
APP='/absolute/path/to/candidate/testd-gui'
ASSETS='/absolute/path/to/candidate-assets'
REPORT="$HOME/testd-reports/$(date -u +%Y%m%dT%H%M%SZ)"
```

Replace the paths before use. GitHub artifact downloads do not preserve Unix executable permissions: inside the backed-up Linux guest, use `chmod u+x /absolute/path/to/candidate.AppImage` before its runtime smoke check if needed. Preserve the downloaded original; record permission changes separately from content hashes. The runner creates the report directory; do not create it yourself or reuse a previous path. For multiple commands on the same restored guest, choose distinct report suffixes. Candidate hashes, command output, OS metadata, and source revision are retained by the runner.

## Run Linux resolver and trust checks

Restore the prepared baseline, set the candidate/report variables, then run:

```sh
sudo -v
python3 tests/run.py host --system --bin "$CLI" --report "$REPORT-system"
```

Do not run `testd init` first. This suite creates an isolated fixture home, installs its test-only route and public root, tests OS resolution and trusted HTTPS, stops its fixture, and invokes ownership-checked cleanup. It refuses existing testd integration. It can change the systemd-resolved drop-in, the distro CA anchor, and generated shared trust-store output/hooks.

Inspect `report.json`, command logs, and `system-before/`. Independently compare resolver state, source anchors, effective trust, generated bundles, and hook effects with the persisted full-system baseline. Scoped cleanup alone leaves this stage incomplete. Record the comparison result separately; neither restoring the VM nor editing report JSON converts missing preservation evidence into a pass.

Complete [SYS-01 through SYS-16](https://github.com/stelmost/testd/blob/master/tests/manual/system-integration.md), including interruption/recovery and synthetic unowned/conflicting-state cases. Each seeded conflict needs its own restored snapshot and before/after identity/content evidence; do not ask `host --system` to overwrite it. SYS-15 covers macOS resolver and certificate ownership; SYS-16 remains blocked until Windows ownership safeguards exist.

## Run Linux service supervision

Restore the prepared baseline and run from the test user's real desktop session:

```sh
systemctl --user show-environment
python3 tests/run.py service --bin "$CLI" --report "$REPORT-service"
```

Do not add `--system` or run this as root. The suite uses a randomly named user unit, isolated home, fake provider, high ports, and actual manager-directed start, crash/restart, and stop. Verify the stopped listener and ownership-checked unit cleanup in the report; `service-unit.service` retains the generated unit. On macOS, run the same `service` command as the logged-in desktop user: the fixture loads a unique job into `gui/<uid>`, checks its plist before signalling or unloading it, and retains the plist in the report without installing a login item. Standalone service checks do not require a full-system snapshot because they do not change resolver/trust state or persistent service entries. Windows CLI service remains not applicable.

## Run real Linux browsers

Before saving the prepared baseline, install Playwright where Bun can resolve it from the checkout, and install the engines you intend to test. Do not add a project dependency or modify tracked manifests just for a VM run:

```sh
bun install --frozen-lockfile
bun add --no-save playwright
bunx playwright --version
# Debian/Ubuntu: installs browser dependencies as well as the selected engines
bunx playwright install --with-deps chromium firefox webkit
```

On Fedora, use `bunx playwright install chromium firefox webkit` and supply the engine's Fedora runtime dependencies separately; do not force Ubuntu's dependency installer onto Fedora. Unsupported or missing engines remain blocked. Linux Chromium/Firefox require NSS `certutil`, provided by the packages above. Record Playwright/browser versions and retain them in the prepared baseline.

For an automatic real-engine run, restore the baseline, block guest external egress, then run:

```sh
sudo -v
python3 tests/run.py browser --system --bin "$CLI" --browser-engine chromium \
  --edge-port 8443 --report "$REPORT-chromium"
```

Repeat from separately restored baselines for `firefox` and `webkit`. No prior `testd init` or daemon is required: `--system` creates the fixture home, configures its candidate resolver/root, starts its own host, drives the browser, checks OS TLS before/after a renewed leaf, and performs scoped cleanup. Shared trust effects still require the independent comparison described above. Playwright WebKit is not installed Safari evidence; Chromium is not installed Chrome or Edge evidence.

For the actual installed Chrome, Firefox, or Edge and their certificate viewers, run a separate manual browser session:

```sh
sudo -v
python3 tests/run.py browser --system --bin "$CLI" --edge-port 8443 --report "$REPORT-browser-manual"
```

Open the printed `https://app.releasecheck.test:8443/` URL in a clean profile. If that browser uses its own trust store, import only the fixture's public root from `<report>/browser-home/.config/testd/pki/root.crt` into that disposable profile. Do not use certificate-warning bypasses, `--insecure`, browser-only DNS overrides, or a real account. Complete [BROWSER-01 through BROWSER-08](https://github.com/stelmost/testd/blob/master/tests/manual/browser.md): certificate/SAN/chain/lifetime, WebSocket idle, SSE/upload, cookie scope, dashboard/dead upstream/unknown name, and renewal. Save the page's JSON report, stop with Ctrl+C, close the test browsers, and answer shutdown confirmations truthfully.

On macOS, run system, browser-system, and desktop suites from Terminal in the logged-in desktop session; `launchctl managername` must report `Aqua`. SSH cannot present the system trust-removal authorization. Native trust dialogs require operator approval during cleanup. `browser --system` uses installation-owned resolver and trust receipts with the same cleanup verification. On Windows, `browser` without `--system` supports a separately preconfigured candidate DNS/trust setup, but does not establish that setup's ownership or cleanup. The host must be stopped before running the suite; it starts its own candidate daemon. Do not improvise a fresh-VM root/NRPT/resolver installation to bypass the current platform blockers. Windows browser coverage remains blocked until a separately proven, restorable setup is available. Record installed Safari observations separately from Playwright WebKit.

## Run Linux desktop, login, and installed-package checks

Restore the baseline and install the actual selected `.deb` or prepare the verified AppImage/source-build executable and required GUI runtime libraries. Do not launch or initialize testd before the desktop runner: the test user must still have no testd configuration. On Debian 13, or best-effort Ubuntu, install the verified package and inspect its paths:

```sh
DEB='/absolute/path/to/candidate.deb'
sudo apt-get install "$DEB"
dpkg-deb --contents "$DEB"
```

Set `APP` to the installed GUI executable shown by the package, not the `.deb` file. Fedora does not install a Debian package; use its verified AppImage or a native source build and record the actual packaging format tested.

The real GUI executable must be able to bind port 443. On the backed-up guest only, grant the capability to the actual ELF executable if needed and record the change:

```sh
sudo setcap 'cap_net_bind_service=+ep' "$APP"
getcap "$APP"
python3 tests/run.py desktop --app "$APP" --bin "$CLI" --report "$REPORT-desktop"
```

Do not grant a capability to an AppImage launcher and assume its extracted child inherits it; identify the actual GUI executable. Do not run the entire desktop as root. The runner launches the app and prompts for [GUI-01 through GUI-28](https://github.com/stelmost/testd/blob/master/tests/host/gui-checklist.md). Perform each action before answering `p`, `f`, `n`, or `q`; record evidence/reasons and never include the operator secret. Only confirm shutdown after all app/daemon/provider processes and fixture listeners have stopped. Deferred cleanup requires full-system recovery.

GUI-24 needs a **separate logout/login run**: retain evidence outside the guest, enable the candidate's login item, log out/in, and verify one actual tray starts with the expected state. Record the login artifact and process observations; an existing plist/desktop/registry entry alone is not proof. Do not expect an interactive runner session to survive logout.

For archive smoke on each supported OS/architecture, on a separately restored VM:

```sh
python3 tests/run.py artifacts --artifacts "$ASSETS" --require-checksums --report "$REPORT-artifacts"
```

Windows equivalent, with a new report path on every attempt:

```powershell
python tests\run.py artifacts --artifacts 'C:\candidate-assets' --require-checksums `
  --report "${HOME}\testd-reports\artifacts-$((Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssZ'))"
```

Artifact smoke validates checksums, CLI extraction/version/init/re-init, and applicable package metadata/runtime; it deliberately reports real uninstall/purge as blocked. Expected exit `2` is **incomplete**, not a pass for the installed workflow. `--previous-bin /path/to/real-previous-testd` adds CLI configuration/root preservation checks, not actual package upgrade evidence.

Complete [INSTALL-01 through INSTALL-10](https://github.com/stelmost/testd/blob/master/tests/manual/installation.md) separately for each claimed package: actual installation and first setup, trusted browser workflow, stopped registration, upgrade from a real prior release, interrupted-upgrade rollback, disabled login item, owned integration removal, package removal, and purge/reinstall. On Linux and macOS, stop candidate processes and retain ownership/root evidence before `testd uninstall`; purge only after system cleanup succeeds. Compare unowned and shared state against the full-system baseline. Do not manually remove conflicting state. macOS ownership-checked cleanup still needs native installed/trusted first-run and full-system comparison evidence. Windows cleanup/trusted first-run remains blocked pending native ownership validation; archive smoke does not resolve either gate.

## Verify signing and conditional network claims

The first release is an unsigned alpha, not a trusted-signed delivery. A successful free macOS ad-hoc resource seal is bundle-integrity evidence only, never Developer ID signing, notarization, or Gatekeeper approval. Windows remains unsigned; Linux provides checksums/SBOMs without additional signing infrastructure. Record observed OS security prompts and installation outcomes truthfully. The credentialed checks below apply only to a separately approved trusted-signed release, not this alpha.

For a separately approved Developer ID signed and notarized release, verify the actual app and its ticket:

```sh
codesign --verify --deep --strict --verbose=2 /path/to/Testd.app
codesign --display --verbose=4 /path/to/Testd.app
spctl --assess --type execute -vv /path/to/Testd.app
xcrun stapler validate /path/to/Testd.app
```

For a separately approved Authenticode-signed release, inspect the Windows installer and installed executables:

```powershell
Get-AuthenticodeSignature 'C:\candidate\testd-setup.exe' | Format-List Status,StatusMessage,SignerCertificate
```

For trusted-signing claims, require the expected signer, not merely any valid or ad-hoc signature. SmartScreen reputation and actual OS prompts need separate observations. Missing credentials leave a signed-delivery claim unverified, not passed; the unsigned alpha makes no such claim.

If a release claims real-provider sharing or non-loopback network behavior, use the separately authorized two-machine procedure in [network and sharing contracts](https://github.com/stelmost/testd/blob/master/tests/manual/network-and-sharing.md). Fake providers do not establish real exposure or crash cleanup; unimplemented network controls remain blocked. Do not enable external exposure or a real provider without explicit approval.

## Retain evidence and recover before repeating

Keep `report.json`, `run.log`, command logs, scoped snapshots, and browser/desktop evidence private. Outside the guest, retain a ledger with one row per SYS/BROWSER/GUI/INSTALL/NET/SHARE check: candidate SHA/digest, OS/browser/package, full-system baseline ID/location, result (`pass`, `fail`, `blocked`, or justified `not applicable`), evidence paths, and independent cleanup comparison. Untested, interrupted, and unanswered checks never become passes.

If interrupted, stop every candidate/fixture process and inspect the retained report. An interactive scoped recovery may be attempted on the same guest:

```sh
python3 tests/run.py --restore-report /path/to/retained-report
```

This is not a full-system restore. Conflicts stay untouched, and Linux/macOS shared trust effects may still produce exit `2`. Preserve recovery logs and restore the full prepared VM baseline before any further sensitive test. Copy evidence out **before** reverting. A restored guest proves recoverability, not that the candidate's uninstall preserved all unowned state.

## Host testing

The container suite covers the protocol, the edge, registration ownership, DNS answers, CLI behavior, sharing, and the standalone operator secret. The checks below cover OS resolver integration, trust stores, privileged ports, packaging, code signing, and the tray.

Run them on real machines, not inside the dev container. A macOS, Linux, and Windows host are needed for full coverage. Use a dedicated test machine or a VM: the optional system checks change the OS resolver and trust store.

### What the scripts cover

| Script | Checks |
|---|---|
| `tests/host/linux.sh` | resolver route, CA trust, edge TLS, loopback dashboard, DNS A records, token requests, issued tokens, local registration, sharing, quick launches, URL actions, doctor state checks, launch-at-login entry, systemd unit |
| `tests/host/macos.sh` | Edge, dashboard, DNS, registration, sharing, quick launches, doctor state, login entry, and plist checks; resolver and trust checks with `host --system` |
| `tests/host/windows.ps1` | Unprivileged edge, dashboard, DNS, registration, sharing, quick launches, and launch-at-login checks; NRPT and root trust checks are blocked |
| `tests/host/gui-checklist.md` | browser setup/handoff, native tray, dashboard tab, click-to-open, single instance, quick launches, certificate controls, URL actions, packaging/signing |

The Linux and macOS scripts share their core checks in `tests/host/common.sh` and their helpers in `tests/host/lib.sh`; the platform scripts add the resolver, trust, and init-system sections.

The sharing checks start the daemon with `TESTD_TUNNEL` pointing at a fake provider, so they need no provider installed: start, list, refuse a second share, stop, refuse an unknown provider, and refuse a name that is no longer registered. The provider shapes themselves (cloudflared, ngrok, tailscale) are covered by `cargo test -p testd-daemon --test share`, which the Windows script cannot do because a PowerShell script is not a spawnable provider binary.

Local registration uses the operator secret that `testd init` writes into `[host] secret`; the standalone host has no accounts or roster. The scripts run the client on the node itself, where the default loopback profile uses that secret automatically; no `TESTD_TOKEN` is exported. A separate named profile with no credential requests a token, which the script approves and then revokes through `testd requests`, `testd approve`, and `testd tokens`.

The quick launch checks need `python3` (or `python` on Windows) to start a local HTTP server. They save a launch through the loopback dashboard, start it, check the name through the edge and DNS, stop it, and delete it.

Each script uses:

- a throwaway `HOME`/`%APPDATA%` retained under the persisted report directory,
- a test-only throwaway TLD (`testdcheck` by default) so it does not collide with a real `.test` setup,
- a high HTTPS port (`8443`) so it does not need root for the edge,
- dedicated upstream ports for registration (`18080`) and quick launches (`18081`).

The scripts do **not** call sudo themselves. On Linux and macOS they use `--cacert` for HTTPS; on Windows they use `--insecure` for edge and proxy checks because Schannel does not treat `--cacert` like OpenSSL. Linux and macOS runners permit optional OS resolver/trust checks with owner authorization, a cached sudo credential, a disposable-host confirmation, and a persisted full-system snapshot. macOS also requires the logged-in GUI terminal and native trust-removal approval. Windows system checks remain blocked pending native ownership validation.

### Linux

Prerequisites: `bash`, `curl`, `python3`, `resolvectl` (for the optional system
check), `systemd-analyze` (optional). `sudo` must be available if
`TESTD_HOST_TEST_SYSTEM=1`.

```sh
cd testd
cargo build --release -p testd

# Core checks, no system changes
tests/host/linux.sh

# Optional system checks on a backed-up disposable VM, with a new persisted report
sudo -v
python3 tests/run.py host --system --bin target/release/testd \
  --report ~/testd-reports/linux-system-01
```

The Linux system check can write `/etc/systemd/resolved.conf.d/90-testd.conf` and the distro's CA anchor: `/usr/local/share/ca-certificates/testd-root.crt` on Debian and Ubuntu or `/etc/pki/ca-trust/source/anchors/testd-root.crt` on Fedora and RHEL. Cleanup uses the candidate's ownership checks and reports conflicts without replacing or deleting unowned state. Trust-store regeneration and hooks can affect additional shared files, which the scoped snapshot does not verify.

### macOS

Prerequisites: `bash`, `curl`, `python3`, `plutil`, and `dscacheutil`. Xcode command-line tools are needed to build.

```sh
cd testd
cargo build --release -p testd
tests/host/macos.sh
```

The unprivileged host checks do not install a resolver or trust root. macOS system/browser/desktop checks use installation-owned resolver and certificate receipts and refuse changed or unowned integration. Run them from the logged-in desktop's terminal and approve native trust-removal authorization. The service suite uses a uniquely owned job with the packaged launchd template. Scoped snapshots and cleanup do not verify every keychain trust setting or shared effect; native full-system comparison remains required.

### Windows

Prerequisites: Python 3.12 or newer, PowerShell 5.1 or later, `curl.exe` (included with Windows 10 1803+), and a free port 53. The daemon uses port 53 for NRPT; if another service owns it, the script fails at the DNS check.

The unprivileged script prints the OS caption, version, build, and architecture at the start of every run. Compilation, PowerShell fixtures, and CLI checks do not validate NRPT, machine trust, or a desktop Windows release.

CI's hosted Windows runners use Windows Server, not Windows 10 22H2. They are useful for compilation and CLI fixtures, but do not replace required Windows 10 VM signoff, NRPT/trust checks, or desktop/installer evidence.

```powershell
cd testd
cargo build --release -p testd

# Core checks: edge and proxy without system trust
powershell -NoProfile -ExecutionPolicy Bypass -File tests\host\windows.ps1
```

The unprivileged run intentionally uses `curl --insecure --ssl-no-revoke` for edge and proxy checks. That validates routing and TLS minting without requiring a trust-store change. The loopback dashboard is proxied through the edge and returns 200; these checks do not establish OS trust.

`testd init` and `testd doctor --fix` no longer need the whole process elevated
on Windows. Privileged actions are delegated to
`scripts/testd-elevated.ps1`, a closed-action helper that validates the TLD,
IPv4 address, certificate thumbprint, and CA constraints. If testd is not
elevated, Windows shows one UAC prompt for that helper only.

The Windows snapshot helper verifies NRPT rules and machine roots without deleting or recreating them by name. Automated system and desktop runners remain blocked; use the [manual ownership checklist](https://github.com/stelmost/testd/blob/master/tests/manual/system-integration.md#platform-evidence) for native Windows uninstall evidence and the persisted full-system snapshot for recovery. Windows NRPT rules cannot specify a port, so the daemon binds `:53`. If it is occupied, use a different disposable host rather than stopping an unrelated service.

### Launch at login

Launch at login is off by default and changes only through the tray's **Launch at login** checkbox. Startup queries the existing entry without creating or restoring it. Linux creates the existing desktop-entry format as an owner-private file without replacement, independent of the caller's umask. Windows uses `auto-launch` 0.5; macOS keeps the same LaunchAgent but serializes its raw executable path with `plist`, creating the file atomically without replacing an existing entry. Queries and changes run off the native event-loop thread. Failures are reported and an unavailable state is not presented as a false unchecked preference. Changed entries or entries pointing elsewhere are preserved and reported rather than silently replaced. AppImage entries target the persistent image, not its mounted inner executable.

| OS | Artifact | Check |
|---|---|---|
| macOS | `~/Library/LaunchAgents/dev.testd.host.plist` | `plutil -lint`, and `launchctl print gui/$(id -u)/dev.testd.host` |
| Linux | `~/.config/autostart/dev.testd.host.desktop` | has an `Exec=` line pointing at the tray binary |
| Windows | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `dev.testd.host` | Read the entry without treating it as candidate evidence |

The CLI-only host scripts cannot establish that an existing login entry belongs to the candidate GUI. Windows records an existing entry as unverified, not as a pass or failure. The GUI checklist covers the actual run and toggle.

### Browser validation

The [Browser validation](#browser-validation) section holds the per-browser script: trust stores, padlock
and certificate expectations, WebSocket and SSE behaviour, cookie scope across
`app.example.test` and `api.example.test`. The host
scripts stop at the HTTP layer on purpose; they cannot judge a padlock.

### GUI

The tray is a public source build and a free release artifact. On a backed-up disposable Debian 13 desktop, run `python3 tests/run.py desktop --app <candidate> --bin <candidate-cli> --report <new-report>` to record `tests/host/gui-checklist.md`. The app and CLI must come from the intended source. macOS 15 and 26 arm64 desktop runs use installation-owned resolver/trust cleanup and require the logged-in GUI session and native authorization. Windows desktop runs remain blocked pending native ownership validation. Attach/detach of existing processes is deferred and does not gate this release.

### Packaging and signing

The unsigned alpha needs no paid signing credentials. The following credentialed reference applies only to separately approved trusted-signed delivery, not the alpha:

- **macOS:** Developer ID certificate, notarization credentials, `codesign`,
  `spctl`, `xcrun stapler`.
- **Windows:** Authenticode certificate and SmartScreen reputation; verify with
  `Get-AuthenticodeSignature`.
- **Linux:** unsigned `.deb`, AppImage, and tarball; verify with `dpkg-deb -I`
  and an AppImage smoke run.

See [Releasing](releasing.md) for the release checklist and `tests/host/gui-checklist.md`
for the per-artifact commands.

### Environment variables

| Variable | Default | Meaning |
|---|---|---|
| `TESTD_BIN` | `target/release/testd` | Binary to test |
| `TESTD_HOST_TLD` | `testdcheck` | Test-only throwaway TLD; must be 1 to 10 lowercase alphanumeric chars |
| `TESTD_HOST_EDGE_PORT` | `8443` | HTTPS edge port |
| `TESTD_HOST_APP_PORT` | `18080` | Test upstream port |
| `TESTD_HOST_LAUNCH_PORT` | `18081` | Quick-launch upstream port |
| `TESTD_HOST_TEST_SYSTEM` | `0` | Allow resolver and trust changes |
| `TESTD_HOST_SNAPSHOT` | unset | Snapshot directory written by `run.py --system` |

### System integration

Before permitted system checks, `run.py` records selected integration under `<report>/system-before/`. The scoped snapshot is not a full-system backup and does not capture every artifact, trust setting, generated store, or hook affected by the product.

| Platform | Selected snapshot | Cleanup policy |
|---|---|---|
| Linux | The systemd-resolved drop-in, the distro's CA anchor, and their ownership markers | Candidate-owned integration is removed through the candidate's ownership checks; other changes are left untouched |
| macOS | The test TLD resolver and receipt, certificate receipts, and named testd roots in the System keychain | Candidate-owned integration is removed through its ownership checks; changed or unowned state is preserved |
| Windows | NRPT rules for the test TLD and named testd roots in the machine store | Verify only; never remove or recreate rules or roots by name |

File fixtures use exclusive creation and record their identity and content before cleanup. A replaced, changed, deleted, or unowned artifact is reported, not overwritten or removed. Pre-existing keychain and machine-root trust settings are not captured and cannot be verified from this snapshot. System stages remain incomplete when shared trust-store output or hooks have not been compared with the persisted full-system snapshot.

After an interrupted run, stop all candidate processes and inspect the persisted report and full-system snapshot. An interactive scoped recovery can be attempted with the relevant report or `self-test-state` directory:

```sh
python3 tests/run.py --restore-report ~/testd-reports/linux-system-01
```

Recovery requires the same platform and any necessary sudo or elevation. It leaves conflicts untouched. Exit `2` means scoped state matched but shared trust effects remain unverified; restore or compare the full-system snapshot before reusing the machine. Direct host scripts cannot run privileged checks without the runner's safeguards.

### CI coverage

`.github/workflows/ci.yml` requires the `ci` suite on macOS 15 arm64 (`macos-15`), macOS 26 arm64 (`macos-26`), and Debian 13 amd64 userspace. Both Mac labels are listed as arm64 in GitHub's [available runner images](https://github.com/actions/runner-images#available-images); Intel uses separate labels. Ubuntu 24.04, macOS 15 Intel, Windows Server 2025, and Fedora 44 userspace are best effort. Hosted OS jobs install Python 3.14; container jobs use their distro Python. Each job provisions stable and MSRV Rust, `cargo-deny`, Bun, and checkout dependencies. Linux build libraries come from the local dependency action; Xcode and MSVC/Windows SDK prerequisites come from hosted runner images. Rust caches are isolated by platform, and only master runs save them.

Debian 13 and Fedora 44 container jobs run as a normal user on the Ubuntu runner's kernel. They establish userspace compilation and CLI/daemon behavior, not native kernel, resolver, trust, systemd-session, SELinux, or desktop integration. Hosted Windows Server is not Windows 10 22H2 validation; the required Windows 10 VM signoff remains separate.

Every applicable job runs ignored contracts and executable doctests. Missing prerequisites, unexpected skips, and incomplete checks keep the CI command nonzero; best-effort jobs tolerate that result only at the workflow level, not by changing check statuses. The workflow uploads only `ci-summary.json`, never fixture state or full reports. Python 3.12 fake system-state fixtures retain minimum-runtime coverage; both macOS 15 and 26 are required and Ubuntu/Windows Server are best effort. The separate `.github/workflows/site.yml` workflow runs the `site` profile's isolated website checks once.

Real system, service, browser-trust, installer, and desktop checks still require owner authorization and a disposable host with a persisted full-system snapshot; hosted CI does not run them. Version-tag pushes and manual `.github/workflows/release.yml` runs call both check workflows without path filtering, then build required Windows x64, Linux x64, and one macOS arm64 artifact on macOS 15, plus SBOMs after required runtime, minimum-runtime fixture, and website gates. Independently validate that same ARM artifact on native 15 and 26; a Mac 26 VM does not replace missing Mac 15 access. Intel macOS artifacts are optional. Artifact requirements are keyed by target, not builder OS. Manual branch runs use the workspace version; tag-ref builds check the tag version against the workspace. Only version-tag pushes can write GitHub Releases. Package creation does not replace installation, signing, or uninstall evidence. A green workflow is not release approval. Repository required-check settings must track the required job names and must not make best-effort jobs mandatory. GitHub leaves required checks pending when their entire workflow is skipped by path filters; configure branch protection to account for the independent CI and site workflows.

### System checks need a running daemon

The Linux resolver and trust checks query the fixture daemon. The runner verifies that its fixture ports are free before cleanup and refuses to stop or replace unrelated listeners. A missing daemon or unresolved cleanup is reported rather than counted as a pass.

### Interpreting failures

- `init` fails: check the printed log. The script uses a temporary home, so a
  pre-existing lock in your real home is not involved.
- `DNS listener` fails: another process owns the port. On Windows this is
  usually `:53`; on Linux/macOS it is `:7753`.
- `HTTPS listener` fails: the configured `edge_addr` is in use, or the binary
  cannot bind it.
- Dashboard TLS fails but `curl --insecure` works: the root is not trusted. On
  Windows, Schannel reports `CERT_TRUST_REVOCATION_STATUS_UNKNOWN` for a private
  root and does not honor `--cacert` the way OpenSSL does; use
  `--ssl-no-revoke` and either `--insecure` before trust or the machine root
  store after `testd doctor --fix`.
- System resolver check fails after `doctor --fix`: inspect
  `resolvectl status` (the route is a global `DNS=`/`Domains=` pair) or
  `/etc/resolver/testdcheck` (macOS), and make sure the daemon is still running.
  On Linux, `doctor --fix` refuses to write the drop-in when systemd-resolved is
  inactive or already has global `DNS=`/`Domains=` configured, and names the
  setting it would have replaced.

## Browser validation

Manual checks that a name works in a real browser. Use a disposable host with a persisted full-system snapshot. A separate device or non-loopback exposure needs separate approval and synthetic data.

### Preconditions

For the fixture-driven browser suite, follow [Run real Linux browsers](#run-real-linux-browsers); do not start a daemon beforehand. The following applies instead to a separately authorized installed-app workflow on a backed-up disposable host. Linux and macOS use installation-owned DNS/trust setup and cleanup; Windows product-managed setup remains blocked pending native ownership validation.

- Initialize the intended candidate, establish owned Linux or macOS DNS/trust integration, and start its host; `testd doctor` must exit `0`
- Start a synthetic local service and register `testd run app.example:3000 -- <your dev server>`
- Trust only the candidate's public root using the proven OS setup or a clean disposable browser profile; record its fingerprint and the import method

### Trust stores

| Browser | Store | Import |
|---|---|---|
| Chrome, Edge (Linux) | Browser certificate manager or platform integration; varies by package | Import `root.crt` as a trusted authority in the browser certificate manager and verify the actual package's trust behavior |
| Chrome, Edge (macOS), Safari | System keychain | Use installation-owned `testd doctor --fix`; retain receipts and approve native trust dialogs |
| Chrome, Edge (Windows) | Machine store | Product-managed checks blocked pending native ownership validation; preconfigured checks require separately proven setup |
| Firefox (all) | Profile NSS store, or system roots when enabled | Settings → Privacy & Security → Certificates → View Certificates → Authorities → Import; trust the root for websites |

For NSS command-line imports, use `certutil -d sql:<actual browser profile directory> -A -t 'C,,' -n testd-root -i root.crt`. Do not assume `~/.pki/nssdb` is the Firefox profile. Firefox's `security.enterprise_roots.enabled` can use supported OS root stores; validate it on the actual platform/package rather than assuming system installation is sufficient.

### Checks

Run each in every browser you support: Chrome, Firefox, Safari, Edge.

1. `https://app.example.test` loads with no certificate interstitial or bypass. The leaf is issued by `testd intermediate`, chains to `testd root`, carries `app.example.test` in its SAN, and expires within 24 hours.
2. Wait for the leaf to be re-minted, or restart the daemon: a reload still shows a valid certificate with a newer validity window. When the browser suite started the host, use the page's restart button.
3. The tray's `testd.test → dashboard` row opens the local dashboard in the default browser through a private handoff file, or `testd dashboard` prints the authenticated URL. The page lists the live name. `testd.test` is served by the edge for loopback clients only.
4. WebSocket: a dev server using HMR websockets connects over `wss` and survives 60 seconds idle.
5. SSE and a 100 MB upload complete without a truncated response.
6. Cookies: two names in one project (`app.example.test`, `api.example.test`) share a cookie set on `.example.test`, exactly as `app.example.com` and `api.example.com` would.
7. Stop the upstream, reload: the edge returns `502`. An unregistered name does not reach an application; DNS or TLS may reject it before any HTTP response.
8. Single-label warning: `testd run app:3000` prints the cross-site warning, and `app.test` still resolves.

### Record

For each browser: name and version, OS, pass or fail, and the leaf's SHA-256 fingerprint. Keep the result in the release notes or the host test log.
