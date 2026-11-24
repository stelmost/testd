---
title: Desktop app
description: 'Install, set up, and use the testd desktop tray app on macOS, Linux, and Windows.'
---

# Desktop app

The testd desktop app runs the host daemon from your system tray or menu bar and opens first-run setup and the dashboard in your default browser. It uses the same configuration, local CA, tokens, quick launches, and SSH apps as the `testd` CLI, so you can mix the desktop app on your host with the CLI in terminals or remote dev boxes.

## Install

Download the package for your platform from [GitHub Releases](https://github.com/stelmost/testd/releases) and verify `SHA256SUMS` before installing:

| Platform | Package | Notes |
|---|---|---|
| macOS | `Testd_<version>_<arch>.dmg` or Homebrew cask `testd-gui` | Drag `Testd.app` into Applications. Requires macOS 13 (Ventura) or newer; `aarch64` is the primary target. |
| Linux | `.deb` or `.AppImage` | Built for `x86_64` GNU libc (Debian 13 baseline; other distributions are best effort). |
| Windows | `.exe` per-user installer | Installs for the current user without requiring administrator rights until setup configures DNS and root trust. |

On macOS and Windows, early alpha releases are unsigned; your OS may ask you to confirm opening the app.

**Linux port 443 note:** Binding port `443` on Linux needs `CAP_NET_BIND_SERVICE` on the actual tray binary (not an AppImage wrapper):

```sh
sudo setcap 'cap_net_bind_service=+ep' /path/to/testd-gui
```

Alternatively, set `[host] edge_addr = '127.0.0.1:8443'` in `~/.config/testd/config.toml` and include `:8443` in browser URLs. See [Privileged ports](advanced.md#privileged-ports).

## First-run setup

When you launch the desktop app without an existing host configuration, it starts a temporary setup server on an ephemeral loopback port and opens the setup page in your default browser.

The setup page walks through three steps:

1. **DNS resolver**: configures the OS `.test` resolver route so `*.test` names reach testd's local DNS server. Requires administrator privileges.
2. **Certificate authority**: creates the local root CA and installs it in your system trust store. Requires administrator privileges. If you open the page in Firefox, it also notes that the dashboard checks Firefox trust separately after setup.
3. **Operator secret**: pre-filled with a generated pronounceable secret (at least 16 characters). You can copy it, click **Generate** for a new one, or edit it to match a secret your unattended clients already use.

Once both system checks are green and the secret is at least 16 characters, click **Finish setup**. testd writes your configuration, starts the host daemon, and redirects that same browser tab to the dashboard.

If you close the setup tab before finishing, the tray stays running with **Continue setup** (which reopens the page with your pending secret preserved) and **Exit** (which stops the setup server).

Only one host daemon can run at a time. Launching a second copy of the desktop app (or running `testd host` while the desktop app is running) will not start a second daemon.

## The tray menu

Once the daemon is running, click the tray or menu-bar icon to see live state and common actions:

- **Repair actions** (shown only when a check fails): **Set up DNS** and **Trust certificate** trigger the same privileged repairs as `testd doctor --fix`.
- **Active domains**: lists every currently registered `.test` name; clicking one opens it in your browser.
- **`testd.test → dashboard`**: opens the authenticated local dashboard in your default browser.
- **Quick launches**: lists saved local commands with **Start** and **Stop** actions.
- **SSH apps**: lists saved remote SSH apps with **Start** and **Stop** actions.
- **Shares**: lists active tunnels with a link to open the shared URL and **Stop sharing \<name\>**.
- **Launch at login**: toggles starting the desktop app when you sign in.
- **Exit**: stops the daemon, any running quick launches, SSH apps, and shares started by the daemon, and closes the tray app.

The tray icon reflects host status at a glance:

| Icon state | Meaning |
|---|---|
| Normal | Resolver and root trust are healthy, and no access requests are waiting |
| Request badge | A remote leaf is waiting for pairing or publishing approval |
| Alert badge | The `.test` resolver or root certificate trust needs repair |

When a health check fails or a remote leaf asks for access, testd also raises a desktop notification. Clicking a repair notification triggers the fix via the `testd://` URL handler (`testd://fix/resolver` or `testd://fix/certificate`).

## Using the dashboard

The desktop app does not bundle its own webview; clicking **`testd.test → dashboard`** opens the dashboard in your default HTTP browser using an owner-private HTML handoff file so admin tokens never appear in process arguments.

From the dashboard you can manage everything without touching a terminal:

- **Live names**: open, **Stop** (ends the connection, keeps the grant), or **Delete** (stops the name and removes its grant).
- **Quick launches**: save a project folder, command, and `.test` names, then start, stop, or inspect logs.
- **SSH apps**: forward a remote port or start a remote dev command over OpenSSH (`~/.ssh/config`, keys, passphrases, 2FA). See [SSH apps in the dashboard](usage.md#ssh-apps-in-the-dashboard).
- **Shares**: expose a live `.test` name over `cloudflared`, `ngrok`, or `tailscale` and stop active shares.
- **Access and tokens**: approve or deny remote pairing and publishing requests, and revoke issued leaf tokens.
- **Certificate**: view trust status, SHA-256 fingerprint, and expiry; install or remove system trust; and check Firefox trust when viewing the dashboard in Firefox.

## Launch at login

**Launch at login** is off by default on a fresh install and no OS login entry is created until you tick it.

| Platform | Login entry location |
|---|---|
| macOS | `~/Library/LaunchAgents/dev.testd.host.plist` |
| Linux | `~/.config/autostart/dev.testd.host.desktop` |
| Windows | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (`dev.testd.host`) |

Unticking **Launch at login** or removing the OS entry externally keeps it off on future restarts. If a customized or conflicting login entry already exists at that path, testd leaves it untouched rather than overwriting it.

## URL scheme and OS integration

Release builds register the `testd://` URL scheme so desktop notifications and links can route repair actions to the running app:

```text
testd://fix/resolver
testd://fix/certificate
```

- On macOS, Launch Services reads the scheme from `Testd.app`'s `Info.plist`.
- On Linux and Windows, the desktop app registers the per-user handler on startup (leaving any conflicting custom handler untouched). Dev builds skip registration unless `TESTD_REGISTER_SCHEME=1` is set.
- You can also invoke the same actions from the CLI with `testd url <url>`.

## Mixing the desktop app and the CLI

The desktop app and the `testd` CLI share the same config directory (`~/.config/testd/` on Linux/macOS, `%APPDATA%\testd\` on Windows):

- Run the desktop app on your browser machine instead of `testd host`, then run `testd run app.example:3000` in any local terminal or remote dev environment.
- Changes made in the CLI (`testd requests`, `testd tokens`, `testd share`, `testd doctor --fix`) appear immediately in the tray and dashboard, and vice versa.
- Do not run `testd host` or a system service at the same time as the desktop app; the instance lock allows only one daemon per user.

## Uninstall

Removing the desktop app package with your OS uninstaller does not remove the `.test` DNS route or trusted root certificate. To clean up completely:

1. Stop any running quick launches and shares, and untick **Launch at login**.
2. Click **Exit** in the tray menu.
3. Run `testd uninstall` (or `testd uninstall --purge` to also delete your config directory and local CA). See [Clean up](advanced.md#clean-up).
4. Remove the desktop app package (drag `Testd.app` to Trash, remove the `.deb`/`.AppImage`, or run the Windows uninstaller).
