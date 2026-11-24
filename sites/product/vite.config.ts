import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { defineConfig, type Plugin } from 'vite';

const host = process.env.TESTD_SITE_HOST ?? '127.0.0.1';
const docsPort = Number(process.env.TESTD_DOCS_PORT ?? 5174);
const outDir = process.env.TESTD_SITE_OUT_DIR ?? '../../dist';

const assetsRoot = fileURLToPath(new URL('../../assets/', import.meta.url));

// Brand sources sit outside the Vite root. In dev, point the HTML at Vite's
// /@fs URL so the dev server can read them; the build bundles the same paths.
function devAssets(): Plugin {
	return {
		name: 'testd-dev-assets',
		apply: 'serve',
		transformIndexHtml(html) {
			return html.replaceAll('../../assets/', `/@fs${assetsRoot}`);
		},
	};
}

// Publish canonical files outside the Vite root at stable URLs.
function publicAssets(): Plugin {
	const assets = [
		{
			fileName: 'og.png',
			path: new URL('../../assets/social/og.png', import.meta.url),
			contentType: 'image/png',
		},
		{
			fileName: 'install.sh',
			path: new URL('../../install.sh', import.meta.url),
			contentType: 'text/x-shellscript; charset=utf-8',
		},
	];
	return {
		name: 'testd-public-assets',
		configureServer(server) {
			for (const asset of assets) {
				server.middlewares.use(`/${asset.fileName}`, (_req, res) => {
					res.setHeader('Content-Type', asset.contentType);
					res.end(readFileSync(asset.path));
				});
			}
		},
		generateBundle() {
			for (const asset of assets) {
				this.emitFile({ type: 'asset', fileName: asset.fileName, source: readFileSync(asset.path) });
			}
		},
	};
}

export default defineConfig({
	base: '/',
	plugins: [devAssets(), publicAssets()],
	server: {
		host,
		port: 5173,
		strictPort: true,
		proxy: {
			'/docs': {
				target: `http://127.0.0.1:${docsPort}`,
				changeOrigin: true,
				ws: true,
				bypass(req, res) {
					if (req.url === '/docs') {
						res.writeHead(301, { Location: '/docs/' });
						res.end();
						return false;
					}
				},
			},
		},
	},
	preview: { host, port: 4173, strictPort: true },
	build: {
		outDir,
		emptyOutDir: true,
		target: 'es2022',
		rollupOptions: {
			input: [
				fileURLToPath(new URL('index.html', import.meta.url)),
				fileURLToPath(new URL('404.html', import.meta.url)),
			],
		},
	},
});
