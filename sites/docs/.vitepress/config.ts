import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitepress';
import night from './night';

const REPO = 'https://github.com/stelmost/testd';
const DISCORD = 'https://discord.gg/skEnmNGm9G';
const SITE = 'https://testd.dev';
const outDir = process.env.TESTD_SITE_OUT_DIR
	? `${process.env.TESTD_SITE_OUT_DIR}/docs`
	: '../../dist/docs';
const publicDir = fileURLToPath(new URL('../../../assets/brand', import.meta.url));

export default defineConfig({
	lang: 'en-US',
	title: 'testd',
	description: 'Stable HTTPS names for containers, VMs, WSL, and remote dev boxes.',
	base: '/docs/',
	// The Markdown pages live at the repository root; this directory holds the site app.
	srcDir: '../../docs',
	outDir,
	cacheDir: process.env.TESTD_DOCS_CACHE_DIR,
	cleanUrls: false,
	lastUpdated: false,
	// The identity has no light ground, so the docs are dark only and the appearance switch is hidden.
	appearance: 'force-dark',
	markdown: { theme: night },
	titleTemplate: ':title | testd docs',
	head: [
		['link', { rel: 'icon', type: 'image/png', href: '/docs/testd-app.png' }],
		['meta', { name: 'theme-color', content: '#05070b' }],
		['meta', { property: 'og:type', content: 'website' }],
		['meta', { property: 'og:site_name', content: 'testd docs' }],
	],
	themeConfig: {
		logo: { src: '/testd-slim.svg', alt: 'testd', width: 28, height: 28 },
		siteTitle: 'testd docs',
		nav: [
			{ text: 'Product', link: SITE },
			{ text: 'Quickstart', link: '/quickstart' },
			{ text: 'Host', link: '/host' },
			{ text: 'Reference', link: '/reference' },
			{ text: 'GitHub', link: REPO },
		],
		sidebar: [
			{
				text: 'Start',
				items: [
					{ text: 'Overview', link: '/' },
					{ text: 'Quickstart', link: '/quickstart' },
				],
			},
			{
				text: 'Host',
				items: [
					{ text: 'Running the host', link: '/host' },
					{ text: 'System integration', link: '/system' },
				],
			},
			{
				text: 'Workflows',
				items: [
					{ text: 'Local apps', link: '/local' },
					{ text: 'Remote & SSH', link: '/remote' },
					{ text: 'Sharing & tunnels', link: '/sharing' },
				],
			},
			{
				text: 'Reference',
				items: [
					{ text: 'CLI & configuration', link: '/reference' },
					{ text: 'Troubleshooting', link: '/troubleshooting' },
					{ text: 'Security', link: '/security' },
				],
			},
			{
				text: 'Developers',
				collapsed: true,
				items: [
					{ text: 'Overview & build', link: '/developers/' },
					{ text: 'Testing', link: '/developers/testing' },
					{ text: 'Release validation', link: '/developers/release-validation' },
					{ text: 'Releasing', link: '/developers/releasing' },
					{ text: 'Dependency policy', link: '/developers/dependency-policy' },
				],
			},
		],
		socialLinks: [
			{ icon: 'github', link: REPO },
			{ icon: 'discord', link: DISCORD },
		],
		editLink: {
			pattern: `${REPO}/edit/master/docs/:path`,
			text: 'Edit this page',
		},
		outline: { level: [2, 3], label: 'On this page' },
		docFooter: { prev: 'Previous', next: 'Next' },
		search: {
			provider: 'local',
			options: {
				translations: {
					button: {
						buttonText: 'Search docs',
						buttonAriaLabel: 'Search docs',
					},
					modal: {
						displayDetails: 'Display detailed list',
						resetButtonTitle: 'Reset search',
						backButtonTitle: 'Close search',
						noResultsText: 'No results for',
						footer: {
							selectText: 'to select',
							selectKeyAriaLabel: 'enter',
							navigateText: 'to navigate',
							navigateUpKeyAriaLabel: 'up arrow',
							navigateDownKeyAriaLabel: 'down arrow',
							closeText: 'to close',
							closeKeyAriaLabel: 'escape',
						},
					},
				},
			},
		},
		footer: {
			message: 'ISC licensed. No account, no telemetry.',
			copyright: '© 2026 Elmo St. Ltd.',
		},
	},
	vite: {
		publicDir,
		server: {
			host: '127.0.0.1',
			port: Number(process.env.TESTD_DOCS_PORT ?? 5174),
			strictPort: true,
		},
	},
});
