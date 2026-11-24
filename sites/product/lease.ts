/*
	The hero: a world per machine, joined to the host by testd links. The worlds are drawn once on a
	static canvas; links, gates, lights and labels on a second, redrawn only when a link comes up or
	drops; and the light that bounces along each live link on a third, redrawn every frame.
	Ctrl+C on the remote box drops its link, and the name behind it goes dark.
*/

import { CanvasSink, DotBuffer } from './dots';
import {
	LEAF_COUNT,
	type LinkState,
	REMOTE_BOX,
	type Scene,
	buildScene,
	drawBounce,
	drawLinks,
	sceneLabels,
} from './scene';
import { renderWorld } from './worlds';

const MONO = '"JetBrains Mono Variable", "JetBrains Mono", ui-monospace, monospace';
const TEXT: Record<'strong' | 'dim' | 'off', string> = {
	strong: 'rgb(236 242 249 / 92%)',
	dim: 'rgb(236 242 249 / 56%)',
	off: 'rgb(236 242 249 / 28%)',
};
const WIDE_LABELS_FROM = 560;
const LINK_SPEED = 1.4;
const LOAD_DELAY = 500;
const LOAD_STAGGER = 380;

interface Link extends LinkState {
	target: number;
	startAt: number;
	/** Seconds the link has been up, which times its bouncing light; negative while it is down. */
	live: number;
}

function findParts(figure: HTMLElement) {
	const stage = figure.querySelector<HTMLElement>('[data-lease-stage]');
	const worldCanvas = figure.querySelector<HTMLCanvasElement>('[data-lease-world]');
	const linkCanvas = figure.querySelector<HTMLCanvasElement>('[data-lease-links]');
	const motionCanvas = figure.querySelector<HTMLCanvasElement>('[data-lease-motion]');
	const toggle = figure.querySelector<HTMLButtonElement>('[data-lease-toggle]');
	const status = figure.querySelector<HTMLElement>('[data-lease-status]');
	const worldContext = worldCanvas?.getContext('2d');
	const linkContext = linkCanvas?.getContext('2d');
	const motionContext = motionCanvas?.getContext('2d');
	if (!stage || !worldCanvas || !linkCanvas || !motionCanvas || !toggle || !status) return undefined;
	if (!worldContext || !linkContext || !motionContext) return undefined;
	const canvases = [worldCanvas, linkCanvas, motionCanvas];
	return { stage, canvases, toggle, status, worldContext, linkContext, motionContext };
}

export function mountLease(figure: HTMLElement): void {
	const parts = findParts(figure);
	if (!parts) return;

	const { stage, canvases, toggle, status, worldContext, linkContext, motionContext } = parts;
	const calm = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
	const restingStatus = status.textContent ?? '';
	const restingToggle = toggle.textContent ?? '';
	const started = performance.now();
	const links: Link[] = Array.from({ length: LEAF_COUNT }, (_, index) => ({
		progress: calm ? 1 : 0,
		target: 1,
		startAt: started + LOAD_DELAY + index * LOAD_STAGGER,
		live: -1,
	}));
	let browserLive = -1;
	let scene: Scene | undefined;
	let wide = true;
	let isVisible = true;
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

	// Reduced motion keeps the links but leaves the light out.
	function drawMotion(): void {
		if (!scene) return;
		motionContext.clearRect(0, 0, scene.width, scene.height);
		if (!calm) drawBounce(new CanvasSink(motionContext), scene, [...links.map((link) => link.live), browserLive]);
	}

	function layout(): void {
		const { width: cssWidth, height: cssHeight } = stage.getBoundingClientRect();
		if (!cssWidth || !cssHeight) return;

		const ratio = window.devicePixelRatio || 1;
		const [width, height] = [Math.round(cssWidth * ratio), Math.round(cssHeight * ratio)];
		if (scene?.width === width && scene.height === height) return;

		wide = cssWidth >= WIDE_LABELS_FROM;
		for (const canvas of canvases) {
			canvas.width = width;
			canvas.height = height;
		}
		scene = buildScene(width, height, (wide ? 11 : 10) * ratio);
		drawWorlds(scene);
		draw();
		drawMotion();
		figure.classList.add('is-ready');
	}

	/** Moves a link towards its target and times its light; true when it changed what draw() shows. */
	function advance(link: Link, elapsed: number, now: number): boolean {
		if (now < link.startAt) return false;

		const distance = link.target - link.progress;
		const before = link.progress;
		link.progress += Math.sign(distance) * Math.min(Math.abs(distance), LINK_SPEED * elapsed);
		if (link.progress < 1) link.live = -1;
		else link.live = link.live < 0 ? 0 : link.live + elapsed;
		return link.progress !== before;
	}

	function step(now: number): void {
		const elapsed = Math.max(0, Math.min((now - previous) / 1000, 0.05));
		previous = now;
		const hasChanged = links.map((link) => advance(link, elapsed, now)).some(Boolean);
		if (now - started >= LOAD_DELAY) browserLive = browserLive < 0 ? 0 : browserLive + elapsed;
		if (hasChanged) draw();
		drawMotion();
		frame = isVisible ? requestAnimationFrame(step) : 0;
	}

	// A link's light bounces for as long as the hero is on screen, so there is always a next frame.
	function animate(): void {
		if (calm) {
			for (const link of links) link.progress = link.target;
			draw();
			return;
		}
		if (frame || !isVisible) return;
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
		animate();
	});

	new IntersectionObserver(([entry]) => {
		isVisible = entry.isIntersecting;
		if (isVisible) animate();
	}).observe(stage);
	new ResizeObserver(() => requestAnimationFrame(layout)).observe(stage);
	void document.fonts.load(`11px ${MONO}`).then(draw);
	layout();
	animate();
}
