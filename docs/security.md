---
title: Security
description: 'Trust boundaries, credentials, exposure, and hardening.'
---

# Security

testd is a development tool. It gives names and HTTPS to machines you control, and it assumes the host is the trust authority. It is not a production ingress controller, a public DNS service, or a multi-tenant platform.

This page covers the operational security model. Vulnerability reporting is in [SECURITY.md](https://github.com/LapilliWorks/testd/blob/master/SECURITY.md).

## Trust boundaries

| Path | Transport | Trust |
|---|---|---|
| Browser to host edge | TLS, rustls with the ring provider | Browser trusts the testd root or a sharing provider |
| Network leaf to host intake | TLS 1.3, NDJSON | Host CA pinned after explicit TLS-bound code comparison; bearer tokens sent only after verification |
| Host to secure leaf | Mutual TLS 1.3 | Leaf CA bound to its authenticated registration; host client certificate required |
| Leaf forwarder to app | Local HTTP/1.1 | Target must resolve to an interface on that leaf |
| Local-only registration and direct targets | Loopback registration, HTTP/1.1 upstream | Local direct registrations do not gain encrypted remote target forwarding |
| Dashboard and setup | Loopback HTTP, or dashboard HTTPS through the edge | Owner-scoped per-run or per-setup bearer credential; loopback alone grants no authority |
| Control channel | Loopback TCP, no TLS | Same-user local access plus a per-run token |

The root CA and intermediate are created by the host. The root key stays on the host and is never copied to a dev environment. Leaf certificates are minted on demand for live names and live 24 hours.

## Secrets

| Secret | Where it lives | Scope |
|---|---|---|
| Operator secret | `[host] secret` in `config.toml` | Any valid name on this host |
| Issued token | `tokens.json` on the host; `credentials/<profile>` on a leaf | The names in its grant; revocable |
| Root CA key | `pki/root.key` | Can sign any certificate under the root |
| Intermediate key | `pki/node.key` | Can issue leaf certificates |
| Control token | `control.json` | Local launches and control channel for this daemon run |
| Dashboard credential | `control.json`, then origin-scoped browser session storage | Local administration for this daemon run; distinct from registration and control tokens |
| Setup credential | Memory and setup window session storage | One setup surface; not reusable in another setup session |
| Leaf forwarding keys | Memory only, per registration connection | Accept application connections from the paired host CA |

`config.toml`, `tokens.json`, `credentials/<profile>`, `control.json`, and private keys are written with user-only permissions on Unix. On Windows, private files are created with a protected ACL granting access only to the owner, SYSTEM, and Administrators, including temporary files used for atomic replacement. Reads verify ownership and ACLs before using credentials, control data, saved host trust, or keys. Unowned files, links, broader ACLs, and filesystems without verifiable ACLs are refused rather than silently repaired or adopted. Private writes also refuse redirected parents, untrusted owners, and parent access rules that let other users modify their contents or permissions.

Back up existing files and their ACLs before manual permission repair. Restricting an already exposed file does not revoke a copied secret or key: rotate compromised credentials and rebuild exposed PKI. Treat any copy as a credential. A leaked operator secret can register any valid name; a leaked issued token can publish only its granted names.

The operator secret cannot be revoked individually. To rotate it, write a new value into `[host] secret` and restart the daemon. Leaves that were admitted with the old secret must be updated. Issued tokens remain valid.

To withdraw one leaf, revoke its token. To withdraw one name, delete that name's grant. Both close the affected connections and release the live claims.

## Registration network

Network registration and token issuance require TLS 1.3. Before connecting with a token, pair the host with `testd pair <endpoint>`. Both endpoints independently derive a 128-bit code using the TLS exporter. The operator compares their trusted displays, approves the host-side pairing request, and enters the host's code on the leaf. An echoed client-supplied secret is not accepted as proof of host identity. Pairing never carries or issues a bearer credential.

The saved public CA pin is scoped to the endpoint and testd's TLS client, not the OS trust store. TLS certificate signatures and names are verified; a changed identity fails before authentication, without a plaintext retry or bypass through `TESTD_TOKEN`. Explicit re-pairing preserves the previous public pin. Ordinary certificate renewal under the pinned CA does not require pairing again.

- Keep intake port 7737 restricted to the intended network; TLS does not turn it into a public ingress service.
- Allow the host to reach the leaf's dynamically allocated application TLS listeners on the selected interface. Pairing does not provide NAT traversal.
- Compare the host's code through a trusted host terminal or dashboard, not a message from the unverified connection.
- Prefer networks that preserve peer addresses. Unpaired loopback endpoints retain the local-only protocol, and source-rewriting SSH forwards change that boundary; they are not a drop-in remote topology.
- Treat a token copied into CI as a production credential for the names it covers.

Issued tokens are scoped and revocable. The operator secret is broad and survives token changes. Use issued tokens wherever approval is possible, and reserve the secret for unattended bootstrap.

`TESTD_TOKEN` is visible to the same user's process inspection on most systems. Prefer the credential file when that exposure matters. Do not put either in a command line, a repository, or a log.

## Remote targets

By default, a leaf that connects from a non-loopback address may only target its own address. This prevents an issued name from becoming a proxy into the host's loopback services or another network.

```toml
[host]
allow_remote_targets = true
```

Enable it only for leaves you would already trust with that reach. It relaxes the host's endpoint-address restriction; it does not disable TLS, leaf certificate verification, or the secure forwarder's local-application restriction. Local-only direct registrations can still name HTTP targets and do not provide encrypted remote forwarding. For a remote application, run a paired leaf beside it.

## The local dashboard

The dashboard is served on loopback and checks that the `Host` header is `localhost`, `127.0.0.1`, `[::1]`, or `testd.<tld>`. That check blocks DNS-rebinding pages from becoming local admin clients.

Administration requires the per-run dashboard credential from the owner's private `control.json`. Tray and CLI dashboard links deliver it in a URL fragment; the page removes the credential from history and keeps it in origin-scoped session storage, not cookies shared across local ports. API and event requests use an Authorization header. An uncredentialed page contains no live names, launches, grants, or requests. The public stylesheet, liveness image, and public root certificate remain downloadable.

The credential is separate from the operator secret, issued leaf tokens, and local launch/control token. A restart creates a new credential; old sessions receive 401 and must be reopened through the tray or CLI. There is no account service or dashboard password. Treat a copied bootstrap link as an administrative credential for that daemon run.

First-run setup has its own in-memory credential. Unauthenticated local callers cannot read or regenerate the operator secret, request privileged setup, or finish another user's setup surface.

A `POST` also needs `X-Testd-Request: 1`, which a cross-site form cannot set and a cross-origin fetch cannot pass without a preflight this server does not grant. That header is the CSRF boundary.

The page keeps an authenticated event stream open for live updates. The stream is loopback-only like the rest of the dashboard.

`https://testd.test/` reaches the same listener through the edge. The edge forwards the dashboard name only when the client is on this machine: the immediate peer must be loopback, and with a configured front proxy the first `X-Forwarded-For` hop must be loopback too. On the plain listener, which only a front reaches, the dashboard name requires `trust_forwarded_for = true`; without it the edge cannot tell the real client and returns 404. Every other client gets a 404.

Do not port-forward the loopback listener. The only network path to it is the edge, which forwards `testd.<tld>` for loopback clients only; the control channel still uses its per-run token from `control.json`.

## Certificates and trust

The testd root is a local certificate authority. Anything that trusts it accepts certificates signed by its key. Protect `pki/root.key` accordingly:

- Keep the config directory private to the host user.
- Do not copy the root key to dev machines or containers.
- If the root key is exposed, rebuild the PKI, reinstall trust on every browser machine, and restart the host.
- If the root key is moved offline, restore it before the intermediate expires; the intermediate renews with it.

Installing the root on a browser machine is a real trust change. On Linux, use `testd uninstall` to remove owned trust and resolver integration when the machine no longer uses testd. macOS uninstall remains blocked until equivalent safeguards exist; Windows uninstall remains blocked pending native validation. Removing the desktop app does not remove the root or the resolver route.

Windows records new NRPT rule identities and complete root-store certificate snapshots under administrator-only `%ProgramData%/testd/ownership`. Cleanup requires the private installation ID and unchanged recorded state. Compatible pre-existing rules and roots are not adopted. New root installation refuses a certificate created in the meantime, and installs the already validated certificate from memory rather than reopening a mutable path. Interrupted setup with incomplete ownership preserves live state for review; interrupted removal can be retried. Private record backups are retained after cleanup.

On Linux, testd checks the distro's system CA bundle and uses an already trusted root without adopting its source. Setup reports a conflict rather than overwriting a different certificate at its system anchor path. Removal requires the installation's private ownership ID, its system ownership record, and an unchanged anchor and certificate; if ownership cannot be established, the anchor is left in place and the conflict is reported. The Linux DNS route uses the same ownership rule for its resolver drop-in. Inspect and back up existing files before making any changes.

Firefox may use its own trust store and need a separate import. That is a browser configuration step, not an additional testd authority.

## Tunnels and exposure

A share is an explicit exposure decision. For secure leaf claims, providers route through the local edge, which preserves the claim's leaf TLS identity. Providers do not receive host signing keys.

- `cloudflared` and `ngrok` create public URLs. The provider terminates TLS and forwards traffic to the app. Anyone with the URL can reach the app; the tunnel adds no authentication.
- `tailscale serve` exposes the name to the tailnet. Its privacy depends on tailnet membership and ACLs.
- The provider sees the application traffic in both cases, including credentials the app sends.

Stop a share when it is no longer needed. A share is bound to the owning connection and target; a reconnect or target change requires sharing again. Provider cleanup and crash recovery still require verification, and a running dev server and forgotten URL can outlive the reason for sharing.

The provider binary is spawned from `PATH` or `TESTD_TUNNEL`. A compromised binary runs with the daemon's privileges and can read the host state.

## External front and forwarded headers

`trust_forwarded_for` must stay off unless a front you control sets `X-Forwarded-For` and strips any client-supplied value. With the setting on and the edge reachable directly, a client can spoof its source address. `X-Forwarded-Proto` is set by the edge itself.

An external front that terminates TLS has its own trust anchor and certificate lifecycle. The testd root is not involved unless the front proxies to the testd TLS edge.

## DNS resolution

The DNS server does not need root at runtime and listens on loopback only. Configuring the OS resolver may require privileges.

## Resource limits

The daemon bounds work so a buggy or hostile local client cannot exhaust it without effort:

- 256 simultaneous intake connections
- 64 KiB NDJSON frames
- 512 edge connections total, 64 per source address
- 32 pending access or pairing requests, each with a two-minute lifetime
- 64 application TLS endpoints per leaf connection, with 64 simultaneous incoming connections per endpoint
- 1024 cached edge certificates
- 32 KiB HTTP request heads and 256 KiB bodies

These are robustness limits, not authentication. Network exposure still needs a network control such as a firewall, a VPN, or a private tailnet.

## Desktop integration

The tray app registers the `testd://` scheme in release builds. The scheme accepts `fix/resolver` and `fix/certificate`; `testd url` forwards them to the running daemon or performs the repair directly. There is no general command channel over the scheme.

The loopback control channel accepts typed commands authenticated by the per-run token in `control.json`: repairs, share start and stop, request decisions, and token revocation. It is not reachable off the machine. The token has no name scope, is accepted only by the daemon that wrote it, and does not appear in the token store.

The root-managed systemd unit grants only `CAP_NET_BIND_SERVICE` and keeps `NoNewPrivileges=yes`. The user unit cannot acquire a file capability, so it must use a high edge port. Keep that separation rather than running the daemon as root.

## Incident checklist

1. Stop sharing names and stop the affected leaves.
2. Revoke issued tokens with `testd tokens --revoke <id>`.
3. If the operator secret leaked, replace it and restart the daemon.
4. If the root or intermediate key leaked, rebuild the PKI, reinstall trust everywhere, and restart.
5. Remove any exposure at the firewall, tunnel, or tailnet layer.
6. Check `~/.config/testd/tokens.json` and the dashboard Access card for unexpected grants.
7. When a Linux host is retired, stop the daemon and run `testd uninstall --purge` after reviewing the data to be deleted. macOS and Windows CLI cleanup remain blocked pending ownership safeguards or native validation.

## What testd does not do

- No telemetry, usage data, or name reporting
- No public DNS or ACME dependency
- No vendor SDK for tunnels; providers are spawned processes
- No remote administration API
- No guarantee of isolation between projects that share a host or a token profile
- No name that outlives its leaf connection or target; a sleeping laptop or stopped server takes its names with it
