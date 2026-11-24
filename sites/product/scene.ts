/*
	The hero scene: the host's world, a browser in orbit, and four other worlds, each joined to the
	host by one testd link. Pure and DOM-free: layout, link drawing and label placement for any
	`DotSink`, so it can be rendered on a canvas or headlessly.
*/

import { type DotSink, PITCH, type Rgb, mix } from './dots';
import { BLUE, LIGHT, WORLD_COLOR, type World, type WorldKind } from './worlds';

/* Layout is in fractions of the canvas width; tune the scene here. */
const HOST = { x: 0.24, y: 0.66, radius: 0.3 };
const BROWSER = { x: 0.2, y: 0.16 };
/* One star, up and to the right. The host sees it nearly edge-on, a thin crescent that leaves its far side dark. */
const HOST_LIGHT: [number, number, number] = [0.9, 0.3, -0.3];
const LEAF_LIGHT: [number, number, number] = [0.7, 0.45, 0.55];

interface LeafLayout {
	runtime: string;
	name: string;
	transport?: string;
	kind: WorldKind;
	x: number;
	y: number;
	radius: number;
	/** Where the label sits relative to the world, chosen to stay clear of its link. */
	labelSide: 'above' | 'right';
}

const LEAVES: LeafLayout[] = [
	{
		runtime: 'this machine',
		name: 'app.example.test',
		transport: 'loopback',
		kind: 'moon',
		x: 0.56,
		y: 0.22,
		radius: 0.036,
		labelSide: 'above',
	},
	{
		runtime: 'vm · wsl',
		name: 'docs.example.test',
		kind: 'ice',
		x: 0.76,
		y: 0.12,
		radius: 0.045,
		labelSide: 'right',
	},
	{
		runtime: 'container',
		name: 'api.example.test',
		kind: 'giant',
		x: 0.8,
		y: 0.37,
		radius: 0.058,
		labelSide: 'above',
	},
	{
		runtime: 'remote box',
		name: 'console.example.test',
		transport: 'TLS 1.3 · paired',
		kind: 'rocky',
		x: 0.93,
		y: 0.57,
		radius: 0.045,
		labelSide: 'above',
	},
];

export const LEAF_COUNT = LEAVES.length;

/** Index of the leaf the Ctrl+C toggle acts on. */
export const REMOTE_BOX = LEAVES.findIndex((leaf) => leaf.runtime === 'remote box');

export interface Leaf extends LeafLayout {
	world: World;
}

export interface Scene {
	width: number;
	height: number;
	host: World;
	leaves: Leaf[];
	browser: { x: number; y: number };
	/** Device pixels per label font size, and the gap between label lines. */
	fontPx: number;
}

export interface LinkState {
	/** 0 when the link is down, 1 when established. */
	progress: number;
	/** 0..1 while a pulse travels the link; outside that range there is none. */
	pulse: number;
}

export interface Label {
	text: string;
	x: number;
	y: number;
	align: 'left' | 'center' | 'right';
	emphasis: 'strong' | 'dim' | 'off';
}

type Point = [number, number];

const SEAL_EVERY = 16;
const LIVE_AT = 0.98;

export function buildScene(width: number, height: number, fontPx: number): Scene {
	const unit = (vector: World['light']): World['light'] => {
		const strength = Math.hypot(...vector);
		return [vector[0] / strength, vector[1] / strength, vector[2] / strength];
	};
	const world = (kind: WorldKind, x: number, y: number, radius: number, seed: number): World => ({
		kind,
		x: x * width,
		y: y * width,
		radius: radius * width,
		seed,
		light: unit(kind === 'host' ? HOST_LIGHT : LEAF_LIGHT),
		spin: seed * 0.9,
	});
	return {
		width,
		height,
		host: world('host', HOST.x, HOST.y, HOST.radius, 7),
		leaves: LEAVES.map((leaf, index) => ({
			...leaf,
			world: world(leaf.kind, leaf.x, leaf.y, leaf.radius, index + 2),
		})),
		browser: { x: BROWSER.x * width, y: BROWSER.y * width },
		fontPx,
	};
}

function between(from: Point, to: Point, amount: number): Point {
	return [from[0] + (to[0] - from[0]) * amount, from[1] + (to[1] - from[1]) * amount];
}

function towards(from: Point, to: Point): { direction: Point; length: number } {
	const length = Math.hypot(to[0] - from[0], to[1] - from[1]);
	return { direction: [(to[0] - from[0]) / length, (to[1] - from[1]) / length], length };
}

/** A link runs radially from a gate on the host's limb to a gate on the near side of the other world. */
export function linkEnds(scene: Scene, leaf: Leaf): { start: Point; end: Point } {
	const { host } = scene;
	const { direction } = towards([host.x, host.y], [leaf.world.x, leaf.world.y]);
	return {
		start: [host.x + direction[0] * host.radius, host.y + direction[1] * host.radius],
		end: [leaf.world.x - direction[0] * leaf.world.radius, leaf.world.y - direction[1] * leaf.world.radius],
	};
}

function browserEnds(scene: Scene): { start: Point; end: Point } {
	const { host, browser } = scene;
	const { direction } = towards([host.x, host.y], [browser.x, browser.y]);
	return {
		start: [host.x + direction[0] * host.radius, host.y + direction[1] * host.radius],
		end: [browser.x, browser.y + scene.width * 0.012],
	};
}

function ring(sink: DotSink, [x, y]: Point, radius: number, color: Rgb, alpha: number): void {
	const count = Math.max(8, Math.round((Math.PI * 2 * radius) / (PITCH * 1.1)));
	for (let index = 0; index < count; index++) {
		const angle = (index / count) * Math.PI * 2;
		sink.dot(x + Math.cos(angle) * radius, y + Math.sin(angle) * radius, 0.75, color, alpha);
	}
}

// The light that comes on at a gate when the name behind it is live.
function node(sink: DotSink, [x, y]: Point, size: number): void {
	sink.glow(x, y, size * 5, BLUE, 0.55);
	for (const [dx, dy] of [[0, 0], [1, 0], [-1, 0], [0, 1], [0, -1]]) {
		sink.dot(x + dx * size, y + dy * size, size * 0.55, BLUE, 1);
	}
}

interface LinkStyle {
	from: Point;
	to: Point;
	progress: number;
	pulse: number;
	color: Rgb;
	gate: number;
}

function drawLink(sink: DotSink, { from, to, progress, pulse, color, gate }: LinkStyle): void {
	const { direction, length } = towards(from, to);
	const normal: Point = [-direction[1], direction[0]];
	const isLive = progress >= LIVE_AT;
	const reach = Math.max(0, (length - gate * 2) * progress);
	const at = (distance: number): Point => [
		from[0] + direction[0] * (gate + distance),
		from[1] + direction[1] * (gate + distance),
	];

	ring(sink, from, gate, LIGHT, isLive ? 0.8 : 0.3);
	ring(sink, to, gate * 0.7, LIGHT, isLive ? 0.8 : 0.3);
	if (isLive) {
		node(sink, from, PITCH * 0.9);
		node(sink, to, PITCH * 0.5);
	}

	for (let step = 0, distance = 0; distance <= reach; step++, distance = step * PITCH) {
		const [x, y] = at(distance);
		sink.dot(x, y, 0.7, mix(LIGHT, color, distance / Math.max(length, 1)), 0.75);
		if (step % SEAL_EVERY !== 0 || step === 0) continue;
		for (const side of [-1, 1]) {
			const offset = side * PITCH * 1.3;
			sink.dot(x + normal[0] * offset, y + normal[1] * offset, 0.6, LIGHT, 0.45);
		}
	}

	if (pulse <= 0 || pulse >= 1) return;
	for (let trail = 0; trail < 6; trail++) {
		const distance = pulse * reach - trail * PITCH * 1.5;
		if (distance < 0) break;
		const [x, y] = at(distance);
		sink.dot(x, y, 1.25 * (1 - trail * 0.12), BLUE, 1 - trail / 6);
	}
}

// The browser: a small delta-wing craft in orbit above the host.
function craft(sink: DotSink, [x, y]: Point, size: number): void {
	const tip: Point = [x, y - size];
	const left: Point = [x - size * 0.8, y + size * 0.7];
	const right: Point = [x + size * 0.8, y + size * 0.7];
	for (const [from, to] of [[tip, left], [tip, right], [left, right]]) {
		const steps = Math.ceil(Math.hypot(to[0] - from[0], to[1] - from[1]) / PITCH);
		for (let step = 0; step <= steps; step++) {
			const [dotX, dotY] = between(from, to, step / steps);
			sink.dot(dotX, dotY, 0.75, LIGHT, 0.85);
		}
	}
}

/** Draw everything that changes: links, gates, lights, pulses and the browser. */
export function drawLinks(sink: DotSink, scene: Scene, states: LinkState[]): void {
	const gate = scene.width * 0.0085;
	scene.leaves.forEach((leaf, index) => {
		const { start, end } = linkEnds(scene, leaf);
		const { progress, pulse } = states[index];
		drawLink(sink, { from: start, to: end, progress, pulse, color: WORLD_COLOR[leaf.kind], gate });
	});

	const { start, end } = browserEnds(scene);
	drawLink(sink, { from: start, to: end, progress: 1, pulse: -1, color: WORLD_COLOR.host, gate });
	craft(sink, [scene.browser.x, scene.browser.y], scene.width * 0.011);
}

/** Label positions in device pixels. Text is monospace, so widths are exact: 0.6 em per character. */
export function sceneLabels(scene: Scene, states: LinkState[], wide: boolean): Label[] {
	const { fontPx, width, host, browser } = scene;
	const line = fontPx * 1.4;
	const pad = fontPx * 1.2;
	const labels: Label[] = [];
	const add = (text: string, x: number, y: number, align: Label['align'], emphasis: Label['emphasis']): void => {
		labels.push({ text, x, y, align, emphasis });
	};

	// The host's far side is dark, so its label sits there without crossing a link.
	const night = host.y - host.radius * 0.42;
	add('testd host', width * 0.035, night, 'left', 'strong');
	if (wide) {
		add('*.test DNS', width * 0.035, night + line, 'left', 'dim');
		add('HTTPS edge · proxy', width * 0.035, night + line * 2, 'left', 'dim');
	}

	const craftText = wide ? ['browser', 'https://app.example.test'] : ['browser'];
	craftText.forEach((text, row) => {
		add(text, browser.x + width * 0.026, browser.y + line * (row - 0.3), 'left', 'dim');
	});

	scene.leaves.forEach((leaf, index) => {
		const { world } = leaf;
		const texts = wide ? [leaf.name, leaf.runtime] : [leaf.runtime];
		const widest = Math.max(...texts.map((text) => text.length)) * fontPx * 0.6;
		const isRight = leaf.labelSide === 'right';
		const x = isRight
			? Math.min(world.x + world.radius * 1.3, width - pad - widest)
			: Math.min(Math.max(world.x, pad + widest / 2), width - pad - widest / 2);
		const firstBaseline = isRight
			? world.y - (line * (texts.length - 1)) / 2 + fontPx * 0.35
			: world.y - world.radius * 1.25 - line * (texts.length - 1);
		texts.forEach((text, row) => {
			const isName = wide && row === 0;
			const emphasis = isName ? (states[index].progress < 0.5 ? 'off' : 'strong') : 'dim';
			add(text, x, firstBaseline + line * row, isRight ? 'left' : 'center', emphasis);
		});
		if (!wide || !leaf.transport) return;
		// Above the link's midpoint, on its upper side.
		const { start, end } = linkEnds(scene, leaf);
		const [midX, midY] = between(start, end, 0.5);
		const { direction } = towards(start, end);
		const side = direction[0] >= 0 ? -1 : 1;
		const labelX = midX + direction[1] * side * line;
		const labelY = midY - Math.abs(direction[0]) * line * 0.9;
		add(leaf.transport, labelX, labelY, 'center', 'dim');
	});
	return labels;
}
