# Developing

Developer documentation lives in [`docs/developers/`](docs/developers/index.md) and is published at <https://testd.dev/docs/developers/>.

- [Overview, prerequisites, build, icons, and website](docs/developers/index.md)
- [Testing](docs/developers/testing.md): hosted CI matrix, local suite reproduction, and targeted checks
- [Release validation](docs/developers/release-validation.md): non-CI VM validation, host testing, and browser validation
- [Releasing](docs/developers/releasing.md): release checklist, Homebrew definitions, and signing/notarization
- [Dependency policy](docs/developers/dependency-policy.md): desktop integration library choices, advisory exceptions, and MSRV constraints

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

Native signoff on both macOS 15 and 26 remains required, using one ARM artifact built on macOS 15 and checking identical source/artifact hashes on each OS. Missing native macOS 15 access is an access gate, not a hardware-specific deferral; Mac 26 cannot close it. Native installation, browser/tray, resolver/trust cleanup, and approval of any claimed trusted signing remain separate from automated gates. Releases ship as unsigned alpha artifacts; macOS builds use optional ad-hoc resource integrity only.
