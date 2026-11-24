---
title: Advanced usage
description: 'Remote development, profiles, grants, quick launches, tunnels, external fronts, and PKI.'
---

# Advanced usage

This guide covers everything beyond basic local naming: SSH apps, direct remote leaves, profiles, quick launches, tunnels, external reverse proxies, privileged ports, desktop integration, PKI, and clean removal.

## SSH apps

SSH apps are the easiest way to reach a dev server on a remote machine without exposing port `7737` on your host.

1. Finish the local host setup on the machine with your browser (see [Quickstart](quickstart.md)).
2. Open the dashboard and go to **SSH apps**.
3. Enter the SSH host alias you normally pass to `ssh` (for example `devbox`), the remote port your app listens on, and the local `.test` name you want.
4. Save the app and click **Open**.

testd uses your existing OpenSSH configuration and surfaces any interactive SSH prompts (passphrases, host-key checks, 2FA) directly in the dashboard. Once the HTTP app on the remote machine's loopback responds, testd opens the local HTTPS URL. Click **Stop** to close the tunnel and release the name.

### Starting a remote command over SSH

If you want testd to start and stop the remote dev server for you, select **Start a remote command** when creating the SSH app, and enter the command, optional project directory, and the port it listens on at remote `127.0.0.1`.

- **Open** connects over one SSH session, starts the command, and forwards its port.
- **Stop** removes the local `.test` name and waits for the SSH leaf to stop the remote command and its child processes.
- If the SSH session drops or stops receiving heartbeats, the remote leaf cleans up the command and its children automatically.
- If the matching version of `testd` is not found in `~/.local/bin/testd` or on the remote `PATH`, the dashboard asks for confirmation before downloading and verifying it into `~/.local/bin/testd`. (Remote command execution targets Linux and macOS SSH hosts; Windows is supported as the local client.)

### SSH forwarding from the command line

You can also wire up SSH port forwarding yourself with `testd run`. The remote SSH server must allow local TCP forwarding:

```sh
testd run --host 127.0.0.1 app.example:18080 -- \
  ssh -NT -S none -o ForkAfterAuthentication=no \
    -o BatchMode=yes -o ExitOnForwardFailure=yes \
    -L 127.0.0.1:18080:127.0.0.1:3000 devbox
```

Once SSH connects, open `https://app.example.test`. If local port `18080` is busy, pick any free port and use it in both places. When the `ssh` process exits, testd removes the name.

Need an interactive SSH login (password or 2FA)? Run the `ssh` command in one terminal without `-o BatchMode=yes`, and once connected run `testd run --host 127.0.0.1 app.example:18080` in a second terminal. Stop both when you are done.

## Remote development

Use a direct remote leaf when your code runs in a VM, container, WSL distro, or LAN box that can reach your host directly.

### 1. Open the host intake listener

By default, `[host] listen_addr` binds `127.0.0.1:7737` (loopback only). To accept remote leaves, set it in `config.toml` to a reachable IP address (for example `192.168.1.5:7737`).

- Binding a specific non-loopback address automatically keeps a loopback listener on the same port for the dashboard and local launches.
- Binding `0.0.0.0:7737` listens on every IPv4 interface; if you use it, restrict port `7737` with a firewall. Never expose port `7737` to the public internet.

### 2. Pair the leaf with the host (once per host endpoint)

On the remote machine:

```sh
testd pair 192.168.1.5
```

Both the leaf and the host independently derive a 128-bit verification code from their TLS 1.3 session:

1. Check the host's code in the dashboard **Access** card or with `testd requests`.
2. Verify it matches the code shown on the leaf (never trust a code sent over an unverified channel).
3. Approve the pairing request on the host, then enter the host's code on the leaf.

Pairing requests expire after two minutes. Once approved, the leaf saves the host CA in `trusted-hosts/<endpoint-hash>.crt` (scoped to that endpoint and to testd, never added to the OS trust store).

- Routine transport certificate renewals keep working with the existing pin.
- If the host identity changes, testd fails before sending any credential. Verify the change, then run `testd pair` again; the old public pin is preserved as a fingerprinted `.previous.crt` file.
- If you use multiple host addresses in a node profile or `--hosts`, pair each one.

### 3. Request a publishing token

Network registration always uses TLS 1.3 (even when `TESTD_TOKEN` is set) and never falls back to plaintext. The first time you run `testd run` from a paired leaf without a token for the selected profile, it asks the host for publishing permission. Approve it on the host:

```sh
testd requests
testd approve <id>
```

The leaf saves the token to `~/.config/testd/credentials/<profile>` and reuses it on future runs. If the host is rebuilt or `tokens.json` is lost, the stored token will be rejected; delete the file and run again:

```sh
rm ~/.config/testd/credentials/<profile>
```

### How traffic flows to a remote leaf

For a paired leaf, traffic from the host to the leaf uses **mutual TLS 1.3**, and the final hop from the leaf forwarder to your dev server uses plain local HTTP:

- Leaf forwarding keys live in memory only for that registration connection.
- Your dev server can keep listening on `127.0.0.1` on the leaf.
- The leaf opens one dynamically allocated TLS port per published entry on the interface it used to reach the host (up to 64 endpoints per connection). The host needs a return route and firewall permission to reach those ports; pairing does not traverse NAT.
- Dropping the registration or withdrawing a grant immediately closes the affected forwarders and open streams.

By default, a remote leaf may only forward to addresses on the leaf itself (preventing a remote leaf from turning a `.test` name into a proxy into your host's loopback or LAN). You can relax the host-side target-address check with:

```toml
[host]
allow_remote_targets = true
```

Only enable this for leaves you already trust with that network reach. Note that unpaired numeric loopback and `localhost` endpoints keep the local-only protocol (verified against the actual peer address); pairing a loopback endpoint explicitly enables TLS. Source-rewriting SSH port forwards are not a drop-in remote-leaf topology.

## Credentials and profiles

testd uses two separate profile concepts so you can mix and match hosts and permissions cleanly:

| Profile | What it chooses | Where it lives |
|---|---|---|
| **Node profile** | Which host address(es) to connect to | `~/.config/testd/leaf.toml` |
| **Token profile** | Which credential file to present | `~/.config/testd/credentials/<profile>` |

### Node profiles (`leaf.toml`)

```toml
default_profile = 'home'

[profiles.home]
hosts = ['192.168.1.5:7737', '192.168.1.6:7737']
```

Pick a node profile with `--node-profile home` or `TESTD_PROFILE=home`, or override it for a single run:

```sh
testd run --host 192.168.1.5 :3000
testd run --hosts 192.168.1.5:7737,192.168.1.6:7737 :3000
```

Multiple addresses are useful when one host is reachable over more than one interface. (A token issued by one host does not authenticate against a different host.)

### Project token profiles (`.testd.toml`)

Give a project its own token profile by placing a `.testd.toml` file in the project root:

```toml
# .testd.toml (safe to commit; contains no secrets)
token_profile = 'myproject'
```

Separate token profiles mean separate grants and independent revocation. If every project uses `default`, they all share one token and one grant list.

### Precedence rules

1. **Host addresses:** `--host` / `--hosts` (or `TESTD_HOST` / `TESTD_HOSTS`) → selected node profile → host's own loopback listener. CLI `--host` and `--hosts` entries without a port default to `:7737`; env vars and `leaf.toml` entries must include a port.
2. **Token profile:** `TESTD_TOKEN_PROFILE` → `--token-profile` → `token_profile` in `.testd.toml` → `default`.
3. **Credential:** `TESTD_TOKEN` → `~/.config/testd/credentials/<profile>` → the host config's operator secret (only when the `default` token profile is active and every target host address is loopback).

The loopback operator-secret fallback is why local runs work without an approval prompt. It is never sent to another machine automatically.

For CI, obtain a scoped token interactively once and store `~/.config/testd/credentials/<profile>` in your CI secret store rather than sharing the broader operator secret. Never commit credentials.

## Grants and lifecycle

A simple mental model:

- **Token** = the key (durable permission stored on host and leaf).
- **Grant** = the exact list of names that key is allowed to publish.
- **Claim** = a live registration currently held open by an active TCP connection.

| Event | Token | Grant | Claim | What the leaf does |
|---|---|---|---|---|
| Ctrl+C or wrapped command exits | Kept | Kept | Released | Exits |
| Dashboard **Stop** | Kept | Kept | Released | Stops; next run registers immediately |
| Dashboard **Delete** on a name | Kept | That name removed | Released | Stops that name; asks again next time |
| **Revoke** token | Deleted | All names gone | Released | Closes connection and forgets credential |
| Network drop | Kept | Kept | Released when noticed | Reconnects and re-registers |
| Host reset | Gone | Gone | Gone | Credential rejected; delete file and run again |

If a leaf with a valid token tries to publish a name not yet in its grant list, the host does not reject it outright: it queues a request for just that new name and waits for your decision. Approving adds the name to the existing token. If a revoked token came from `TESTD_TOKEN`, unset or replace the environment variable before running again.

Names published by quick launches or with the operator secret have no per-leaf grant to delete, so the dashboard offers only **Stop** for them.

## Quick launches

A quick launch is a saved local command that you start and stop from the dashboard or desktop tray.

Each definition includes a label, command, optional working directory (give the full project path so commands do not run in the daemon's starting directory), optional environment variables, and one or more name entries:

- Leaving a target empty (or setting `auto`) automatically detects the port your child process listens on.
- Providing a port or `host:port` skips auto-detection.
- Each entry progresses from `configured` → `probing` → `confirmed`, and the `.test` name is registered only once the target accepts connections.

Definitions are saved in `~/.config/testd/launches.json`. Running state is not persisted across restarts, and logs are kept in memory (up to 500 lines). Clicking **Stop** ends the process group, and quitting the app stops all running launches.

## Tunnels in depth

`testd share` asks the running daemon to spawn a tunnel provider binary from `PATH` (no vendor SDKs or linked libraries):

| Provider | Exposure | Where the URL comes from | Notes |
|---|---|---|---|
| `cloudflared` | Public internet | Provider output | Quick tunnel; no account needed |
| `ngrok` | Public internet | Provider output | Run `ngrok config add-authtoken` once |
| `tailscale` | Tailnet | `tailscale status --json` | One Tailscale share at a time |

Override defaults with environment variables:

| Variable | Meaning |
|---|---|
| `TESTD_TUNNEL_PROVIDER` | Default provider when `--provider` is omitted |
| `TESTD_TUNNEL` | Explicit path to a provider binary outside `PATH` |

How routing and lifecycle work:

- Every provider points at the daemon's plain HTTP listener (`http_addr`), which runs whether or not built-in TLS is enabled.
- The provider hostname becomes a claim-bound alias. The edge preserves the incoming `Host` header, uses `trust_forwarded_for` for the scheme, and still authenticates secure leaves over mutual TLS (providers never receive host signing keys).
- Only one share is allowed per registered name. Stopping a share leaves the `.test` name live; removing the name closes the share.
- Changing a name's owner or target (including reconnecting to the same target) ends the share. Repeating an unchanged registration on the same connection keeps it.
- Share aliases are not restored after a daemon restart. After a crash, a provider process or `tailscale serve --bg` config may still be active; inspect provider state manually rather than assuming an empty share list means everything stopped.
- On startup, testd warns about matching Tailscale routes without deleting them, and refuses to overwrite an existing `:443` Tailscale serve rule. If provider cleanup cannot be verified, testd reports the error and blocks replacement until resolved.

Because the provider terminates TLS, visitors trust the provider's certificate rather than the testd root. Share only what you intend to expose.

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

## DNS resolution

testd runs a built-in DNS server on loopback (`127.0.0.1:7753` on Linux/macOS, `127.0.0.1:53` on Windows). On Linux, setup reuses a compatible existing global `.test` route without adopting it, and reports conflicting global settings or per-link routes rather than overwriting them.

## Privileged ports

- **Linux:** Binding port `443` requires `CAP_NET_BIND_SERVICE`, root, or a high port:
  - System service (recommended): `packaging/systemd/testd@.service` runs as your user with only `CAP_NET_BIND_SERVICE`.
  - Foreground CLI: `sudo setcap 'cap_net_bind_service=+ep' "$(readlink -f "$(command -v testd)")"` (re-run after replacing the binary).
  - High port: set `[host] edge_addr = '127.0.0.1:8443'` and open `https://app.example.test:8443`.
  - User service: `packaging/systemd/testd.service` sets `NoNewPrivileges=yes`, so it cannot use file capabilities; pair it with a high port.
- **macOS:** If binding `127.0.0.1:443` is denied, testd asks the logged-in user's `launchd` GUI session for that exact loopback socket, then unloads the temporary socket-transfer job before startup finishes. Both the daemon and tray stay unprivileged. A logged-in desktop session is required for this fallback; headless environments can use a high port or the launchd template in `packaging/launchd/`.
- **Windows:** No extra capability is needed beyond approving the normal firewall prompt.

## Desktop app and OS integration

The desktop tray app is a separate native Rust build. On first start, it opens a browser-based setup page on an ephemeral loopback port to configure the DNS resolver, root certificate, and operator secret (pre-filled with a generated pronounceable secret you can edit or replace). Once setup finishes and the daemon starts, the same tab navigates to the dashboard. Closing the tab leaves the tray running; use **Continue setup** to reopen it or **Exit** to stop the app.

The tray app:

- Starts the daemon and opens the dashboard using an owner-private HTML handoff file (never passing admin credentials in process arguments)
- Keeps **Launch at login** off until you opt in (unticking it or removing the OS login item keeps it off)
- Shows pending access requests as desktop notifications and changes its tray icon while a request is waiting
- Lists saved quick launches with **Start** and **Stop** actions and provides share controls

Notifications and OS URL-scheme links share one action format:

```text
testd://fix/resolver
testd://fix/certificate
```

`testd url <url>` forwards the action to the running daemon, or runs the repair itself (using a native authorization dialog when there is no terminal). Release builds register the `testd://` scheme automatically (at startup on Windows/Linux, via the app bundle on macOS); dev builds skip it unless `TESTD_REGISTER_SCHEME=1` is set.

Removing the desktop app with the OS uninstaller does not remove the `.test` DNS route or trusted root certificate. Run `testd uninstall` first to clean up system integration.

## Certificates and PKI

testd manages a three-tier local PKI under `~/.config/testd/pki/`:

- **Root CA** (`root.crt` / `root.key`): 10-year lifetime; installed once in your system trust store.
- **Host intermediate** (`node.crt` / `node.key`): 1-year lifetime; renews automatically near expiry on startup or certificate minting without changing root trust. Previous intermediates are backed up in `pki/backups/`.
- **Leaf certificates**: 24-hour lifetime; minted on demand in memory for each live name.

`testd doctor` reports upcoming expiry and `testd doctor --fix` repairs the PKI if `root.key` is present. (If you moved `root.key` offline, bring it back before the intermediate expires.)

The dashboard's **Certificate** card shows trust status, SHA-256 fingerprint, expiry, and install/remove buttons (plus a separate Firefox trust check when viewed in Firefox). Anyone on the host can download the public root certificate with:

```sh
curl http://127.0.0.1:7737/ca.crt -o testd-root.crt
```

Keep `pki/root.key` strictly on the host. If it is ever exposed, rebuild the PKI, reinstall trust on every browser machine, and restart the host.

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

## Clean up

Stop the daemon first, then remove system integration:

```sh
testd uninstall          # removes owned DNS resolver route and root trust; keeps local config/data
testd uninstall --purge  # also deletes the config directory after system cleanup succeeds
```

Key safety guarantees across Linux, macOS, and Windows:

- **Ownership verified first:** `uninstall` only removes resolver rules and root certificates that belong to this installation and have not been modified externally. Unowned or changed system state is left untouched and reported as a conflict.
- **Missing root certificate:** If `pki/root.crt` is missing, trust removal and `--purge` will not complete; restore the certificate first.
- **Custom PKI paths:** `--purge` deletes the config directory (including the default PKI and tokens), but does not delete a custom `pki_dir` outside it.
- **Windows ownership records:** Windows verifies the private installation ID and administrator-owned NRPT and root-store records before removing anything, leaving backups in `%ProgramData%/testd/ownership/history`.
- **Binaries stay put:** `testd uninstall` cleans up system integration and optional config data; it does not delete the `testd` binary or desktop app.
