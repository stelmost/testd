/*
	Worlds: procedural planets, sampled on the dot grid. Each one is shaded like a high-definition
	model (sphere normal, soft terminator, fractal surface detail), then every sample becomes a dot.
	Pure and DOM-free: it renders into any `DotSink`.
*/

import { type DotSink, PITCH, type Rgb, hex, mix, smoothstep, toneDot } from './dots';
import { fbm, noise, ridged } from './noise';

export type WorldKind = 'host' | 'moon' | 'giant' | 'ice' | 'rocky';
type Vec3 = [number, number, number];

export interface World {
	kind: WorldKind;
	/** Centre and radius in device pixels. */
	x: number;
	y: number;
	radius: number;
	seed: number;
	/** Unit vector towards the light: x right, y up, z towards the viewer. */
	light: Vec3;
	/** Rotation of the surface about the vertical axis, in radians. */
	spin: number;
	/** Multiplies the surface detail's frequency: a world seen close up needs finer detail. Default 1. */
	detail?: number;
	/** Multiplies every tone, to dim a world. Default 1. */
	gain?: number;
}

export const LIGHT = hex('#ecf2f9');
export const BLUE = hex('#5ce1ff');
export const AMBER = hex('#ff7828');
const TEAL = hex('#5bb5aa');
const VIOLET = hex('#9c92d8');
const SAND = hex('#d2b98f');
const OCEAN = hex('#3f8fb8');
const DEEP_VIOLET = hex('#5b5390');
const DEEP_SAND = hex('#8a7551');

/** The hue each world is drawn in; the scene reuses it for that world's link. */
export const WORLD_COLOR: Record<WorldKind, Rgb> = {
	host: OCEAN,
	moon: LIGHT,
	giant: VIOLET,
	ice: TEAL,
	rocky: SAND,
};

const AMBIENT = 0.16;
/* Tone curve: a gamma above 1 keeps mid tones sparse, and the gain keeps even full tone delicate. */
const GAMMA = 1.35;
const GAIN = 0.85;
const RING = { tilt: 0.28, rotation: -0.32, inner: 1.38, outer: 2.18 };
const ATMOSPHERE = 0.035;

interface Sample {
	tone: number;
	color: Rgb;
}

interface Surface {
	albedo: number;
	color: Rgb;
	/** 1 where there is land, for city lights. */
	land: number;
}

function scale(point: Vec3, factor: number): Vec3 {
	return [point[0] * factor, point[1] * factor, point[2] * factor];
}

function spun([x, y, z]: Vec3, angle: number): Vec3 {
	return [x * Math.cos(angle) + z * Math.sin(angle), y, -x * Math.sin(angle) + z * Math.cos(angle)];
}

function hostSurface(n: Vec3, seed: number, detail: number): Surface {
	const [x, y, z] = scale(n, 1.7 * detail);
	const height = fbm(x + 3, y, z, seed, 5);
	const land = smoothstep(0.55, 0.59, height);
	const polar = smoothstep(0.78, 0.9, Math.abs(n[1]));
	const [cx, cy, cz] = scale(n, detail);
	const cloud = smoothstep(0.56, 0.8, fbm(cx * 2.8, cy * 5.5, cz * 2.8, seed + 9, 4));
	const surface = mix(OCEAN, SAND, land);
	const albedo = 0.34 + 0.34 * land + (height - 0.5) * 0.5;
	const white = Math.max(polar, cloud * 0.55);
	return { albedo: albedo + (0.92 - albedo) * white, color: mix(surface, LIGHT, white), land };
}

function moonSurface(n: Vec3, seed: number): Surface {
	const craters = smoothstep(0.84, 0.96, ridged(n[0] * 4.4, n[1] * 4.4, n[2] * 4.4, seed, 2));
	const mare = fbm(n[0] * 2.2, n[1] * 2.2, n[2] * 2.2, seed, 4);
	return { albedo: 0.46 + 0.4 * mare - 0.3 * craters, color: LIGHT, land: 0 };
}

function giantSurface(n: Vec3, seed: number): Surface {
	const swirl = fbm(n[0] * 2, n[1] * 0.7, n[2] * 2, seed, 4) * 4.4;
	const band = 0.5 + 0.5 * Math.sin(n[1] * 11 + swirl);
	return { albedo: 0.4 + 0.5 * band, color: mix(DEEP_VIOLET, VIOLET, band), land: 0 };
}

function iceSurface(n: Vec3, seed: number): Surface {
	const frost = fbm(n[0] * 3, n[1] * 3, n[2] * 3, seed, 4);
	const cracks = smoothstep(0.88, 0.98, ridged(n[0] * 6.5, n[1] * 6.5, n[2] * 6.5, seed, 3));
	const polar = smoothstep(0.7, 0.9, Math.abs(n[1]));
	return { albedo: 0.58 + 0.3 * frost - 0.3 * cracks + 0.2 * polar, color: mix(TEAL, LIGHT, polar * 0.6), land: 0 };
}

function rockySurface(n: Vec3, seed: number): Surface {
	const ridges = ridged(n[0] * 2.4, n[1] * 2.4, n[2] * 2.4, seed, 4);
	const dust = fbm(n[0] * 6, n[1] * 6, n[2] * 6, seed + 5, 3);
	return { albedo: 0.34 + 0.5 * ridges + 0.15 * dust, color: mix(DEEP_SAND, SAND, ridges), land: 0 };
}

const SURFACES: Record<WorldKind, (n: Vec3, seed: number, detail: number) => Surface> = {
	host: hostSurface,
	moon: moonSurface,
	giant: giantSurface,
	ice: iceSurface,
	rocky: rockySurface,
};

// The ring, seen as a tilted ellipse about the world's centre. `front` is the half nearer the viewer.
function ringSample(world: World, px: number, py: number): (Sample & { front: boolean }) | undefined {
	const [dx, dy] = [px - world.x, py - world.y];
	const [rx, ry] = [
		dx * Math.cos(RING.rotation) - dy * Math.sin(RING.rotation),
		(dx * Math.sin(RING.rotation) + dy * Math.cos(RING.rotation)) / RING.tilt,
	];
	const reach = Math.hypot(rx, ry) / world.radius;
	if (reach < RING.inner || reach > RING.outer) return undefined;

	const bands = smoothstep(0.25, 0.75, noise(reach * 26, 0, 0, world.seed));
	const gap = 1 - smoothstep(0.015, 0, Math.abs(reach - 1.86)) * 0.9;
	const edge = smoothstep(0, 0.05, reach - RING.inner) * smoothstep(0, 0.05, RING.outer - reach);
	return { tone: (0.22 + 0.68 * bands) * gap * edge, color: SAND, front: ry > 0 };
}

function planetSample(world: World, px: number, py: number): Sample | undefined {
	const nx = (px - world.x) / world.radius;
	const ny = -(py - world.y) / world.radius;
	const flat = nx * nx + ny * ny;
	if (flat > 1) return world.kind === 'host' ? atmosphereSample(world, flat) : undefined;

	const nz = Math.sqrt(1 - flat);
	const [lx, ly, lz] = world.light;
	const lit = smoothstep(-0.1, 0.42, nx * lx + ny * ly + nz * lz);
	const detail = world.detail ?? 1;
	const surface = SURFACES[world.kind](spun([nx, ny, nz], world.spin), world.seed, detail);
	let tone = surface.albedo * (AMBIENT + (1 - AMBIENT) * lit);
	let color = surface.color;

	if (world.kind === 'host') {
		const rim = (1 - nz) ** 3;
		tone += rim * 0.55 * lit;
		color = mix(color, BLUE, rim * 1.3);
		const city = surface.land * (1 - smoothstep(0, 0.3, lit));
		const lights = noise(nx * 70 * detail, ny * 70 * detail, nz * 70 * detail, world.seed + 31);
		if (city > 0 && lights > 0.84) return { tone: 0.75 * city, color: AMBER };
	}
	return { tone, color };
}

// A thin blue halo just beyond the limb, strongest on the lit side.
function atmosphereSample(world: World, flat: number): Sample | undefined {
	const above = (Math.sqrt(flat) - 1) / ATMOSPHERE;
	if (above > 1) return undefined;
	return { tone: (1 - above) ** 2 * 0.5, color: BLUE };
}

export function sampleWorld(world: World, px: number, py: number): Sample | undefined {
	const ring = world.kind === 'giant' ? ringSample(world, px, py) : undefined;
	if (ring?.front) return ring;
	return planetSample(world, px, py) ?? ring;
}

/** Draw one world into the sink, sampling only the part of it that falls inside the frame. */
export function renderWorld(sink: DotSink, world: World, width: number, height: number): void {
	const extent = world.radius * (world.kind === 'giant' ? RING.outer : 1 + ATMOSPHERE) + PITCH;
	const firstCol = Math.max(0, Math.floor((world.x - extent) / PITCH));
	const lastCol = Math.min(Math.ceil(width / PITCH), Math.ceil((world.x + extent) / PITCH));
	const firstRow = Math.max(0, Math.floor((world.y - extent) / PITCH));
	const lastRow = Math.min(Math.ceil(height / PITCH), Math.ceil((world.y + extent) / PITCH));

	for (let row = firstRow; row <= lastRow; row++) {
		for (let col = firstCol; col <= lastCol; col++) {
			const [px, py] = [(col + 0.5) * PITCH, (row + 0.5) * PITCH];
			const sample = sampleWorld(world, px, py);
			if (sample) toneDot(sink, px, py, GAIN * (world.gain ?? 1) * sample.tone ** GAMMA, sample.color, 0.95);
		}
	}
}
