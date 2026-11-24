---
title: Remote & SSH
description: 'Reach dev servers on other machines with SSH apps or direct paired leaves, plus profiles, grants, and tokens.'
---

# Remote & SSH

You have two ways to publish a dev server running on another machine (a remote box, VM, container, or WSL distro) as a local `https://<name>.test` address:

- **SSH apps (easiest):** tunnel over your existing OpenSSH setup from the dashboard or tray. No ports need to be opened on your host.
- **Direct remote leaves:** connect a remote `testd` straight to your host over pinned TLS 1.3 and mutual TLS forwarding. Best for LAN boxes, VMs, containers, and WSL.

## SSH apps

SSH apps let you open a remote dev server as a local `https://<name>.test` URL using your normal OpenSSH setup (`~/.ssh/config`, keys, passphrases, 2FA prompts) without exposing port `7737` on your host.

1. Finish the local host setup on the machine with your browser (see [Quickstart](quickstart.md)).
2. Open the dashboard and go to **SSH apps**.
3. Enter the SSH host alias you normally pass to `ssh` (for example `devbox`), the remote port your app listens on, and the local `.test` name you want.
4. Save the app and click **Open** (saved SSH apps can also be started and stopped from the desktop tray menu).

testd surfaces any interactive SSH prompts (passphrases, host-key checks, 2FA) directly in the dashboard. Once the HTTP app on the remote machine's loopback responds, testd opens the local HTTPS URL. Click **Stop** to close the tunnel and release the name. Forwarding an existing app needs only OpenSSH on the remote host.

### Starting a remote command over SSH

If you want testd to start and stop the remote dev server for you, select **Start a remote command** when creating the SSH app, and enter the command, optional project directory, and the port it listens on at remote `127.0.0.1`:

- **Open** connects over one SSH session, starts the command, and forwards its port.
- **Stop** removes the local `.test` name and waits for the SSH leaf to stop the remote command and its child processes.
- If the SSH session drops or stops receiving heartbeats, the remote leaf cleans up the command and its children automatically.
- Before starting, testd checks `~/.local/bin/testd` and the remote `PATH` for a matching version. If it is missing or different, the dashboard prompts you to approve installing that version into `~/.local/bin/testd` (cancelling leaves the remote host untouched).

Details on remote commands and automatic install:

- Remote command execution targets **Linux and macOS** SSH hosts. Windows works as the local client, but starting commands and auto-installing on native Windows SSH hosts are not supported.
- Automatic installation supports Linux `x86_64` and macOS `x86_64` / `arm64`. It downloads the matching version-pinned CLI release from GitHub over HTTPS, verifies its `SHA256SUMS` entry, and backs up any existing `~/.local/bin/testd` under `~/.local/share/testd/backups`.
- The remote host needs a POSIX shell, `tar`, `curl` or `wget`, and `sha256sum` or `shasum`. No root privileges or remote `PATH` edits are needed because testd invokes the installed binary by its full path.

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

## Direct remote leaves

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

Pairing requests expire after two minutes. Once approved, the leaf saves the host CA in `trusted-hosts/<endpoint-hash>.crt` (scoped to that endpoint and to testd, never added to the OS trust store). Pairing saves host trust only; it does not grant publishing permission yet.

- Routine transport certificate renewals keep working with the existing pin.
- If the host identity changes, testd fails before sending any credential. Verify the change, then run `testd pair` again; the old public pin is preserved as a fingerprinted `.previous.crt` file.
- If you use multiple host addresses in a node profile or `--hosts`, pair each one.

### 3. Request a publishing token

Network registration always uses TLS 1.3 (even when `TESTD_TOKEN` is set) and never falls back to plaintext. The first time you run `testd run --host 192.168.1.5 :3000` without a token for the selected profile, it asks the host for publishing permission. Approve it in the dashboard's **Access** card, from the desktop tray notification, or with the CLI:

```sh
testd requests
testd approve <id>
testd deny <id>
```

Requests include the leaf label (for example `user@host:example`), requested names, source address, and age, and expire after two minutes. If a request is denied or expires, run the leaf command again to send a fresh request.

The leaf saves its issued token to `~/.config/testd/credentials/<profile>` and reuses it on future runs. Manage issued tokens from the dashboard's **Tokens** card or the CLI:

```sh
testd tokens
testd tokens --revoke <id>
```

Token secrets are never printed. If the host is rebuilt or `tokens.json` is lost, the stored token will be rejected; delete `~/.config/testd/credentials/<profile>` and run again.

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

Multiple connections can reuse the same token and share its grants. If a leaf with a valid token tries to publish a name not yet in its grant list, the host queues a request for just that new name and holds the registration until you decide; approving adds the name to the existing token without disturbing its other names. If a revoked token came from `TESTD_TOKEN`, unset or replace the environment variable before running again.

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

For unattended automation or CI where interactive approval is not possible, obtain a scoped token interactively once and store `~/.config/testd/credentials/<profile>` (or pass it via `TESTD_TOKEN`) rather than sharing the broader operator secret. Never commit credentials. See [Automation](system.md#automation).
