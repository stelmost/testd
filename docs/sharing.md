---
title: Sharing & tunnels
description: 'Expose a live .test name over cloudflared, ngrok, or tailscale from the dashboard, tray, or CLI.'
---

# Sharing & tunnels

`.test` names only resolve on your host machine. When you need to test on a phone, receive a webhook, or show a colleague, share any live name through a tunnel.

## Start and stop a share

- **Dashboard:** open the **Shares** card, pick a live name and provider, and click **Share** (or **Stop** to close an active share).
- **Desktop tray:** active shares appear in the tray menu with a link to open the shared URL and **Stop sharing \<name\>**.
- **CLI:**

```sh
testd share app.example
testd share app.example --provider tailscale
testd share --list
testd share --stop app.example
```

## Providers

The running daemon spawns the provider binary directly from `PATH` (no vendor SDKs or linked libraries):

| Provider | Exposure | Where the URL comes from | What you need installed |
|---|---|---|---|
| `cloudflared` (default) | Public internet | Provider output (`https://*.trycloudflare.com`) | `cloudflared` on `PATH` (quick tunnel; no account needed) |
| `ngrok` | Public internet | Provider output (`https://*.ngrok-free.app`, `*.ngrok-free.dev`, `*.ngrok.io`, `*.ngrok.app`) | `ngrok` on `PATH` (run `ngrok config add-authtoken` once) |
| `tailscale` | Your tailnet | `tailscale status --json` (`https://*.ts.net`) | `tailscale` on `PATH` (one Tailscale share at a time) |

Override defaults with environment variables:

| Variable | Meaning |
|---|---|
| `TESTD_TUNNEL_PROVIDER` | Default provider when `--provider` is omitted |
| `TESTD_TUNNEL` | Explicit path to a provider binary outside `PATH` |

## Routing and lifecycle

- **Plain HTTP listener:** every provider points at the daemon's plain HTTP listener (`http_addr`), which runs whether or not built-in TLS is enabled.
- **Claim-bound alias:** the provider hostname becomes an alias for the current claim. The edge preserves the incoming `Host` header, uses `trust_forwarded_for` for the scheme, and still authenticates remote leaves over mutual TLS (providers never receive host signing keys).
- **Owned by the daemon:** closing the terminal where you ran `testd share` does not drop the share.
- **One share per name:** stopping a share leaves the `.test` name live; removing the name closes the share.
- **Target or owner changes:** changing a name's owner or target (including reconnecting to the same target) ends the share, so share again afterward. Repeating an unchanged registration on the same connection keeps it.
- **Restarts and crashes:** share aliases are not restored after a daemon restart. After a crash, a provider process or `tailscale serve --bg` config may still be active; inspect provider state manually rather than assuming an empty share list means everything stopped.
- **Tailscale safety:** on startup, testd warns about matching Tailscale routes without deleting them, and refuses to overwrite an existing `:443` Tailscale serve rule. If provider cleanup cannot be verified, testd reports the error and blocks replacement until resolved.

## Security reminder

Because the provider terminates TLS, visitors trust the provider's certificate rather than the testd root. A public share (`cloudflared` or `ngrok`) exposes your local app to anyone with the URL, and a Tailscale share is as private as your tailnet. Share only what you intend to expose.
