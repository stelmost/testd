---
title: Testing
description: 'Hosted CI platform matrix, local suite reproduction, and targeted Rust checks.'
---

# Testing

Use hosted CI for the repeatable platform matrix. Use the OS-detecting `tests/run.py` to reproduce a failure locally. Run local commands from the repository root; GitHub commands below explicitly select `stelmost/testd` and also work from another directory.

| Run | Scope |
|---|---|
| Push to `master` or open a pull request | Change-filtered automated checks; documentation-only changes may skip builds |
| Manual `ci.yml` workflow run | Required and best-effort platform matrix, minimum-runtime fixtures, website checks, native package builds, and workspace SBOMs; no release publishing |
| Local default `check` profile | Non-GUI Rust checks, fixtures, audit, isolated website, and CLI/daemon checks; `--gui` includes native GUI compilation/tests |
| Local `ci` profile | Unprivileged whole-workspace checks including GUI; the separate CI `site` job validates the website once |
| Local `all` profile | `check` plus explicitly selected system, artifact, desktop, browser, and manual stages; incomplete selected evidence remains blocked |

## Run the full matrix on GitHub

No local Rust, Python, Bun, or VM setup is needed for hosted CI. The workflow provisions its tools and build libraries. With the [GitHub CLI](https://cli.github.com/) installed, check authentication with `gh auth status`; the account must be allowed to dispatch this repository's workflows. A personal access token also needs `workflow` scope when pushing changes to `.github/workflows/`.

Commit and push the revision you want tested before dispatching. `--ref master` uses GitHub's copy of `master`, not local or uncommitted changes; replace `master` with a pushed branch name to test that branch. Do not push a version tag just to run tests: version-tag pushes can write GitHub Release assets.

```sh
gh workflow run ci.yml --repo stelmost/testd --ref master
gh run list --repo stelmost/testd --workflow ci.yml --event workflow_dispatch --limit 5
```

Alternatively, open the repository's **Actions → ci → Run workflow**, select the branch, and start the run. Manual runs include package and SBOM builds only after the test gates pass. GitHub Actions retains their artifacts. The release-writing job stays skipped even when the dispatch ref is a version tag.

Replace `RUN_ID` in the following commands with the numeric ID from the new run. Confirm its source SHA and URL before interpreting results:

```sh
gh run view RUN_ID --repo stelmost/testd --json headSha,status,conclusion,url
gh run watch RUN_ID --repo stelmost/testd --interval 45 --exit-status
gh run view RUN_ID --repo stelmost/testd --web
```

`watch --exit-status` returns nonzero for an unsuccessful run. A transport error or a watch timeout is not itself a CI failure: inspect `run view` and resume watching the same run. Require `ci-summary-macos-15`, `ci-summary-macos-26`, and `ci-summary-debian-13` to show every applicable selected unprivileged stage passing with the expected GUI scope. For a full manual run, also require the website, macOS 15 and 26 minimum-runtime fixtures, Windows x64/Linux x64/macOS arm64 bundle jobs, and SBOM job to pass. Only one required ARM bundle is built, on macOS 15. Other runtime summaries and Intel bundles are best effort; review their failures and incomplete coverage without relabelling them as passes. An intentionally skipped release-publishing job is not missing test evidence.

## Inspect failures and repeat a run

Wait for the whole run to complete before requesting its combined failure log:

```sh
gh run view RUN_ID --repo stelmost/testd --log-failed
gh run rerun RUN_ID --repo stelmost/testd --failed
gh run watch RUN_ID --repo stelmost/testd --interval 45 --exit-status
```

Use `rerun --failed` for a transient infrastructure failure. A rerun uses the original commit and workflow, not newly pushed fixes. After fixing code, commit and push it, dispatch a new run, and use the new run ID. For a complete repeat of the original run, omit `--failed`.

## Retain reports and built artifacts

Use a new, private, persisted destination for each run or rerun attempt. Replace `OUTPUT_DIR` below with that destination; do not overwrite an earlier attempt's evidence. The download commands work in both POSIX shells and PowerShell.

```sh
gh run download RUN_ID --repo stelmost/testd --pattern 'ci-summary-*' --dir 'OUTPUT_DIR/summaries'
gh run download RUN_ID --repo stelmost/testd --pattern 'bundles-*' --dir 'OUTPUT_DIR/bundles'
gh run download RUN_ID --repo stelmost/testd --name sbom --dir 'OUTPUT_DIR/sbom'
gh run view RUN_ID --repo stelmost/testd --log > 'OUTPUT_DIR/run.log'
```

Each `ci-summary-*` artifact contains `ci-summary.json`, with source/OS/Python metadata and stage statuses. Package and SBOM artifacts are available from full manual or tag runs, not ordinary push/PR runs. Full runner reports, fixture homes, PKI, browser profiles, and source patches are not uploaded. GitHub retains command output in the job logs; review logs before sharing because fixture output can contain sensitive data. Built artifacts are unsigned and are not installation or signing evidence.

## Reproduce the unprivileged suite locally

Install the [prerequisites](index.md#prerequisites), including the native tray build libraries/compiler even if you only want to run `ci`. Python 3.12 or newer is supported; hosted OS jobs use 3.14, userspace containers use their distro Python, and minimum-runtime fixtures use 3.12. Ensure the rustup shims and Bun are on `PATH`; a distro-provided `cargo` cannot interpret `+stable`. On Linux/macOS, load `source "$HOME/.cargo/env"` in the current shell if needed; on Windows, reopen PowerShell after installation. Prepare the shared tools and checkout dependencies once:

```sh
rustup toolchain install stable 1.90.0 --profile minimal
cargo +stable --version
rustup component add --toolchain stable rustfmt clippy
cargo +stable install --locked cargo-deny
bun install --frozen-lockfile
python3 tests/run.py prepare --gui
```

The runner installs nothing. Run it from a normal user terminal with no existing testd daemon occupying the fixture ports. Do not initialize testd or install system integration first: the host checks create their own isolated home. Unset `CARGO_BUILD_TARGET` for automatic native builds, or pass `--bin` with a separately verified candidate CLI. When working in a Git worktree, point `CARGO_TARGET_DIR` at the main checkout's `target/` to share the build cache.

On Debian, other Linux distributions, and macOS:

```sh
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
python3 tests/run.py ci --plan
python3 tests/run.py ci
```

On native Windows, use PowerShell, not WSL; no elevation is needed for `ci`:

```powershell
$env:CARGO_INCREMENTAL = '0'
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
python tests/run.py ci --plan
python tests/run.py ci
```

The runner prints a new persisted report path; choose another with `--report` if needed. Existing paths and temporary storage are refused. Use the default `check` profile for non-GUI checks including the website, or `site` for website-only checks. To repeat a run, retain the previous report and let the runner create a new one. Windows also requires a free UDP port 53. Other occupied fixture ports are reported by the runner; do not stop unrelated services merely to make a test pass.

## Read local outcomes and remaining coverage

Inspect `report.json` for consolidated stage/command evidence and `run.log` for progress. Full raw command output remains in the indexed stage logs; use `--verbose` to mirror it into the console and transcript. The `ci` suite additionally writes the publishable status-only `ci-summary.json`.

| Exit code | Meaning |
|---|---|
| `0` | Applicable checks completed without failures |
| `1` | At least one failure |
| `2` | Incomplete coverage without failures |

Missing prerequisites and unexpected skips are not passes. The `ci` profile excludes actual tray/login observations and privileged, installer, real-browser, and manual stages. `all` adds only explicitly selected sensitive stages; their incomplete evidence remains blocked. Unselected release checks are neither run nor passed.

Debian and Fedora containers establish userspace behavior on the Ubuntu runner's kernel, not native system or desktop integration. Hosted Windows Server is not Windows 10 22H2 evidence. Resolver/trust changes, service supervision, real browser trust, desktop setup, installation/uninstall, and signing/notarization remain separate checks. For those operations use the [release-validation commands](release-validation.md) on a clean disposable host with a persisted full-system backup, retain reports outside the guest before recovery, and restore the baseline between sensitive stages. macOS supports installation-owned resolver/trust cleanup from a logged-in desktop terminal with native authorization; shared trust effects still need full-system comparison. Windows product system/desktop checks remain blocked pending native ownership validation. Do not bypass guards or append `--system` to `ci`.

See [tests/README.md](https://github.com/stelmost/testd/blob/master/tests/README.md) for individual suite commands, detailed contracts, browser preparation, and recovery requirements, and [CI coverage](release-validation.md#ci-coverage) for the hosted platform matrix. A green CI run is not release approval. Runner images, stable Rust, and installed tool versions can change between runs even when the source commit and lockfiles are unchanged.

## Targeted checks

For quick individual checks without a runner report:

```sh
cargo +stable test --workspace --all-features --all-targets --no-fail-fast -- --include-ignored --show-output
cargo +stable clippy --workspace --all-targets --locked -- -D warnings
cargo +stable fmt --all -- --check
cargo +stable deny --workspace --locked check
```

The test command above includes ignored contracts. The default runner also includes them; `contracts` is the focused Rust 1.90 runtime check, while routine profiles type-check the MSRV and run tests/doctests on stable Rust. Exclude `testd-gui` from workspace commands if the native GUI build prerequisites are absent.
