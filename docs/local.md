---
title: Local apps
description: 'Register local projects with testd run, naming rules, lifetimes, and saved quick launches.'
---

# Local apps

Once your host daemon is running (via the [desktop app](host.md#desktop-tray-app) or `testd host`), you can publish local projects either from a terminal with `testd run` or from the dashboard and tray with **Quick launches**.

## Register a name (`testd run`)

The most common command is:

```sh
testd run app.example:3000
```

All ways to specify what to register:

| Entry | What it registers |
|---|---|
| `:3000` | Uses the current directory name; targets port `3000` on this machine |
| `app.example:3000` | Registers `app.example.test`; targets port `3000` on this machine |
| `10.0.0.5:app.example:3000` | Registers `app.example.test`; targets `10.0.0.5:3000` explicitly |
| `app.example.test:3000` | Same as `app.example:3000` (a trailing `.test` is stripped) |

You can publish multiple services in one command:

```sh
testd run frontend:3000 api:8080
```

All entries in one `testd run` share a single connection, token, and grant list.

To tie the names to your dev server process, add `--` followed by the command:

```sh
testd run app.example:3000 -- npm run dev
```

The names stay live while the command runs and disappear as soon as it exits or you press Ctrl+C. `testd run` exits with your command's exit code.

**How bare ports are resolved:**

- On a **local** run, `:3000` targets `127.0.0.1:3000`.
- On a **paired remote leaf**, `:3000` targets `127.0.0.1:3000` on the leaf itself (the leaf forwards traffic from the host over mutual TLS to its own loopback).
- If you pass an explicit target host on a local-only registration, the host must be able to reach that address directly.

Local runs on loopback using the default token profile automatically authenticate with the host's operator secret, so no approval prompt is needed.

## Names and targets

Write names as `<service>.<project>` without the `.test` suffix. The daemon stores and displays the bare name (for example `app.example`), and browsers open `https://app.example.test`.

**Why two labels matter:** `app.example.test` and `api.example.test` share a registrable domain, so cookies and SameSite rules behave just like `app.example.com` and `api.example.com`. A single-label name like `app.test` is treated as cross-site with every other project, so `testd` prints a warning when you use one.

Rules to keep in mind:

- Labels may contain ASCII letters, digits, and hyphens, and cannot start or end with a hyphen.
- Each label can be up to 63 bytes, and the full name up to 253 bytes.
- `testd.test` is reserved for the local dashboard (reachable only from this machine).
- Targets can be a bare port, `host:port`, `ip:port`, or `[ipv6]:port` (port `0` is rejected).

**What your app receives:** DNS always points registered names at the host, not directly at the app. The built-in edge speaks HTTP/1.1 and supports WebSocket upgrades (so HMR works out of the box), forwarding the original `Host` header along with `X-Forwarded-For` and `X-Forwarded-Proto`.

## Lifetimes and ownership

Think of the TCP connection as the lease:

- Press Ctrl+C, stop the wrapped process, or lose the network, and the host releases those names automatically.
- If the network drops, a running leaf reconnects every few seconds and re-registers its names without needing approval again (as long as its token and grants still exist).

If another connection already owns a name and you intentionally want to take it over, pass `--force`:

```sh
testd run --force app.example:3000
```

Without `--force`, a duplicate name fails and reports the current owner's connection id.

Different actions in the dashboard and CLI affect tokens, grants, and live names differently (see [Grants and lifecycle](remote.md#grants-and-lifecycle) for the full table):

| Action | What happens |
|---|---|
| **Stop** (dashboard) | Ends the leaf connection now, but keeps its token and grants. The next run works immediately without re-approval. |
| **Delete** (dashboard) | Stops that one name and removes it from the token's grant list. Other names on the leaf keep running; publishing that name again will ask for approval. |
| **Revoke token** (dashboard or `testd tokens --revoke <id>`) | Deletes the token, closes every connection using it, releases all of its names, and tells the leaf to forget its saved credential. |

Names published by quick launches or with the operator secret have no per-leaf grant to delete, so the dashboard offers only **Stop** for them.

## Quick launches

A quick launch is a saved local command that you start and stop from the dashboard or the desktop tray menu without keeping a terminal open.

Each definition includes a label, command, optional working directory (give the full project path so commands do not run in the daemon's starting directory), optional environment variables, and one or more name entries:

- Leaving a target empty (or setting `auto`) automatically detects the port your child process listens on.
- Providing a port or `host:port` skips auto-detection.
- Each entry progresses from `configured` → `probing` → `confirmed`, and the `.test` name is registered only once the target accepts connections.

Quick launches run without a console window on Windows. Expand **Output** under a launch in the dashboard to view its live stdout and stderr, labeled by stream. The dashboard keeps the latest 500 lines from the current or most recent run, including after the command stops; starting the launch again or restarting the daemon clears that output.

Definitions are saved in `~/.config/testd/launches.json`. Running state is not persisted across restarts. Clicking **Stop** ends the process group, and quitting the host daemon stops all running launches.

## Common workflows

| Goal | What to run or click |
|---|---|
| Move an app to another port | `testd run app.example:4000` |
| Run frontend and API together | `testd run frontend:3000 api:8080 -- npm run dev` |
| Restart a dev server | Restart the command; the name comes back automatically |
| Keep a name even if a server restarts | Run `testd run ...` without `--`, or use a dashboard **Quick launch** |
| Take over a stale or duplicate name | `testd run --force ...` |
| Pause a name temporarily | Dashboard **Stop** (next run registers without re-approval) |
| Remove permission for one name | Dashboard **Delete** on that name |
| Match production cookie scoping | Use `<service>.<project>` instead of a single label |
| Avoid port 443 | Set `[host] edge_addr` to a high port (for example `127.0.0.1:8443`) and include `:8443` in URLs |
