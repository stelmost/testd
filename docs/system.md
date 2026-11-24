---
title: System integration
description: 'Privileged ports, DNS resolution, certificates and PKI, external fronts like Caddy, automation, and clean uninstall.'
---

# System integration

This page covers how testd integrates with your OS and network stack: port `443`, `.test` DNS, local PKI, external reverse proxies, CI automation, and clean removal.

## Privileged ports

- **Linux:** binding port `443` requires `CAP_NET_BIND_SERVICE`, root, or a high port:
  - System service (recommended for headless CLI): `packaging/systemd/testd@.service` runs as your user with only `CAP_NET_BIND_SERVICE`.
  - Foreground CLI: `sudo setcap 'cap_net_bind_service=+ep' "$(readlink -f "$(command -v testd)")"` (re-run after replacing the binary).
  - Desktop app: `sudo setcap 'cap_net_bind_service=+ep' /path/to/testd-gui` on the actual tray binary (not an AppImage wrapper).
  - High port: set `[host] edge_addr = '127.0.0.1:8443'` in `~/.config/testd/config.toml` and open `https://app.example.test:8443`.
  - User service: `packaging/systemd/testd.service` sets `NoNewPrivileges=yes`, so it cannot use file capabilities; pair it with a high port.
- **macOS:** if binding `127.0.0.1:443` is denied, testd asks the logged-in user's `launchd` GUI session for that exact loopback socket, then unloads the temporary socket-transfer job before startup finishes. Both the daemon and tray stay unprivileged. A logged-in desktop session is required for this fallback; headless environments can use a high port or the launchd template in `packaging/launchd/dev.testd.host.plist`.
- **Windows:** no extra capability is needed beyond approving the normal firewall prompt.

## DNS resolution

testd runs a built-in DNS server on loopback (`127.0.0.1:7753` on Linux/macOS, `127.0.0.1:53` on Windows).

- On **Linux**, setup reuses a compatible existing global `.test` route without adopting it, and reports conflicting global settings or per-link routes rather than overwriting them.
- On **macOS**, setup writes a `.test` resolver file pointing to `127.0.0.1:7753`.
- On **Windows**, setup configures an NRPT rule routing `.test` to `127.0.0.1:53`.

## Certificates and PKI

testd manages a three-tier local PKI under `~/.config/testd/pki/` (or `%APPDATA%\testd\pki\` on Windows):

- **Root CA** (`root.crt` / `root.key`): 10-year lifetime; installed once in your system trust store.
- **Host intermediate** (`node.crt` / `node.key`): 1-year lifetime; renews automatically near expiry on startup or certificate minting without changing root trust. Previous intermediates are backed up in `pki/backups/`.
- **Leaf certificates**: 24-hour lifetime; minted on demand in memory for each live name.

`testd doctor` reports upcoming expiry and `testd doctor --fix` repairs the PKI if `root.key` is present. (If you moved `root.key` offline, bring it back before the intermediate expires.)

The dashboard's **Certificate** card shows trust status, SHA-256 fingerprint, expiry, and install/remove buttons (plus a separate Firefox trust check when viewed in Firefox). Anyone on the host can download the public root certificate with:

```sh
curl http://127.0.0.1:7737/ca.crt -o testd-root.crt
```

Keep `pki/root.key` strictly on the host. If it is ever exposed, rebuild the PKI, reinstall trust on every browser machine, and restart the host.

## External front

The built-in HTTPS edge works out of the box and does not require Caddy or nginx. If you already run an external reverse proxy on port `443`, you can wire testd behind it:

```toml
[host]
# Move the built-in HTTPS edge off 443, or disable it with edge_enabled = false.
edge_addr = '127.0.0.1:8443'
# Plain HTTP routing listener used by external fronts (always runs).
http_addr = '127.0.0.1:7740'
# Only enable when a front you control sets and strips X-Forwarded-For.
trust_forwarded_for = false
```

- Set `edge_enabled = false` when an external front owns port `443`, or keep `edge_enabled = true` and move `edge_addr` to another port to run both side by side.
- If your external front runs in a container or another network namespace, bind `http_addr` to an address it can reach.
- For Caddy users: `testd init` detects Caddy and offers to add the import line to your global Caddyfile. You can also run `testd export-caddyfile` to generate the managed Caddyfile and print the `import` line, and `testd doctor --fix` to verify the import and reload Caddy.
- With `edge_enabled = false`, browsers trust your external front's CA (such as Caddy's internal CA) rather than the testd root. `testd doctor` still checks testd's PKI because remote registration TLS needs it.
- Keep `trust_forwarded_for = false` unless a front you control sets `X-Forwarded-For`; otherwise clients can spoof their source IP. On the plain listener, `testd.test` is only served when `trust_forwarded_for = true` and the first forwarded hop is loopback.

## Automation

For non-interactive scripts or CI, supply a credential so `testd run` does not pause for approval:

```sh
export TESTD_TOKEN="$(cat /run/secrets/testd-token)"
testd run --host 192.168.1.5 --token-profile ci app.example:3000 -- ./server
```

Prefer an issued token (scoped and revocable) over the operator secret. A credential file under `~/.config/testd/credentials/<profile>` also works if you want to avoid putting the token in an environment variable (which is visible to same-user process inspection on most OSes).

`testd run` exits with the child process's exit code (and stops the child if registration fails). Use `testd doctor --json` for structured health checks in scripts.

## Design notes

Why testd is built the way it is:

- **One host is the authority:** DNS, TLS, tokens, and approvals live on the browser machine. No central server, accounts, mesh, or public DNS.
- **The connection is the lease:** A claim exists only while its TCP connection is open, so crashes and network drops clean up automatically.
- **Tokens are durable, grants are exact:** You approve a leaf once per name instead of on every run, and can revoke individual names or whole tokens later.
- **Encrypted over the network:** Remote registration uses NDJSON over pinned TLS 1.3; remote app forwarding uses mutual TLS. Local-only registrations keep a simple loopback protocol.
- **Local CA, short-lived leaves:** Trust one local root once, and get automatic HTTPS for every project without per-project cert files.

## Clean up and uninstall

Removing the desktop app package with your OS uninstaller does not remove the `.test` DNS route or trusted root certificate. To clean up completely:

1. Stop any running quick launches, SSH apps, and shares. If you use the desktop app, untick **Launch at login** and click **Exit** in the tray menu (or stop `testd host` / your system service).
2. Remove system integration:

```sh
testd uninstall          # removes owned DNS resolver route and root trust; keeps local config/data
testd uninstall --purge  # also deletes the config directory after system cleanup succeeds
```

3. Remove the installed binaries or desktop package (drag `Testd.app` to Trash, remove the `.deb`/`.AppImage`, run the Windows uninstaller, or delete `testd` from `~/.local/bin`).

Key safety guarantees across Linux, macOS, and Windows:

- **Ownership verified first:** `uninstall` only removes resolver rules and root certificates that belong to this installation and have not been modified externally. Unowned or changed system state is left untouched and reported as a conflict.
- **Missing root certificate:** if `pki/root.crt` is missing, trust removal and `--purge` will not complete; restore the certificate first.
- **Custom PKI paths:** `--purge` deletes the config directory (including the default PKI and tokens), but does not delete a custom `pki_dir` outside it.
- **Windows ownership records:** Windows verifies the private installation ID and administrator-owned NRPT and root-store records before removing anything, leaving backups in `%ProgramData%/testd/ownership/history`.
- **Binaries stay put:** `testd uninstall` cleans up system integration and optional config data; it does not delete the `testd` binary or desktop app.
