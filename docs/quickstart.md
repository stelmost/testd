---
title: Quickstart
description: 'From a clean machine to a working HTTPS name: install, first host, first registration.'
---

# Quickstart

This guide takes you from a clean machine to opening `https://app.example.test` in your browser.

Your **host** is the machine with the browser. Your project can run on that same machine, or on another machine (a VM, container, WSL distro, or remote dev box).

```text
browser
  │  DNS: app.example.test → host
  ▼
host machine (testd host)
  ├─ local DNS for *.test
  ├─ HTTPS edge on :443
  └─ reverse proxy → your app
                         ▲
                         │  one outbound connection
leaf (local machine, VM, container, WSL, or remote box)
  └─ testd run
```

The browser only ever talks to the host. The host terminates HTTPS and forwards traffic to the app. For remote leaves, traffic between the host and the leaf uses mutual TLS, while the last hop from the leaf to your dev server stays plain local HTTP.

## 1. Install testd

**Option A: Download a release**

Grab CLI archives, desktop tray bundles, or Homebrew definitions from [GitHub Releases](https://github.com/stelmost/testd/releases), and verify `SHA256SUMS` before installing.

On Linux and macOS, you can also run the version-pinned CLI installer command shown in the chosen release's notes (`TESTD_VERSION` is the tag without the leading `v`; omitting it installs only a stable release, not prereleases).

- Linux `x86_64` CLI archives come in `x86_64-unknown-linux-gnu` (GNU libc) and `x86_64-unknown-linux-musl` (static musl). The installer picks GNU libc when detected and musl otherwise; set `TESTD_LIBC=gnu` or `TESTD_LIBC=musl` to choose explicitly.
- Linux desktop packages use GNU libc. Debian 13 is the native validation baseline; other distributions are best effort.
- The native tray app is a separate Rust build; its setup flow and dashboard open in your default browser.

**Option B: Build from source**

```sh
cargo install --path .
```

See [DEVELOPING.md](https://github.com/stelmost/testd/blob/master/DEVELOPING.md#release) for building and packaging details.

## 2. Set up the host (once)

Run these two commands on the machine with your browser:

```sh
testd init
testd doctor --fix
```

`testd init` is safe to run again at any time. It:

1. Creates `~/.config/testd/config.toml` (mode `0600`, or `%APPDATA%\testd\config.toml` on Windows).
2. Generates a local root CA and host intermediate certificate under `~/.config/testd/pki/`.
3. Writes an operator secret to `[host] secret`.
4. Detects an existing Caddy installation and offers to wire up the managed Caddyfile.
5. Installs the root certificate into your system trust store when running interactively (or with `--yes`).

By default, `testd init` waits to configure the OS `.test` DNS resolver until the first remote client connects. Running `testd doctor --fix` sets up both the `.test` resolver and root certificate trust right away (this requires administrator privileges).

## 3. Start the host daemon

```sh
testd host
```

Leave this running in a terminal (or run the desktop tray app / system service). The daemon opens these local listeners:

| Listener | Default | What it does |
|---|---|---|
| TCP | `127.0.0.1:7737` | Leaf intake and the local dashboard |
| TCP | `127.0.0.1:443` | Built-in HTTPS edge for `*.test` names (when `edge_enabled` is true) |
| TCP | `127.0.0.1:7740` | Plain HTTP routing (for tunnels and external fronts like Caddy) |
| UDP | `127.0.0.1:7753` | Local `.test` DNS on Linux and macOS |
| UDP | `127.0.0.1:53` | Local `.test` DNS on Windows |

Only one daemon can run at a time; a second instance exits immediately on the instance lock. Run `testd dashboard` in another terminal to print the authenticated dashboard URL.

**Linux port 443 note:** Binding port 443 on Linux needs `CAP_NET_BIND_SERVICE`. You can use the root-managed `packaging/systemd/testd@.service`, grant the capability to the `testd` binary yourself, or pick a high port like `127.0.0.1:8443` and include it in browser URLs. See [Privileged ports](advanced.md#privileged-ports).

## 4. Register your first name

In your project directory, point testd at the port your dev server listens on:

```sh
testd run app.example:3000
```

You can also let testd infer the name from the folder, wrap your dev command, or register multiple services at once:

```sh
testd run :3000                              # uses the current folder name
testd run app.example:3000 -- npm run dev    # name lives as long as the dev server
testd run frontend:3000 api:8080 -- npm run dev
```

Now open `https://app.example.test` in your browser. Notice you leave `.test` off in the command; the host appends it automatically. Keep `testd run` running: pressing Ctrl+C, exiting the wrapped command, or losing the connection removes the name.

**Tip on naming:** Prefer two-label names like `app.example` instead of single-label names like `app`. Two labels give browsers a real registrable domain so cookies behave like `app.example.com` and `api.example.com`. `testd` warns when you use a single-label name.

## 5. Verify everything is green

```sh
testd doctor
```

`doctor` checks configuration, DNS resolution, PKI, root trust, and the HTTPS edge:

- `0`: Everything is ready.
- `1`: Missing or invalid configuration or state.
- `2`: DNS resolver route or name resolution problem.
- `3`: PKI, certificate trust, or HTTPS edge problem.

Add `--json` for scripts, or `--fix` to repair the resolver, Caddyfile import, and root trust.

## Connecting a remote dev box

You have two ways to reach a dev server on another machine:

- **Over SSH (easiest):** Open the dashboard, go to **SSH apps**, enter your SSH host alias, remote port, and local `.test` name, then click **Open**. No network ports need to be opened on your host. See [SSH apps](advanced.md#ssh-apps).
- **Direct LAN / VM / WSL leaf:** Point a remote `testd` directly at your host over TLS 1.3.

For a direct remote leaf:

1. On the host, set `[host] listen_addr` in `config.toml` to an address the remote machine can reach (by default it only listens on `127.0.0.1`), and restrict port `7737` with your firewall.
2. On the remote machine, pair once with the host, then run:

```sh
testd pair <host-lan-ip>
testd run --host <host-lan-ip> :3000
```

During `testd pair`, both sides display a verification code. Check that they match in the host's dashboard **Access** card or via `testd requests`, approve pairing on the host, and enter the host's code on the leaf. Pairing saves host trust only; it does not grant publishing access yet.

Next, `testd run` asks for permission to publish its names. Approve that request on the host:

```sh
testd requests
testd approve <id>
```

The leaf saves its issued token and reuses it on future runs. Your app can keep listening on `127.0.0.1` on the remote machine; the leaf forwards traffic to it over local HTTP after authenticating the host over mutual TLS.

The host does need a direct route to the leaf's dynamically allocated TLS ports (pairing does not traverse NAT). Never expose port `7737` to the public internet. See [Remote development](advanced.md#remote-development) and [Security](security.md).

## Where to go next

- [Daily usage](usage.md): naming rules, the dashboard, SSH apps, sharing with tunnels, access requests, and troubleshooting
- [Advanced usage](advanced.md): remote leaves, profiles, quick launches, tunnels, external fronts, and system services
- [Reference](reference.md): every command, option, config key, file, HTTP endpoint, and limit
- [Security](security.md): trust boundaries, credentials, exposure, and hardening
