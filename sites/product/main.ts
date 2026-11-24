import './style.css';
import { mountLease } from './lease';
import { mountLimb } from './limb';

document.documentElement.classList.add('js');

const toast = document.querySelector<HTMLElement>('[data-toast]');
let toastTimer: number | undefined;

function showToast(message: string): void {
	if (!toast) return;

	window.clearTimeout(toastTimer);
	toast.textContent = message;
	toast.classList.add('visible');
	toastTimer = window.setTimeout(() => toast.classList.remove('visible'), 2400);
}

function copyWithFallback(text: string): boolean {
	const textarea = document.createElement('textarea');
	textarea.value = text;
	textarea.setAttribute('readonly', '');
	textarea.style.position = 'fixed';
	textarea.style.opacity = '0';
	document.body.append(textarea);
	textarea.select();
	const copied = document.execCommand('copy');
	textarea.remove();
	return copied;
}

async function copyText(text: string): Promise<void> {
	if (navigator.clipboard && window.isSecureContext) {
		await navigator.clipboard.writeText(text);
		return;
	}

	if (!copyWithFallback(text)) throw new Error('Clipboard access is unavailable');
}

function getCommandForButton(button: HTMLButtonElement): string {
	const targetId = button.dataset.copyTarget;
	return (targetId ? document.getElementById(targetId)?.textContent : '')?.trim() ?? '';
}

const copyResetTimers = new WeakMap<HTMLButtonElement, number>();

async function handleCopy(button: HTMLButtonElement): Promise<void> {
	const command = getCommandForButton(button);
	const label = button.querySelector<HTMLElement>('.copy-label');
	if (!command) {
		showToast('No command found to copy');
		return;
	}

	window.clearTimeout(copyResetTimers.get(button));

	try {
		await copyText(command);
		if (label) label.textContent = 'Copied';
		showToast('Command copied to clipboard');
	} catch {
		if (label) label.textContent = 'Failed';
		showToast('Copy failed. Select the command manually.');
	}

	const timer = window.setTimeout(() => {
		if (label) label.textContent = 'Copy';
	}, 1800);
	copyResetTimers.set(button, timer);
}

function setupCopyButtons(): void {
	document.querySelectorAll<HTMLButtonElement>('[data-copy-target]').forEach((button) => {
		button.addEventListener('click', () => void handleCopy(button));
	});
}

function detectPlatform(): 'macos' | 'linux' | 'windows' | undefined {
	const ua = `${navigator.platform} ${navigator.userAgent}`.toLowerCase();
	if (ua.includes('win')) return 'windows';
	if (ua.includes('mac')) return 'macos';
	if (ua.includes('linux')) return 'linux';
	return undefined;
}

function setupTabs(): void {
	document.querySelectorAll<HTMLElement>('[role="tablist"]').forEach((tablist) => {
		const tabs = Array.from(tablist.querySelectorAll<HTMLButtonElement>('[role="tab"]'));

		const activate = (selected: HTMLButtonElement): void => {
			tabs.forEach((tab) => {
				const isSelected = tab === selected;
				tab.setAttribute('aria-selected', String(isSelected));
				tab.tabIndex = isSelected ? 0 : -1;
				const panelId = tab.getAttribute('aria-controls');
				const panel = panelId ? document.getElementById(panelId) : null;
				if (panel) panel.hidden = !isSelected;
			});
		};

		tabs.forEach((tab, index) => {
			tab.addEventListener('click', () => activate(tab));
			tab.addEventListener('keydown', (event) => {
				let nextIndex: number | undefined;
				if (event.key === 'ArrowRight') nextIndex = (index + 1) % tabs.length;
				if (event.key === 'ArrowLeft') nextIndex = (index - 1 + tabs.length) % tabs.length;
				if (event.key === 'Home') nextIndex = 0;
				if (event.key === 'End') nextIndex = tabs.length - 1;
				if (nextIndex === undefined) return;

				event.preventDefault();
				activate(tabs[nextIndex]);
				tabs[nextIndex].focus();
			});
		});

		const os = detectPlatform();
		const osTab = os ? tabs.find((tab) => tab.dataset.osTab === os) : undefined;
		if (osTab) activate(osTab);
	});
}

setupCopyButtons();
setupTabs();

const lease = document.querySelector<HTMLElement>('[data-lease]');
if (lease) mountLease(lease);

const limb = document.querySelector<HTMLCanvasElement>('[data-limb]');
if (limb) mountLimb(limb);

document.querySelectorAll<HTMLElement>('[data-year]').forEach((element) => {
	element.textContent = String(new Date().getFullYear());
});
