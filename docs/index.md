---
title: Documentation
description: 'Everything for running testd: quickstart, daily usage, advanced setups, reference, and the security model.'
---

# testd documentation

testd gives your dev projects real, stable HTTPS addresses like `https://app.example.test` on the machine where your browser runs, whether the project runs locally, in a container, a VM, WSL, or on a remote box over SSH or your LAN.

You do not need to edit `/etc/hosts`, manage per-project certificates, or run a separate reverse proxy. The browser machine runs `testd host`; each dev environment connects to it and registers the names it needs.

New to testd? Start with the [Quickstart](quickstart.md).

## Pick where to start

<div class="doc-grid">
	<a class="doc-card" href="./quickstart.html">
		<span class="doc-card__index">01</span>
		<strong>Quickstart</strong>
		<span>Install testd, prepare your host, start the daemon, and open your first HTTPS name.</span>
	</a>
	<a class="doc-card" href="./usage.html">
		<span class="doc-card__index">02</span>
		<strong>Daily usage</strong>
		<span>Registering names, the dashboard, SSH apps, sharing with tunnels, access requests, and fixes.</span>
	</a>
	<a class="doc-card" href="./advanced.html">
		<span class="doc-card__index">03</span>
		<strong>Advanced usage</strong>
		<span>Remote leaves, profiles, grants, quick launches, tunnels, external fronts, and PKI.</span>
	</a>
	<a class="doc-card" href="./reference.html">
		<span class="doc-card__index">04</span>
		<strong>Reference</strong>
		<span>Commands, options, configuration keys, files, endpoints, limits, and platform notes.</span>
	</a>
	<a class="doc-card" href="./security.html">
		<span class="doc-card__index">05</span>
		<strong>Security</strong>
		<span>Trust boundaries, credentials, exposure, hardening, and the incident checklist.</span>
	</a>
	<a class="doc-card" href="./dependency-policy.html">
		<span class="doc-card__index">06</span>
		<strong>Dependency policy</strong>
		<span>Desktop library choices, advisory exceptions, and MSRV constraints.</span>
	</a>
</div>

## How testd works in 30 seconds

There are two sides:

- **Host** (the machine with your browser): runs `testd host`. It answers `.test` DNS queries locally, terminates HTTPS using a local certificate authority, stores issued tokens, and proxies traffic to each live app.
- **Leaf** (where your code runs: this machine, a VM, container, WSL, or remote box): runs `testd run` and keeps one outbound connection open to the host. While that connection is open, the name is live. When it closes, the name disappears automatically.

If you connect to a remote machine over SSH, you can also skip setting up a network listener altogether and use **SSH apps** right from the dashboard.

## Source and releases

The source code, changelog, and release archives live at <https://github.com/stelmost/testd>. The product site is at <https://testd.dev>. testd is ISC licensed and has no account, telemetry, public DNS, or ACME dependency.
