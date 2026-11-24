/*
	The hero: a world per machine, joined to the host by testd links. The worlds are drawn once on a
	static canvas; links, gates, lights, pulses and labels are redrawn on a second canvas as they change.
	Ctrl+C on the remote box drops its link, and the name behind it goes dark.
*/

import { CanvasSink, DotBuffer } from './dots';
import { LEAF_COUNT, type LinkState, REMOTE_BOX, type Scene, buildScene, drawLinks, sceneLabels } from './scene';
import { renderWorld } from './worlds';

const MONO = '"JetBrains Mono Variable", "JetBrains Mono", ui-monospace, monospace';
const TEXT: Record<'strong' | 'dim' | 'off', string> = {
	strong: 'rgb(236 242 249 / 92%)',
	dim: 'rgb(236 242 249 / 56%)',
	off: 'rgb(236 242 249 / 28%)',
};
const WIDE_LABELS_FROM = 560;
const LINK_SPEED = 1.4;
const PULSE_SECONDS = 1.2;
const LOAD_DELAY = 500;
const LOAD_STAGGER = 380;

interface Link extends LinkState {
	target: number;
	startAt: number;
}

function findParts(figure: HTMLElement) {
	const stage = figure.querySelector<HTMLElement>('[data-lease-stage]');
	const worldCanvas = figure.querySelector<HTMLCanvasElement>('[data-lease-world]');
	const linkCanvas = figure.querySelector<HTMLCanvasElement>('[data-lease-links]');
	const toggle = figure.querySelector<HTMLButtonElement>('[data-lease-toggle]');
	const status = figure.querySelector<HTMLElement>('[data-lease-status]');
	const worldContext = worldCanvas?.getContext('2d');
	const linkContext = linkCanvas?.getContext('2d');
	if (!stage || !worldCanvas || !linkCanvas || !toggle || !status || !worldContext || !linkContext) return undefined;
	return { stage, worldCanvas, linkCanvas, toggle, status, worldContext, linkContext };
}

export function mountLease(figure: HTMLElement): void {
	const parts = findParts(figure);
	if (!parts) return;

	const { stage, worldCanvas, linkCanvas, toggle, status, worldContext, linkContext } = parts;
	const calm = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
	const restingStatus = status.textContent ?? '';
	const restingToggle = toggle.textContent ?? '';
	const started = performance.now();
	const links: Link[] = Array.from({ length: LEAF_COUNT }, (_, index) => ({
		progress: calm ? 1 : 0,
		pulse: -1,
		target: 1,
		startAt: started + LOAD_DELAY + index * LOAD_STAGGER,
	}));
	let scene: Scene | undefined;
	let wide = true;
	let frame = 0;
	let previous = 0;

	function drawWorlds(current: Scene): void {
		const buffer = new DotBuffer(current.width, current.height);
		renderWorld(buffer, current.host, current.width, current.height);
		for (const leaf of current.leaves) renderWorld(buffer, leaf.world, current.width, current.height);
		worldContext.putImageData(new ImageData(buffer.data, current.width, current.height), 0, 0);
	}

	function draw(): void {
		if (!scene) return;
		linkContext.clearRect(0, 0, scene.width, scene.height);
		drawLinks(new CanvasSink(linkContext), scene, links);
		linkContext.font = `${scene.fontPx}px ${MONO}`;
		linkContext.textBaseline = 'alphabetic';
		for (const label of sceneLabels(scene, links, wide)) {
			linkContext.textAlign = label.align;
			linkContext.fillStyle = TEXT[label.emphasis];
			linkContext.fillText(label.text, label.x, label.y);
		}
	}

	function layout(): void {
		const { width: cssWidth, height: cssHeight } = stage.getBoundingClientRect();
		if (!cssWidth || !cssHeight) return;

		const ratio = window.devicePixelRatio || 1;
		const [width, height] = [Math.round(cssWidth * ratio), Math.round(cssHeight * ratio)];
		if (scene?.width === width && scene.height === height) return;

		wide = cssWidth >= WIDE_LABELS_FROM;
		for (const canvas of [worldCanvas, linkCanvas]) {
			canvas.width = width;
			canvas.height = height;
		}
		scene = buildScene(width, height, (wide ? 11 : 10) * ratio);
		drawWorlds(scene);
		draw();
		figure.classList.add('is-ready');
	}

	function advance(link: Link, elapsed: number, now: number): boolean {
		if (now < link.startAt) return true;

		const distance = link.target - link.progress;
		const before = link.progress;
		link.progress += Math.sign(distance) * Math.min(Math.abs(distance), LINK_SPEED * elapsed);
		if (before < 1 && link.progress === 1) link.pulse = 0;
		if (link.pulse >= 0) {
			link.pulse += elapsed / PULSE_SECONDS;
			if (link.pulse >= 1) link.pulse = -1;
		}
		return link.progress !== link.target || link.pulse >= 0;
	}

	function step(now: number): void {
		const elapsed = Math.min((now - previous) / 1000, 0.05);
		previous = now;
		const moving = links.map((link) => advance(link, elapsed, now)).some(Boolean);
		draw();
		frame = moving ? requestAnimationFrame(step) : 0;
	}

	function animate(): void {
		if (calm) {
			for (const link of links) link.progress = link.target;
			draw();
			return;
		}
		if (frame) return;
		previous = performance.now();
		frame = requestAnimationFrame(step);
	}

	toggle.addEventListener('click', () => {
		const isStopped = toggle.getAttribute('aria-pressed') !== 'true';
		toggle.setAttribute('aria-pressed', String(isStopped));
		toggle.textContent = isStopped ? (toggle.dataset.pressedLabel ?? restingToggle) : restingToggle;
		status.textContent = isStopped ? (status.dataset.pressedText ?? restingStatus) : restingStatus;
		const remote = links[REMOTE_BOX];
		remote.target = isStopped ? 0 : 1;
		remote.startAt = 0;
		remote.pulse = -1;
		animate();
	});

	new ResizeObserver(() => requestAnimationFrame(layout)).observe(stage);
	void document.fonts.load(`11px ${MONO}`).then(draw);
	layout();
	animate();
}
