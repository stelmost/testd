---
title: Documentation
description: 'Everything for running testd: quickstart, host setup, local and remote workflows, sharing, troubleshooting, reference, and security.'
---

# testd documentation

testd gives your dev projects real, stable HTTPS addresses like `https://app.example.test` on the machine where your browser runs, whether the project runs locally, in a container, a VM, WSL, or on a remote box over SSH or your LAN.

You do not need to edit `/etc/hosts`, manage per-project certificates, or run a separate reverse proxy. The browser machine runs the host daemon (via the desktop tray app or `testd host`); each dev environment connects to it and registers the names it needs.

New to testd? Start with the [Quickstart](quickstart.md).

## Pick where to start

<div class="doc-grid">
	<a class="doc-card" href="./quickstart.html">
		<span class="doc-card__index">01</span>
		<strong>Quickstart</strong>
		<span>Install the desktop app or CLI, set up your host, and open your first HTTPS name.</span>
	</a>
	<a class="doc-card" href="./host.html">
		<span class="doc-card__index">02</span>
		<strong>Running the host</strong>
		<span>Desktop tray app, guided setup, headless daemon, the web dashboard, and mixing GUI with CLI.</span>
	</a>
	<a class="doc-card" href="./local.html">
		<span class="doc-card__index">03</span>
		<strong>Local apps</strong>
		<span>Registering names with <code>testd run</code>, naming rules, lifetimes, and quick launches.</span>
	</a>
	<a class="doc-card" href="./remote.html">
		<span class="doc-card__index">04</span>
		<strong>Remote &amp; SSH</strong>
		<span>SSH apps, remote commands, direct remote leaves, pairing, grants, and profiles.</span>
	</a>
	<a class="doc-card" href="./sharing.html">
		<span class="doc-card__index">05</span>
		<strong>Sharing &amp; tunnels</strong>
		<span>Expose a live name over <code>cloudflared</code>, <code>ngrok</code>, or <code>tailscale</code>.</span>
	</a>
	<a class="doc-card" href="./system.html">
		<span class="doc-card__index">06</span>
		<strong>System integration</strong>
		<span>Privileged ports, DNS, certificates and PKI, external fronts like Caddy, and clean uninstall.</span>
	</a>
	<a class="doc-card" href="./reference.html">
		<span class="doc-card__index">07</span>
		<strong>CLI &amp; configuration</strong>
		<span>Every command, flag, config key, file path, HTTP endpoint, limit, and platform note.</span>
	</a>
	<a class="doc-card" href="./troubleshooting.html">
		<span class="doc-card__index">08</span>
		<strong>Troubleshooting</strong>
		<span><code>testd doctor</code>, tray repair alerts, common fixes, and layer-by-layer diagnostics.</span>
	</a>
</div>

## How testd works in 30 seconds

There are two sides:

- **Host** (the machine with your browser): runs either the desktop tray app or `testd host`. It answers `.test` DNS queries locally, terminates HTTPS using a local certificate authority, stores issued tokens, serves the web dashboard, and proxies traffic to each live app.
- **Leaf** (where your code runs: this machine, a VM, container, WSL, or remote box): runs `testd run` and keeps one outbound connection open to the host. While that connection is open, the name is live. When it closes, the name disappears automatically.

Both binaries share the same config directory, CA, tokens, and dashboard. Most people run the desktop app on their host machine and install the CLI in terminals or remote dev boxes. If you reach a remote machine over SSH, you can also use **SSH apps** right from the dashboard without opening any ports on your host.

For trust boundaries, credentials, and hardening, see [Security](security.md).

## Support and contact

- **Bugs and pull requests:** open an [issue](https://github.com/stelmost/testd/issues) or [pull request](https://github.com/stelmost/testd/pulls) on GitHub
- **Discord:** join the [Discord server](https://discord.gg/skEnmNGm9G) and ask in `#testd-support`
- **Email:** [hey@elmo.st](mailto:hey@elmo.st)
- **Security vulnerabilities:** use [GitHub private vulnerability reporting](https://github.com/stelmost/testd/security/advisories/new) instead of a public issue

## Source and releases

The source code, changelog, and release archives live at <https://github.com/stelmost/testd>. The product site is at <https://testd.dev>. testd is ISC licensed and has no account, telemetry, public DNS, or ACME dependency.
