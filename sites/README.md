# testd site

The product site lives under `sites/`; the documentation Markdown lives under `docs/` at the repository root. Both are built and served as one tree from the repository root. The product page is at `/`, and the VitePress docs are at `/docs/`.

## Develop

The dev server runs in a container; no local install is needed to run it.

```sh
./sites/bin/site.sh dev  # product at http://127.0.0.1:5173/, docs at /docs/
# or
bun run dev
```

`site.sh` runs the official Bun image (`oven/bun:1.4.4`) with the site sources mounted, installs dependencies into the `testd-site-node-modules` volume, and runs the stock `vitepress dev` and `vite` CLI entrypoints with Bun. It prefers `podman`, falls back to `docker`, and can be pinned with `TESTD_SITE_RUNTIME`. The product server is the public origin and proxies `/docs/` to VitePress on loopback.

`--host <address>` (or `TESTD_SITE_HOST`) moves the published port off loopback; `--host 0.0.0.0` serves the network. `--port <port>` overrides the published port for `dev` or `preview`; `TESTD_SITE_PORT` and `TESTD_SITE_PREVIEW_PORT` set the defaults. `TESTD_DOCS_PORT` moves the internal docs port, and `TESTD_SITE_IMAGE` or `TESTD_SITE_VOLUME` overrides the image or dependency volume. `./sites/bin/site.sh clean` removes the volumes and `dist/`.

## Build

```sh
bun install
bun run build
bun run preview  # serves the built tree at http://localhost:4173/
bun run check
bun run check:links
```

`bun run build` writes the product site to `dist/` first, then the docs to `dist/docs/`. Deploy `dist/` as one static tree. `bun run preview -- --host 0.0.0.0` exposes the built tree on every interface.

The same work runs in the container, so a host without Bun can still build and check:

```sh
./sites/bin/site.sh build    # writes dist/, owned by the calling user
./sites/bin/site.sh preview  # serves the built tree
./sites/bin/site.sh check
./sites/bin/site.sh links    # checks links in the built tree
./sites/bin/site.sh shell
```

`site.sh build` exports the tree out of its build volume, so `dist/` belongs to the host user. `preview` and `links` read that exported tree.

## Publish

Production is a Cloudflare Pages project built from this repository by Git integration. Every push to `master` publishes; other branches get a preview URL. The dashboard settings:

| Setting | Value |
|---|---|
| Production branch | `master` |
| Build command | `bun install --frozen-lockfile && bun run build` |
| Build output directory | `dist` |
| Root directory | `/` (default) |

The Pages build image includes Bun (1.4.4, the same version as the container image); set `BUN_VERSION` only to pin another one. Vite copies `sites/product/public/_headers` and `_redirects` into `dist/`, and Pages applies both. `testd.dev` is the project's custom domain, so the tree carries no `CNAME` file.

Workers static assets would serve the same tree and also honour `_headers` and `_redirects`, but they need a `wrangler.jsonc`, a deploy command, and a Worker name that matches the project. Choose Workers when the site needs code at the edge; until then Pages keeps the configuration in the dashboard and the repository free of deploy files.

CI builds and link-checks `dist/` on every run and does not deploy it; publication belongs to the Pages Git integration alone.

The product version label and checksum link use the published version pinned in root `install.sh`. Development Cargo version changes do not advance the public installer. After a release's assets are public, use the [installer promotion procedure](../docs/developers/releasing.md#cli-installer), then commit and publish the canonical installer change.

## Layout

- `assets/`: canonical brand and social images
- `install.sh`: canonical installer; Vite serves and publishes it at `/install.sh`
- `sites/product/`: product landing page, styles, deploy files, and its Vite config
- `docs/`: documentation Markdown
- `sites/docs/`: VitePress configuration and theme; reads the pages from `docs/` and serves `assets/brand/` as its public directory
- `sites/bin/site.sh`: entry point; picks the runtime and runs one action in a container
- `sites/bin/entrypoint.sh`: installs dependencies in the container, then runs the action
- `sites/bin/dev-servers.sh`: runs stock `vitepress dev` and `vite` inside the container
- `crates/testd-gui/icons/`: generated bundle and tray icons
- `scripts/build-icons.py`: regenerates `crates/testd-gui/icons/` from `assets/brand/`
- `tests/check-links.ts`: Link checker for the built tree
- `dist/`: combined build output, served at the origin root

## Brand assets

`assets/brand/testd-slim.svg` is the slim brand icon used on the product page (inlined in its header so it follows the text colour), in the dashboard and setup headers, and in larger placements. The product site imports the brand files through Vite, and the docs serve `assets/brand/` as their public directory, using `testd-app.png` for both the header logo and the favicon.

`assets/brand/testd-app.png` is the 1024px fat app icon used below 128px, including the favicon and the docs header. `testd-app-large.png` is the 1024px thin render of `testd-slim.svg` used at 128px and above, so the dock and task switch keep the lighter mark. `testd-tray-mask.png` and `testd-tray-plate-mask.png` are the alpha masks for the tray variants. `scripts/build-icons.py` regenerates `crates/testd-gui/icons/` from those sources; the native tray and packager use that directory. `assets/social/og.png` is the social card, emitted at `/og.png` by the product Vite config.

The product site builds on the Elmo St. identity, widened to be comfortable to read and work from. The ground is Night `#05070B`, with two raised surfaces (`#070B11` for code, `#0A0F17` for panels) and three Light text tiers. St. Elmo blue `#5CE1FF` is the interactive colour (focus, the primary button, live lights), with `#8FDDF0` for links; amber `#FF7828` marks the helm and the one caution, the unsigned-alpha notice. Muted teal `#5BB5AA`, violet `#9C92D8`, and sand `#D2B98F` colour the worlds and the code highlighting. The tokens are at the top of `sites/product/style.css`. The footer lockup uses the Tier II maker's mark, `assets/brand/elmo-st-tier-2-shipyard.svg`, which the 404 page shares. Fonts are self-hosted through Fontsource: Alegreya for text, and JetBrains Mono for code and labels. The docs theme still uses Archivo.

The hero and the footer limb are drawn on canvas as fine halftone dots, sampled from procedural planets. `sites/product/dots.ts` is the dot renderer and `noise.ts` and `worlds.ts` shade the worlds; `scene.ts` lays out the host's world, the browser, the four other worlds, and the testd links between them; `lease.ts` animates it and drives the Ctrl+C toggle; `limb.ts` draws the footer's planet. `highlight.ts` colours the shell blocks. The page reads fully without any of them, and all of them are DOM-free apart from `lease.ts`, `limb.ts`, and `highlight.ts`, so a scene can be rendered headlessly.
