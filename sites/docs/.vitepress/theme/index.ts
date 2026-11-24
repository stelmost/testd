import DefaultTheme from 'vitepress/theme';
import type { Theme } from 'vitepress';
import '@fontsource-variable/archivo';
import '@fontsource-variable/jetbrains-mono';
import DocTabs from './DocTabs.vue';
import './custom.css';

function setupImageLightbox(): void {
	let dialog: HTMLDialogElement | null = null;
	let preview: HTMLImageElement | null = null;

	const open = (img: HTMLImageElement): void => {
		if (!dialog) {
			dialog = document.createElement('dialog');
			dialog.className = 'td-lightbox';
			dialog.setAttribute('aria-label', 'Enlarged screenshot');
			preview = document.createElement('img');
			dialog.append(preview);
			dialog.addEventListener('click', () => dialog?.close());
			document.body.append(dialog);
		}
		if (!preview) return;
		preview.src = img.currentSrc || img.src;
		preview.alt = img.alt;
		dialog.showModal();
	};

	document.addEventListener('click', (event) => {
		const target = event.target;
		if (!(target instanceof HTMLImageElement)) return;
		if (!target.closest('.vp-doc')) return;
		open(target);
	});
}

export default {
	extends: DefaultTheme,
	enhanceApp({ app }) {
		app.component('DocTabs', DocTabs);
		if (typeof document !== 'undefined') setupImageLightbox();
	},
} satisfies Theme;
