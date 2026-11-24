# Security policy

## Supported versions

The latest minor release of the current major version receives security fixes.
Older majors keep working but do not receive fixes.

## Reporting a vulnerability

Use [GitHub private vulnerability reporting](https://github.com/stelmost/testd/security/advisories/new). Do not open a public issue for a suspected vulnerability. Private reporting must be enabled on the public repository before its first release. Include:

- affected version and platform
- reproduction steps or a proof of concept
- impact: name spoofing, traffic interception, secret disclosure, local escalation

Expect an acknowledgement within 7 days and a fix or mitigation plan within 30
days. Credit is given unless you ask otherwise.

## Design boundaries

- The daemon never needs root at runtime; privileged setup is limited to the OS
  resolver and the root CA trust store.
- The root CA key stays in the host config directory and is never copied to a dev environment.
- Leaf certificates live 24 hours; the local intermediate lives one year and renews near expiry without replacing the root. Renewal requires the root key and backs up the previous intermediate first.
- Leaf tokens are approved only over the node's loopback dashboard or control channel, stored with owner-only permissions, and revocable. A token request never receives the operator secret. Dashboard reads, mutations, and event streams require a separate owner-scoped per-run credential from `control.json`; setup uses its own ephemeral credential. A bare dashboard URL exposes no live state.
- A remote leaf may only target its own address unless `[host] allow_remote_targets` is enabled. This keeps an issued name grant from becoming a proxy into the host's loopback or another network.
- Browser-facing TLS is rustls with the ring provider. There is no OpenSSL in the dependency tree.
- Network leaf registration and token issuance use TLS 1.3 after explicit host pairing and saved host-CA verification. Application forwarding from the host to a secure leaf uses mutual TLS with connection-scoped leaf keys; the dev server can remain local HTTP. Do not expose intake port 7737 to the internet.
- Intake accepts at most 256 simultaneous connections. NDJSON frames are limited to 64 KiB, HTTP headers to 32 KiB, setup and started-frame reads to 10 seconds, and reply writes to 5 seconds. Idle authenticated leaves keep their registrations.
- The product does not send telemetry, usage data, or names anywhere.
