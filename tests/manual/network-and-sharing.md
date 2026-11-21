# Remote development and share contracts

The implemented remote-development and advertised provider contracts require real evidence on Windows 10 22H2 amd64, Debian 13 amd64, and macOS 15 arm64 for the first release. Other targets are best effort; hardware-specific behavior is deferred. Separate network controls and network-change pause are not implemented, so their checks below are deferred. Claim-bound share lifetime has fake-provider coverage but still requires real-provider evidence and a crash-recovery policy.

Use two isolated full-system VMs restored to recorded baselines, synthetic services, and short-lived fixture credentials. Separate explicit operator authorization must identify the selected interfaces, intake and return-path firewall scope, provider tools/versions, accounts, and egress before network exposure or real-provider checks. Record the candidate commits and hashes, interface addresses, authorization decisions, process and provider observations, and results in private persisted reports. Never commit reports, tokens, private keys, or live public URLs; do not put credentials in process arguments or public logs.

## Implemented remote development

- [ ] NET-01: A fresh instance accepts registration and HTTPS only on loopback, serves DNS only on loopback, and does not expose the dashboard to the second VM. Verify refusal from that VM, not merely the configured bind addresses.
- [ ] REMOTE-01: Explicitly configure `[host] listen_addr` on the authorized interface. The other VM can reach that intake; dashboard and local control remain unavailable remotely, and DNS and application edges retain their loopback defaults. Record the host-to-leaf return path for dynamically allocated TLS forwarders.
- [ ] REMOTE-02: Approve a pairing request but enter a mismatched code on the leaf. No host trust or publishing credential is installed, and no registration is admitted.
- [ ] REMOTE-03: Compare independently displayed host and leaf codes, approve pairing, and enter the matching host code. Only endpoint-scoped public trust is saved; pairing alone does not issue a publishing grant.
- [ ] REMOTE-04: Replace the synthetic host identity at the same endpoint. The previously paired leaf refuses it before sending credentials, with no plaintext fallback. Explicitly re-pair only after independent verification; the previous public pin is preserved.
- [ ] REMOTE-05: Request and approve a scoped publishing grant, register the synthetic application, and verify application data and WebSocket upgrades through mutual TLS. A client with no or the wrong client certificate cannot reach the leaf application forwarder; plaintext remote registration is refused.
- [ ] REMOTE-06: Revoke the established grant while application streams are active. Its intake connection, names, forwarders, and streams close; unrelated grants remain. Reconnection with the withdrawn credential is refused, and a fresh approval request is not evidence of credential reuse.

## Deferred network controls

These checks do not establish current-release coverage and must not be recorded as passes before the corresponding controls exist.

- [ ] NET-02: When the separate controls exist, explicitly enable only registration on a selected interface. Remote registration works only there, local dashboard and quick-launch control continue to work, and other surfaces remain unexposed.
- [ ] NET-03: Explicitly enable only the application edge on a selected interface. The intended remote client can reach an authorized app, while registration and dashboard exposure remain independently controlled.
- [ ] NET-04: Remote dashboard access remains disabled until explicitly enabled and requires the approved authentication and session protections. Test a different local OS user, an unauthenticated remote browser, CSRF, and DNS rebinding after that design exists.
- [ ] NET-05: While affected HTTP, streaming, WebSocket, dashboard, and registration connections are active, disconnect or reassociate the selected connection. Affected exposure pauses and connections close; stored grants remain.
- [ ] NET-06: Reconnect with the same network label. Exposure remains paused until explicit confirmation unless authenticated continuity has been established; a label or SSID alone is not proof of continuity.
- [ ] NET-07: Repeat the applicable transition and confirmation checks for wired and wireless connections. Verify that unaffected interfaces remain available only when their independence is established.
- [ ] NET-08: On network pause, affected or not-provably-independent shares stop and affected pending starts are cancelled. They do not restart automatically after network confirmation; cleanup failures are visible.

## Local HTTPS over SSH

Use the [SSH forwarding recipe](../../docs/advanced.md#ssh-forwarding) with a test app that provides an HTTP response and a WebSocket echo. Bind the app to the SSH server's loopback and keep the testd listeners at their loopback defaults.

- [ ] SSH-01. Verify HTTPS with the local testd CA and exchange a `wss` message through the tunnel. Check that the forwarding port listens only on `127.0.0.1`.
- [ ] SSH-02. Stop SSH while the wrapper is running. Check that the name is deregistered and the local forwarding port is released. Repeat by interrupting `testd run`.
- [ ] SSH-03. Run the recipe with the local port already occupied. Confirm that SSH reports the bind failure, `testd run` returns a failure, and no live registration remains.

## Share lifetime

Start with a controlled registered service and an approved provider fixture. Repeat applicable lifecycle and exposure checks with the advertised `cloudflared`, `ngrok`, and `tailscale` providers on each required target, using separately authorized tools/accounts and synthetic content. Verify the provider endpoint and declared public or tailnet exposure before sending any request; fake-provider results cannot substitute for these checks.

- [ ] SHARE-01: Start and stop a share. Its published URL, exposure, target, and provider are visible; stop removes the publication while the underlying local registration remains.
- [ ] SHARE-02: Repeat the same name and target on the same connection. The existing share and provider remain uninterrupted.
- [ ] SHARE-03: Change the registered target on the same connection. The previous share stops and does not publish either the old or the new target until explicitly started again.
- [ ] SHARE-04: Force a name takeover from another connection, then separately reconnect using the same credential and target. Each new owner causes the previous share to stop and requires explicit re-sharing.
- [ ] SHARE-05: Fail an owner or target change and restore the original claim without an intervening successful change. The previous share and provider remain uninterrupted. An unverified rollback or intervening successful owner or target change stops it.
- [ ] SHARE-06: Hold a provider at a controlled startup barrier, remove or change its authorizing claim, then release the barrier. The start is cancelled and no share is listed or reachable. Verify the provider stopped; if its cleanup is unverified, the error is visible and replacement is refused.
- [ ] SHARE-07: On normal stop, daemon exit, failed provider startup, and provider exit, check both the reported share list and the actual owned provider state. A missing list entry or a successful provider command alone is not proof of cleanup. Unknown residue is incomplete, not success; changed or unowned provider state must not be adopted or deleted.
- [ ] SHARE-08: After the cleanup and recovery policy is decided, interrupt the disposable daemon and test restart reconciliation. Unrelated provider processes, accounts, and configuration must not be changed without proven ownership.

A fake provider is useful for deterministic lifecycle tests but cannot establish a real provider's exposure, stop behavior, network independence, or crash cleanup. Record those as separate platform and provider results.
