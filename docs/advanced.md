---
title: Advanced usage
description: 'Remote development, profiles, grants, quick launches, tunnels, external fronts, and PKI.'
---

# Advanced usage

## Remote development

The host intake defaults to loopback. To accept remote leaves, set `[host] listen_addr` to a reachable address. A specific non-loopback intake bind also starts a loopback listener on the same port for the dashboard and local launches. A `0.0.0.0:7737` bind includes loopback but exposes the intake on every IPv4 interface; if used, restrict port 7737 with a firewall.

Pair the host before registering from a leaf:

```sh
testd pair 192.168.1.5
```

The leaf and host independently derive a 128-bit verification code from their TLS session. Read the host's code in its dashboard Access card or `testd requests`, compare it with the leaf, approve the pairing request on the host, and enter the host's code on the leaf. Do not confirm an echoed code received from an unverified network message. Pairing requests expire after two minutes.

The leaf saves the host CA in `trusted-hosts/<endpoint-hash>.crt`, scoped to that endpoint and to testd, not installed into the OS trust store. Ordinary transport certificate renewal preserves this pin. A changed host identity fails before authentication; explicitly run `testd pair` again only after verifying the change. Re-pairing preserves the previous public pin as a fingerprinted `.previous.crt` file. Pair every endpoint selected through a node profile or `--hosts`.

Network registration uses TLS 1.3, even when `TESTD_TOKEN` supplies a credential. It never falls back to plaintext. A remote leaf with no credential for the selected profile separately asks for publishing access. On the host:

```sh
testd requests
testd approve <id>
```

The leaf saves the issued token in `~/.config/testd/credentials/<profile>` and reuses it. Later runs do not ask again unless the token is revoked or the host state is reset.

A bare application port is reached through the leaf's loopback interface. An explicit target must resolve to an address on the leaf itself; run the leaf beside the application rather than adding an unencrypted third-machine hop. Existing local-only registrations can still name direct targets.

A remote leaf may only target its own address by default. To allow a different target, set:

```toml
[host]
allow_remote_targets = true
```

This turns an issued name into a possible proxy into the host's loopback or another network. Enable it only for leaves you would already trust with that reach.

For a secure leaf, application requests use mutual TLS from the host to the leaf forwarder, then HTTP to the local application. Leaf signing keys exist only in memory for that registration connection. The host authenticates the leaf's advertised CA through the encrypted registration; the leaf accepts application connections only with a client certificate from its paired host CA. Host root and intermediate private keys never leave the host.

The leaf binds one dynamically allocated TLS listener per published entry on the interface used to reach the host. The host needs a return route and firewall permission to those listeners. Dev servers can listen on loopback; a successful registration does not establish NAT traversal. Each connection supports up to 64 application endpoints. Losing the registration or withdrawing a grant closes the affected forwarders, including open streams.

Unpaired numeric loopback and `localhost` endpoints retain the local-only protocol, with a second check that the actual peer is loopback. Pairing a loopback endpoint explicitly selects TLS. A source-rewriting SSH forward changes apparent addresses and is not a drop-in remote-development topology. Never expose port 7737 to the public internet.

If the host is rebuilt or `tokens.json` is lost, the leaf's stored credential is rejected. Remove the credential file and run again:

```sh
rm ~/.config/testd/credentials/<profile>
```

## Credentials and profiles

Two independent profiles answer two different questions:

| Profile | Question | Stored in |
|---|---|---|
| Node profile | Which host addresses to connect to | `~/.config/testd/leaf.toml` |
| Token profile | Which credential file to present | `~/.config/testd/credentials/<profile>` |

Example node profile:

```toml
default_profile = 'home'

[profiles.home]
hosts = ['192.168.1.5:7737', '192.168.1.6:7737']
```

Select it with `--node-profile home` or `TESTD_PROFILE=home`. A one-off address wins over the profile:

```sh
testd run --host 192.168.1.5 :3000
testd run --hosts 192.168.1.5:7737,192.168.1.6:7737 :3000
```

Selection order:

1. Host addresses: CLI or environment, then the selected node profile, then the host's own loopback listener. A CLI `--host` or `--hosts` entry without a port gets `:7737`; environment and `leaf.toml` addresses must include a port.
2. Token profile: `TESTD_TOKEN_PROFILE`, then the CLI option, then `token_profile` in `.testd.toml`, then `default`.
3. Credential: `TESTD_TOKEN`, then the credential file for the token profile, then the host config's operator secret when the default profile is selected and every node address is loopback.

The operator secret fallback exists so a local run on the host works without an approval. It is never sent to another machine automatically. If you put it in `TESTD_TOKEN` on a leaf, you are deliberately copying it there.

Use a project file to give one project its own token profile:

```toml
# .testd.toml
token_profile = 'myproject'
```

The file contains no secret and is safe to commit. Separate token profiles give separate grants and separate revocation. If every project uses `default`, they all share one token and one grant list.

Several addresses are useful when they reach the same host. A token issued by one host does not authenticate against a different host. Use the operator secret across hosts only when you accept that tradeoff.

For CI, obtain a token once interactively, then store `~/.config/testd/credentials/<profile>` in the CI secret store. The operator secret is broader and cannot be revoked individually. Do not commit either.

## Grants and lifecycle

A token is durable permission. A grant is the token's list of exact names. A claim is a live registration on a connection. The connection is the lease. A token is the key, its grants are the doors it may open, and a claim is a door currently held open.

| Event | Token | Grant | Claim | Leaf |
|---|---|---|---|---|
| Ctrl+C or process exit | Kept | Kept | Released | Exits |
| Dashboard Stop | Kept | Kept | Released | Stops; next run registers again |
| Dashboard Delete on a name | Kept | That name removed | Released | Stops that name; asks again next time |
| Token revoke | Deleted | All names gone | Released | Connection closes, credential forgotten |
| Network drop | Kept | Kept | Released when noticed | Reconnects and re-registers |
| Host reset | Gone | Gone | Gone | Stored credential rejected; remove it and run again |

A name outside a token's grant does not fail the registration outright. The host queues a request for that one name and holds the registration until the operator decides. Approving adds the name to the existing token. Deleting one name removes only that name. If a revoked token was supplied through `TESTD_TOKEN`, unset or replace the variable before running again; the leaf cannot clear it for you.

Quick launches and the operator secret are not issued grants. Names published that way have no per-leaf grant to delete, so the dashboard offers only Stop for them.

## Quick launches

A quick launch is a saved command started from the local dashboard or the tray. It is local-only: the dashboard serves it on loopback and the authenticated intake never exposes process control. The tray lists saved launches with start and stop actions.

A launch definition has a label, a command, an optional working directory, an optional environment, and one or more name entries. The dashboard asks for the full project directory so commands run there rather than in the tray's or daemon's starting directory. For each entry:

- An empty target, or `auto`, detects the child's listening port.
- A port or `host:port` overrides detection.
- The name moves through `configured`, `probing`, and `confirmed`; it is registered only after the target accepts.

Running state is not persisted. Definitions live in `~/.config/testd/launches.json`; logs are capped in memory. Stop ends the process group. Quitting the app stops every launch.

## Tunnels in depth

`testd share` asks the running daemon to start a provider. The provider is spawned from `PATH`, never linked or called through an SDK.

| Provider | Exposure | Target | URL source |
|---|---|---|---|
| `cloudflared` | Public internet | Any reachable target | Provider output |
| `ngrok` | Public internet | Any reachable target | Provider output |
| `tailscale` | Tailnet | The plain edge listener | `tailscale status --json` |

`cloudflared` uses a quick tunnel and needs no account. `ngrok` needs `ngrok config add-authtoken` once.

Secure cloudflared and ngrok shares route through the local edge, which retains the leaf TLS identity and serves the provider URL as a claim-bound alias. Providers do not receive host signing keys, and requests keep the provider's original Host header.

`tailscale serve --bg` cannot proxy a container or LAN address directly. `testd` points it at the host's plain edge listener and registers the tailnet name as an alias there, so the edge proxies to the real target. This needs the plain HTTP routing listener, which runs with or without the built-in TLS edge. If `tailscale serve` already has configuration on port 443, `testd` refuses to overwrite it.

A `serve --bg` entry survives a daemon restart while the alias does not. On startup, `testd` clears any serve entry whose root handler points at its own edge and leaves every other entry alone.

Overrides:

| Variable | Meaning |
|---|---|
| `TESTD_TUNNEL_PROVIDER` | Default provider when `--provider` is omitted |
| `TESTD_TUNNEL` | Path to a provider binary outside `PATH` |

One share per registered name. A share ends when its owner or target changes, including a reconnect to the same target. Repeating the same owner's unchanged registration keeps it. A failed change keeps the share only when the original claim is restored without an intervening successful change. Stopping a share leaves the name registered; removing the name closes the share. Tailscale serve permits one share at a time. If provider cleanup cannot be verified, testd reports the error and refuses a replacement for the rest of that daemon run; an unverified Tailscale cleanup blocks other Tailscale shares too. Inspect provider state before restarting; a restart does not prove cleanup.

A public provider makes the app reachable by anyone with the URL. The provider terminates TLS, so the browser trusts the provider's certificate, not the testd root. Share only what you intend to expose.

## External front

The built-in edge is the default and needs no Caddy. An external front is optional for operators who already run one.

Relevant host settings:

```toml
[host]
# The HTTPS edge. Move it off 443 when another process owns that port.
edge_addr = '127.0.0.1:8443'
# Plain HTTP routing for an external front.
http_addr = '127.0.0.1:7740'
# Only enable when a configured front sets X-Forwarded-For.
trust_forwarded_for = false
```

The plain listener always runs. `edge_enabled` controls only the built-in TLS listener. Set `edge_enabled = false` when an external front owns 443; keep it true and move `edge_addr` if both the built-in edge and the front run. If the front runs in a container or another network namespace, bind `http_addr` to an address it can reach.

`testd init` detects a Caddy install and offers to add an import to the global Caddyfile. `testd export-caddyfile` writes the managed Caddyfile and prints the import line. `testd doctor` checks the import and reloads Caddy when a global config is configured; `--fix` adds a missing import.

The generated Caddyfile terminates TLS with Caddy's internal CA and proxies to `http_addr`. Browsers trust that CA, not the testd root. With `edge_enabled = false`, `doctor` still checks the PKI needed for registration TLS, but does not require the testd browser root to be installed; it requires the external front's certificate to be trusted by the system. `trust_forwarded_for` must stay off unless a front you control sets the header; otherwise a client can spoof its source address. On the plain listener the dashboard name is served only with this on and a loopback first hop.

## DNS resolution

The resolver is a built-in DNS server on loopback. On Linux, setup uses a compatible existing global route without adopting it and reports conflicting global settings or per-link routes instead of changing them.

## Privileged ports

On Linux, binding 443 needs `CAP_NET_BIND_SERVICE`, root, or a high port.

- System service: `packaging/systemd/testd@.service` runs as a named user with only that capability.
- Foreground executable: `sudo setcap 'cap_net_bind_service=+ep' "$(readlink -f "$(command -v testd)")"`. Re-apply after replacing the binary.
- High port: set `[host] edge_addr = '127.0.0.1:8443'` and open `https://app.example.test:8443`.
- User service: `packaging/systemd/testd.service` sets `NoNewPrivileges=yes`, so it cannot acquire a file capability. Use a high port there.

macOS uses the launchd template in `packaging/launchd/`. Windows needs no capability for the default configuration beyond the usual firewall prompt.

## Desktop app and OS integration

The tray app is a separate Tauri build. On first start it opens a setup dialog served by the daemon on an ephemeral loopback port: resolver, root certificate, and operator secret. The dialog generates a pronounceable secret into an editable field, so an existing secret can replace it. Setup talks HTTP, not IPC, and shares the dashboard's stylesheet.

The tray:

- Runs the daemon and opens the dashboard in a window
- Enables launch at login once, on first start, with a **Launch at login** checkbox afterwards
- Shows pending access requests as notifications and changes its icon while one waits
- Lists saved quick launches with start and stop actions
- Provides share controls

Unticking **Launch at login** is final until you tick it again, even if the login item is removed outside the tray. Removing the app with the OS uninstaller does not remove the DNS route or the trusted root. On Linux, run `testd uninstall` first; macOS and Windows uninstall is blocked until system ownership can be verified.

Notifications and OS URL-scheme launches share one action grammar:

```text
testd://fix/resolver
testd://fix/certificate
```

`testd url <url>` forwards the action to a running daemon. If the daemon is gone, it performs the repair itself, using a native authorization dialog when there is no terminal. Release builds register the `testd` scheme on Windows and Linux at startup and through the bundle on macOS. Dev builds skip registration unless `TESTD_REGISTER_SCHEME=1` is set.

## Certificates and PKI

- The root CA lives 10 years and is the trust anchor installed in the system store.
- The host intermediate lives one year and renews near expiry on startup or certificate minting, without changing root trust.
- Leaf certificates live 24 hours and are minted on demand for a name.
- The previous intermediate certificate and key are kept under `pki/backups/`.

`testd doctor` reports expiry. `testd doctor --fix` repairs the PKI when the root key is available. If `root.key` was moved offline, restore it before the intermediate expires.

The dashboard's Certificate card shows trust state, SHA-256 fingerprint, and expiry, and installs or removes system trust. Fetch the root from either dashboard URL:

```sh
curl http://127.0.0.1:7737/ca.crt -o testd-root.crt
```

The root key stays on the host and is never copied to a dev environment. If it is compromised, rebuild the PKI, reinstall trust on every browser machine, and restart the host.

## Automation

A non-interactive run needs a credential that does not wait for approval:

```sh
export TESTD_TOKEN="$(cat /run/secrets/testd-token)"
testd run --host 192.168.1.5 --token-profile ci app.example:3000 -- ./server
```

The token is an issued token from the CI profile or the operator secret. Prefer the issued token because it is scoped and revocable. A credential file also works when the environment cannot carry the variable.

`testd run` exits with the child's status; a registration failure stops the child and exits non-zero. `testd doctor --json` provides stable exit codes and check objects for scripts.

Do not put secrets in `.testd.toml`, command lines, or logs. `TESTD_TOKEN` is visible to the same user's process inspection on most systems.

## Design notes

- One host is the authority. DNS, TLS, the registry, and approvals live on the browser machine. There is no central service, account system, cross-host mesh, or public DNS dependency.
- The connection is the lease. A claim lives exactly as long as the TCP connection that created it, so a crash or lost network releases the name without a cleanup job.
- The token is durable permission. Re-approving a leaf on every run would be annoying, so tokens and grants persist across reconnects and can be revoked later.
- Grants are per name. A token covers exact names, not a project wildcard or everything a machine can reach.
- Network registration is NDJSON over pinned TLS 1.3; application forwarding to a secure leaf uses mutual TLS. Local-only registrations retain the loopback protocol.
- The CA is local. A root installed once per browser machine plus short-lived leaf certificates gives private HTTPS without per-project certificate files.

## Clean up

Stop the daemon first, then remove system integration:

```sh
testd uninstall
testd uninstall --purge
```

On Linux, `uninstall` removes owned resolver and trust integration and retains local data. `--purge` also deletes the configuration directory, including its default PKI and tokens, but only after system cleanup succeeds. A custom PKI outside that directory is not deleted. Unowned or changed system state is left untouched and conflicts are reported. If the local root certificate is missing, trust removal and purge are incomplete; restore the certificate before retrying.

Uninstall is blocked on macOS and Windows until resolver and trust ownership can be verified. It does not remove the executable or desktop app.
