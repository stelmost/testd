---
title: Quickstart
description: 'Install testd, set up your host with the desktop app or CLI, and open your first HTTPS name.'
---

# Quickstart

This guide takes you from a clean machine to opening `https://app.example.test` in your browser.

Your **host** is the machine with the browser. Your project can run on that same machine, or on another machine (a VM, container, WSL distro, or remote dev box).

```text
browser
  │  DNS: app.example.test → host
  ▼
host machine (desktop app or testd host)
  ├─ local DNS for *.test
  ├─ HTTPS edge on :443
  └─ reverse proxy → your app
                         ▲
                         │  one outbound connection
leaf (local machine, VM, container, WSL, or remote box)
  └─ testd run
```

The browser only ever talks to the host. The host terminates HTTPS and forwards traffic to the app.

## 1. Install testd

testd ships as two binaries that share the same config directory, local CA, and dashboard:

- **Desktop app (GUI):** runs the host daemon from your system tray or menu bar with guided first-run setup in your browser. Best for your **host** machine.
- **CLI (`testd`):** registers projects with `testd run` from local terminals or remote dev boxes (VMs, containers, WSL, SSH servers), and can also run the host headlessly (`testd host`).

You can install both on your host machine: the desktop app runs the host while `testd` registers projects from your terminal. Always verify `SHA256SUMS` from [GitHub Releases](https://github.com/stelmost/testd/releases) before installing a downloaded package.

<DocTabs :labels="['Desktop app (GUI)', 'CLI (Linux / macOS)', 'CLI (Windows)', 'From source']">
<template #tab-0>

Download the package for your platform from [GitHub Releases](https://github.com/stelmost/testd/releases):

| Platform | Package | Install |
|---|---|---|
| macOS (Apple Silicon) | `Testd_<version>_aarch64.dmg` | Open the `.dmg` and drag `Testd.app` into Applications (macOS 13+) |
| macOS (Intel, best effort) | `Testd_<version>_x64.dmg` | Open the `.dmg` and drag `Testd.app` into Applications |
| macOS (Homebrew) | `testd-gui.rb` from the release | `brew install --cask ./testd-gui.rb` |
| Linux (`x86_64`, GNU libc) | `Testd_<version>_x64.deb` or `.AppImage` | `sudo apt install ./Testd_<version>_x64.deb` or `chmod +x` and run the `.AppImage` |
| Windows (`x64`) | `Testd_<version>_x64-setup.exe` | Run the per-user installer |

The desktop app only includes the tray/host binary (`testd-gui`); install the CLI too if you want `testd run` in your terminal. Early alpha bundles are unsigned, so macOS and Windows may ask you to confirm opening the app.

On Linux, binding port `443` needs `CAP_NET_BIND_SERVICE` on the tray binary (`sudo setcap 'cap_net_bind_service=+ep' /path/to/testd-gui`, not an AppImage wrapper) or a high `edge_addr` like `127.0.0.1:8443`. See [Privileged ports](system.md#privileged-ports).

</template>
<template #tab-1>

Run the installer script:

```sh
curl -fsSL https://testd.dev/install.sh | sh
```

The script downloads the latest release, verifies `SHA256SUMS`, and installs `testd` into `~/.local/bin` (override with `TESTD_INSTALL_DIR`). To pin a version, set `TESTD_VERSION` without the leading `v` (for example `TESTD_VERSION='0.1.0-alpha.1'`). On Linux `x86_64`, it picks GNU libc when detected and static musl otherwise; set `TESTD_LIBC=gnu` or `TESTD_LIBC=musl` to choose explicitly.

You can also download the release `.tar.gz` archive manually or install the generated Homebrew formula attached to each release (`brew install --formula ./testd.rb`). Linux `aarch64` has no prebuilt archive yet; build from source there.

</template>
<template #tab-2>

Download `testd-<version>-x86_64-pc-windows-msvc.zip` and `SHA256SUMS` from [GitHub Releases](https://github.com/stelmost/testd/releases).

Verify the checksum with `certutil -hashfile <zip> SHA256` or `Get-FileHash`, then extract `testd.exe` to a directory on your `PATH`. The Windows CLI links the MSVC runtime statically and needs no separate Visual C++ redistributable.

</template>
<template #tab-3>

Requires Rust 1.90+:

```sh
cargo install --path .
```

See [Developing](developers/index.md) and [Releasing](developers/releasing.md) for building and packaging details, including the desktop app.

</template>
</DocTabs>

## 2. Set up and start the host

Choose either the desktop app or the CLI on your browser machine (only one host daemon can run at a time):

<DocTabs :labels="['Desktop app (GUI)', 'Headless CLI']">
<template #tab-0>

1. Launch **Testd**. On first run it opens a guided setup page in your default browser.
2. Complete **DNS resolver** and **Certificate authority** (both prompt for administrator privileges) and review the generated operator secret.
3. Click **Finish setup**. testd writes your config, starts the host daemon in your tray or menu bar, and opens the dashboard.

See [Running the host](host.md#first-run-setup) for full setup and tray details.

</template>
<template #tab-1>

Initialize the host config, local CA, and system integration once:

```sh
testd init
testd doctor --fix
```

`testd init` is idempotent: it creates `config.toml` (`0600`), generates the local CA under `pki/`, writes an operator secret, detects Caddy if present, and installs root trust interactively (or with `--yes`). `testd doctor --fix` sets up both the `.test` resolver and root trust right away (requires administrator privileges).

Then start the host daemon in a terminal (or configure a system service under [Running the host](host.md#headless-cli-and-system-services)):

```sh
testd host
```

Run `testd dashboard` in another terminal to print the authenticated dashboard URL. On Linux, binding port `443` requires `CAP_NET_BIND_SERVICE` or a high `edge_addr`; see [Privileged ports](system.md#privileged-ports).

</template>
</DocTabs>

The running daemon opens these local listeners by default:

| Listener | Default | What it does |
|---|---|---|
| TCP | `127.0.0.1:7737` | Leaf intake and the local dashboard |
| TCP | `127.0.0.1:443` | Built-in HTTPS edge for `*.test` names (when `edge_enabled` is true) |
| TCP | `127.0.0.1:7740` | Plain HTTP routing (for tunnels and external fronts like Caddy) |
| UDP | `127.0.0.1:7753` | Local `.test` DNS on Linux and macOS |
| UDP | `127.0.0.1:53` | Local `.test` DNS on Windows |

## 3. Register your first name

You can publish a local project from the CLI or from the dashboard's **Quick launches** card:

```sh
testd run app.example:3000
```

Other common forms:

```sh
testd run :3000                              # uses the current folder name
testd run app.example:3000 -- npm run dev    # name lives as long as the dev server
testd run frontend:3000 api:8080 -- npm run dev
```

Open `https://app.example.test` in your browser. Leave `.test` off in the command; the host appends it automatically. Keep `testd run` running: pressing Ctrl+C or exiting the wrapped command removes the name.

**Tip on naming:** Prefer two-label names like `app.example` instead of single-label names like `app` so browsers treat `app.example.test` and `api.example.test` as sharing a registrable domain for cookies.

## 4. Verify everything is green

- **Desktop app:** the tray icon is normal (no alert badge), and the dashboard's **Certificate** card shows system trust (and Firefox trust when opened in Firefox) as green.
- **CLI:** run `testd doctor`:

```sh
testd doctor
```

Exit code `0` means configuration, DNS resolution, PKI, root trust, and the HTTPS edge are all healthy. If anything fails, see [Troubleshooting](troubleshooting.md).

## Where to go next

- [Running the host](host.md): tray menu, icon states, notifications, launch at login, the web dashboard, and headless services
- [Local apps](local.md): naming rules, targets, lifetimes, and saved quick launches
- [Remote & SSH](remote.md): SSH apps from the dashboard, or direct paired leaves for VMs, containers, WSL, and LAN boxes
- [Sharing & tunnels](sharing.md): share a live name over `cloudflared`, `ngrok`, or `tailscale`
- [System integration](system.md): privileged ports, DNS, PKI, external fronts (Caddy), and clean uninstall
- [CLI & configuration](reference.md): every command, flag, config key, file, endpoint, and limit
