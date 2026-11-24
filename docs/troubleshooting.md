---
title: Troubleshooting
description: 'Diagnose and repair testd with testd doctor, tray alerts, common fixes, and layer-by-layer checks.'
---

# Troubleshooting

## Check and repair

### From the desktop app

While the host runs, it re-checks the `.test` resolver every 10 seconds and root trust every 60 seconds. If either check fails:

- The tray icon switches to the **alert badge** and posts a desktop notification.
- Click the notification, or open the tray menu and click **Set up DNS** or **Trust certificate**, to run the privileged repair.
- For browser trust issues, open the dashboard's **Certificate** card to see system trust status (and Firefox's separate trust check when viewed in Firefox).

### From the CLI

Run `testd doctor` any time something looks off:

```sh
testd doctor
testd doctor --json
testd doctor --fix
```

`doctor` checks your config, operator secret, OS resolver route, actual `.test` DNS resolution, optional Caddy integration, PKI validity, and the HTTPS edge. (PKI is always required for registration TLS even when the built-in browser edge is disabled; OS root trust is required only when the built-in edge is enabled.)

| Exit code | Meaning |
|---|---|
| `0` | Everything is ready |
| `1` | Missing or invalid configuration or state |
| `2` | Resolver route or `.test` DNS resolution is broken |
| `3` | PKI, certificate trust, or the HTTPS edge is not ready |

`testd doctor --fix` applies privileged repairs: configures the OS resolver, writes a missing operator secret, adds the Caddyfile import and reloads Caddy if configured, and installs or reinstalls the root certificate.

## Common problems

| Problem | Likely cause and fix |
|---|---|
| Browser shows a certificate warning | Root CA is not trusted yet; run `testd doctor --fix` or click **Trust certificate** in the tray menu. In Firefox, open the dashboard to check Firefox trust and import the root if prompted. |
| `.test` name does not resolve | OS resolver is not configured or the host daemon is not running; run `testd doctor` or click **Set up DNS** in the tray menu. On Windows, a stale answer may need `ipconfig /flushdns`. |
| `404` from the edge | No live registration for that name: the leaf stopped, you clicked **Stop**, or its grant was deleted. |
| `502` from the edge | The host cannot reach the target: wrong port/address, remote server bound only to loopback without a paired leaf, or a firewall blocking it. |
| Duplicate name error | Another connection already owns the name; pass `--force` only if you want to take it over. |
| Authentication failed | Stale credential after revoking a token or resetting the host; delete `~/.config/testd/credentials/<profile>` and run again. |
| Access request expired | Nobody approved it within two minutes; run `testd run` again. |
| `port 53 already in use` on Windows | Another local DNS service owns UDP port 53; stop it so testd can bind `.test` DNS. |
| `nothing is serving HTTPS` | Start the desktop app or `testd host` and check `edge_addr`; on Linux, port 443 needs `CAP_NET_BIND_SERVICE` or a high port (see [Privileged ports](system.md#privileged-ports)). |
| `no daemon is running` | Start the desktop app or `testd host`; the dashboard, shares, requests, and tokens live in the daemon. |
| Second daemon exits immediately | Only one host daemon can run per user (either the desktop app or `testd host`, not both). |
| Share has no URL | Tunnel provider is missing, not authenticated (`ngrok`), or failed to report a URL; check the daemon logs. |

## Layer-by-layer checks

| Layer | Command or observation |
|---|---|
| Configuration present | `testd doctor`; first check is `config` |
| Operator secret set | `doctor` check `secret` |
| Resolver route | `doctor` check `resolver` |
| Name resolves through the OS | `doctor` check `resolution` |
| PKI can issue a leaf | `doctor` check `pki`, with the built-in edge enabled |
| Root trusted | `doctor` check `certificate`, with the built-in edge enabled |
| Edge listening and trusted | `doctor` check `edge` |
| Optional Caddy wiring | `doctor` checks `caddy`, `caddyfile`, `reload` |
| Live names and targets | Dashboard **Live names**, or `curl -H "Authorization: Bearer <dashboard_token>" http://127.0.0.1:7737/api/status` |
| Pending requests | Dashboard **Access**, or `testd requests` |
| Issued tokens | Dashboard **Tokens**, or `testd tokens` |
| Shares and URLs | Dashboard **Shares**, tray menu, or `testd share --list` |
