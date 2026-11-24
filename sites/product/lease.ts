/*
	The lease diagram: a browser, the testd host, and four dev environments, each holding one
	connection open to the host. Ending a connection releases its name, which the toggle shows.
*/

import { AMBER, CELL, DotGrid } from './dots';

type Vec = [number, number, number];
type Cell = [number, number];
type Edge = [Vec, Vec];
interface Face {
	corners: Vec[];
	alpha: number;
}
interface Shape {
	edges: Edge[];
	faces: Face[];
}
interface Leaf {
	runtime: string;
	name: string;
	center: Vec;
	size: Vec;
	port: Vec;
	progress: number;
	target: number;
	startAt: number;
}

const YAW = (-20 * Math.PI) / 180;
const TILT = (28 * Math.PI) / 180;
const MONO = '"JetBrains Mono Variable", "JetBrains Mono", ui-monospace, monospace';
const TEXT = 'rgb(236 242 249 / 90%)';
const TEXT_DIM = 'rgb(236 242 249 / 56%)';
const TEXT_OFF = 'rgb(236 242 249 / 28%)';
const WIDE_LABELS_FROM = 560;
const TETHER_SPEED = 1.4;
const LOAD_STAGGER = 350;
const TICKS = 8;

/* World units: x along the hull, y up, z towards the leaves. */

function project([x, y, z]: Vec): Cell {
	const depth = x * Math.sin(YAW) + z * Math.cos(YAW);
	return [x * Math.cos(YAW) - z * Math.sin(YAW), y * Math.cos(TILT) - depth * Math.sin(TILT)];
}

const FACE_CORNERS = [
	[0, 2, 6, 4],
	[1, 3, 7, 5],
	[0, 1, 5, 4],
	[2, 3, 7, 6],
	[0, 1, 3, 2],
	[4, 5, 7, 6],
];
// -x, +x, -y, +y, -z, +z: faces turned towards the camera are filled more strongly.
const FACE_ALPHA = [0.08, 0.03, 0.02, 0.16, 0.03, 0.1];

function box(min: Vec, max: Vec): Shape {
	const corner = (index: number): Vec => [
		index & 1 ? max[0] : min[0],
		index & 2 ? max[1] : min[1],
		index & 4 ? max[2] : min[2],
	];
	const edges: Edge[] = [];
	for (let index = 0; index < 8; index++) {
		for (const bit of [1, 2, 4]) if (!(index & bit)) edges.push([corner(index), corner(index | bit)]);
	}
	const faces = FACE_CORNERS.map((indices, side) => ({ corners: indices.map(corner), alpha: FACE_ALPHA[side] }));
	return { edges, faces };
}

function boxAround([x, y, z]: Vec, [width, height, depth]: Vec): Shape {
	return box([x - width / 2, y - height / 2, z - depth / 2], [x + width / 2, y + height / 2, z + depth / 2]);
}

function loop(points: Vec[]): Edge[] {
	return points.map((point, index) => [point, points[(index + 1) % points.length]]);
}

// A circle in the y-z plane, so seen from the side it is the hull's wheel.
function circle([x, y, z]: Vec, radius: number, segments: number): Edge[] {
	const around = (index: number): Vec => {
		const angle = (index / segments) * Math.PI * 2;
		return [x, y + radius * Math.cos(angle), z + radius * Math.sin(angle)];
	};
	return loop(Array.from({ length: segments }, (_, index) => around(index)));
}

function join(...shapes: Shape[]): Shape {
	return { edges: shapes.flatMap((shape) => shape.edges), faces: shapes.flatMap((shape) => shape.faces) };
}

const PORT_X = [-36, -12, 12, 36];
const WHEEL_CENTER: Vec = [-64, 0, 0];
const BRIDGE_CORE: Vec = [36, 11.5, 0];

function buildHost(): Shape {
	const ribs = [-30, -10, 10, 30].flatMap((x) =>
		loop([
			[x, -7, -9],
			[x, 7, -9],
			[x, 7, 9],
			[x, -7, 9],
		]),
	);
	const spokes: Edge[] = [0, 1, 2, 3].map((index) => {
		const angle = (index * Math.PI) / 2 + Math.PI / 4;
		const [x, y, z] = WHEEL_CENTER;
		return [WHEEL_CENTER, [x, y + 16 * Math.cos(angle), z + 16 * Math.sin(angle)]];
	});
	const strut: Edge = [[-50, 0, 0], WHEEL_CENTER];
	const wheel: Shape = {
		edges: [...ribs, ...circle(WHEEL_CENTER, 24, 28), ...circle(WHEEL_CENTER, 16, 20), ...spokes, strut],
		faces: [],
	};
	const ports = PORT_X.map((x) => box([x - 3, -3, 9], [x + 3, 3, 13]));
	return join(box([-50, -7, -9], [50, 7, 9]), box([26, 7, -6], [46, 16, 6]), ...ports, wheel);
}

const BROWSER = { left: -26, right: 10, bottom: -4, top: 20, z: -62 };

function buildBrowser(): Shape {
	const { left, right, bottom, top, z } = BROWSER;
	const corners: Vec[] = [
		[left, bottom, z],
		[right, bottom, z],
		[right, top, z],
		[left, top, z],
	];
	const bar: Edge = [
		[left, top - 5, z],
		[right, top - 5, z],
	];
	const address: Edge = [
		[left + 10, top - 2.5, z],
		[right - 4, top - 2.5, z],
	];
	return { edges: [...loop(corners), bar, address], faces: [{ corners, alpha: 0.1 }] };
}

const BROWSER_LINK_FROM: Vec = [-8, BROWSER.bottom, BROWSER.z];
const BROWSER_LINK_TO: Vec = [-8, 0, -9];

function buildLeaves(): Leaf[] {
	const definitions: Pick<Leaf, 'runtime' | 'name' | 'center' | 'size'>[] = [
		{ runtime: 'this machine', name: 'app.example.test', center: [-54, -2, 62], size: [14, 8, 10] },
		{ runtime: 'container', name: 'api.example.test', center: [-20, 4, 80], size: [10, 10, 10] },
		{ runtime: 'vm · wsl', name: 'docs.example.test', center: [24, 0, 64], size: [16, 8, 12] },
		{ runtime: 'remote box', name: 'console.example.test', center: [57, 6, 84], size: [9, 13, 9] },
	];
	return definitions.map((definition, index) => ({
		...definition,
		port: [PORT_X[index], 0, 13],
		progress: 0,
		target: 1,
		startAt: 0,
	}));
}

const REMOTE_BOX = 3;

function attachPoint(leaf: Leaf): Vec {
	return [leaf.center[0], leaf.center[1], leaf.center[2] - leaf.size[2] / 2];
}

function lerp(from: Vec, to: Vec, amount: number): Vec {
	return [0, 1, 2].map((axis) => from[axis] + (to[axis] - from[axis]) * amount) as Vec;
}

function clamp01(value: number): number {
	return Math.min(Math.max(value, 0), 1);
}

type ToCell = (point: Vec) => Cell;

// Scale and centre the scene so every static point sits inside the grid, with room for labels.
function fitView(points: Vec[], cols: number, rows: number, wide: boolean): ToCell {
	const flat = points.map(project);
	const xs = flat.map(([x]) => x);
	const ys = flat.map(([, y]) => y);
	const [minX, maxX, minY, maxY] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
	const padX = 6;
	const padTop = 4;
	const padBottom = wide ? 12 : 9;
	const scale = Math.min((cols - padX * 2) / (maxX - minX), (rows - padTop - padBottom) / (maxY - minY));
	const offsetX = (cols - (maxX - minX) * scale) / 2;
	const offsetY = padTop + (rows - padTop - padBottom - (maxY - minY) * scale) / 2;
	return (point) => {
		const [x, y] = project(point);
		return [offsetX + (x - minX) * scale, offsetY + (maxY - y) * scale];
	};
}

export function mountLease(figure: HTMLElement): void {
	const canvas = figure.querySelector<HTMLCanvasElement>('[data-lease-canvas]');
	const toggle = figure.querySelector<HTMLButtonElement>('[data-lease-toggle]');
	const status = figure.querySelector<HTMLElement>('[data-lease-status]');
	if (!canvas || !toggle || !status || !canvas.getContext) return;

	const grid = new DotGrid(canvas);
	const host = buildHost();
	const browser = buildBrowser();
	const leaves = buildLeaves();
	const leafShapes = leaves.map((leaf) => boxAround(leaf.center, leaf.size));
	const calm = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
	const restingStatus = status.textContent ?? '';
	const restingToggle = toggle.textContent ?? '';
	const staticPoints = [browser, host, ...leafShapes].flatMap((shape) => shape.edges.flat());
	let toCell: ToCell = () => [0, 0];
	let wide = true;
	let frame = 0;
	let previous = 0;

	function fillFaces(shapes: Shape[]): void {
		const { source } = grid;
		for (const { faces } of shapes) {
			for (const face of faces) {
				source.beginPath();
				face.corners.forEach((corner, index) => {
					const [x, y] = toCell(corner);
					if (index === 0) source.moveTo(x, y);
					else source.lineTo(x, y);
				});
				source.closePath();
				source.fillStyle = `rgb(255 255 255 / ${face.alpha})`;
				source.fill();
			}
		}
	}

	function strokeLine(from: Vec, to: Vec, withTicks: boolean, reach = 1): void {
		const { source } = grid;
		const end = lerp(from, to, reach);
		source.moveTo(...toCell(from));
		source.lineTo(...toCell(end));
		if (!withTicks) return;
		for (let tick = 1; tick < TICKS; tick++) {
			if (tick / TICKS > reach) break;
			const at = lerp(from, to, tick / TICKS);
			source.moveTo(...toCell([at[0], at[1] - 1.4, at[2]]));
			source.lineTo(...toCell([at[0], at[1] + 1.4, at[2]]));
		}
	}

	function strokeStructure(): void {
		const { source } = grid;
		source.beginPath();
		for (const shape of [browser, host, ...leafShapes]) {
			for (const [from, to] of shape.edges) strokeLine(from, to, false);
		}
		strokeLine(BROWSER_LINK_FROM, BROWSER_LINK_TO, true);
		for (const leaf of leaves) strokeLine(leaf.port, attachPoint(leaf), true, leaf.progress);
		source.strokeStyle = 'rgb(255 255 255 / 90%)';
		source.lineWidth = 1;
		source.stroke();
	}

	function fillDiamond(center: Cell, radius: number, color: string): void {
		const { source } = grid;
		source.beginPath();
		source.moveTo(center[0], center[1] - radius);
		source.lineTo(center[0] + radius, center[1]);
		source.lineTo(center[0], center[1] + radius);
		source.lineTo(center[0] - radius, center[1]);
		source.closePath();
		source.fillStyle = color;
		source.fill();
	}

	function fillLights(): void {
		const { source } = grid;
		for (const leaf of leaves) {
			const lit = clamp01((leaf.progress - 0.9) / 0.1);
			if (lit > 0) fillDiamond(toCell(leaf.port), 3, `rgb(92 225 255 / ${lit})`);
		}
		const [x, y] = toCell(BRIDGE_CORE);
		source.fillStyle = AMBER;
		source.fillRect(Math.round(x) - 2, Math.round(y) - 2, 4, 4);
	}

	function label(lines: [string, string][], at: Vec, align: CanvasTextAlign, dx: number, dy: number): void {
		const { display } = grid;
		const [x, y] = toCell(at);
		display.textAlign = align;
		lines.forEach(([text, color], index) => {
			display.fillStyle = color;
			display.fillText(text, x * CELL + dx, y * CELL + dy + index * (wide ? 15 : 13));
		});
	}

	function drawLabels(): void {
		grid.display.font = `${wide ? 11 : 10}px ${MONO}`;
		const [x, y] = [BROWSER.left, BROWSER.top];
		label([['browser', TEXT_DIM]], [x, y, BROWSER.z], 'left', 0, -8);
		label([['https://app.example.test', TEXT_DIM]], lerp(BROWSER_LINK_FROM, BROWSER_LINK_TO, 0.5), 'left', 12, 4);
		const hostLines: [string, string][] = [['testd host', TEXT]];
		if (wide) hostLines.push(['*.test DNS · HTTPS edge · proxy', TEXT_DIM]);
		label(hostLines, [WHEEL_CENTER[0], 26, 0], 'left', -20, -22);
		leaves.forEach((leaf) => {
			const lines: [string, string][] = [[leaf.runtime, TEXT_DIM]];
			if (wide) lines.unshift([leaf.name, leaf.progress > 0.5 ? TEXT : TEXT_OFF]);
			label(lines, [leaf.center[0], leaf.center[1] - leaf.size[1] / 2, leaf.center[2]], 'center', 0, wide ? 22 : 18);
		});
	}

	function draw(): void {
		if (!grid.cols) return;
		grid.clear();
		fillFaces([browser, host, ...leafShapes]);
		strokeStructure();
		fillLights();
		grid.paint();
		drawLabels();
	}

	function layout(): void {
		if (!grid.resize()) return;
		wide = grid.cols * CELL >= WIDE_LABELS_FROM;
		toCell = fitView(staticPoints, grid.cols, grid.rows, wide);
		draw();
	}

	function step(now: number): void {
		const elapsed = Math.min((now - previous) / 1000, 0.05);
		previous = now;
		let moving = false;
		for (const leaf of leaves) {
			if (now < leaf.startAt) {
				moving = true;
				continue;
			}
			const distance = leaf.target - leaf.progress;
			leaf.progress += Math.sign(distance) * Math.min(Math.abs(distance), TETHER_SPEED * elapsed);
			moving = moving || leaf.progress !== leaf.target;
		}
		draw();
		frame = moving ? requestAnimationFrame(step) : 0;
	}

	function animate(): void {
		if (calm) {
			for (const leaf of leaves) leaf.progress = leaf.target;
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
		const remote = leaves[REMOTE_BOX];
		remote.target = isStopped ? 0 : 1;
		remote.startAt = 0;
		animate();
	});

	const start = performance.now();
	leaves.forEach((leaf, index) => {
		leaf.progress = calm ? 1 : 0;
		leaf.startAt = start + 400 + index * LOAD_STAGGER;
	});

	new ResizeObserver(layout).observe(canvas);
	void document.fonts.load(`11px ${MONO}`).then(draw);
	layout();
	animate();
}

