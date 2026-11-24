/* Seeded 3D value noise: the surface detail the worlds are sampled from. */

function hash(x: number, y: number, z: number, seed: number): number {
	let value =
		Math.imul(x, 374761393) ^ Math.imul(y, 668265263) ^ Math.imul(z, 1274126177) ^ Math.imul(seed, 2246822519);
	value = Math.imul(value ^ (value >>> 13), 1274126177);
	value ^= value >>> 16;
	return (value >>> 0) / 4294967295;
}

function fade(t: number): number {
	return t * t * (3 - 2 * t);
}

function mixAmount(from: number, to: number, amount: number): number {
	return from + (to - from) * amount;
}

/** Smooth noise in 0..1. */
export function noise(x: number, y: number, z: number, seed = 0): number {
	const [ix, iy, iz] = [Math.floor(x), Math.floor(y), Math.floor(z)];
	const [fx, fy, fz] = [fade(x - ix), fade(y - iy), fade(z - iz)];
	const corner = (dx: number, dy: number, dz: number): number => hash(ix + dx, iy + dy, iz + dz, seed);
	const near = mixAmount(
		mixAmount(corner(0, 0, 0), corner(1, 0, 0), fx),
		mixAmount(corner(0, 1, 0), corner(1, 1, 0), fx),
		fy,
	);
	const far = mixAmount(
		mixAmount(corner(0, 0, 1), corner(1, 0, 1), fx),
		mixAmount(corner(0, 1, 1), corner(1, 1, 1), fx),
		fy,
	);
	return mixAmount(near, far, fz);
}

/** Layered noise in 0..1: broad shapes first, finer detail on top. */
export function fbm(x: number, y: number, z: number, seed = 0, octaves = 5): number {
	let sum = 0;
	let weight = 0.5;
	let frequency = 1;
	let total = 0;
	for (let octave = 0; octave < octaves; octave++) {
		sum += weight * noise(x * frequency, y * frequency, z * frequency, seed + octave * 17);
		total += weight;
		weight *= 0.5;
		frequency *= 2.03;
	}
	return sum / total;
}

/** Sharp creases in 0..1, for crater rims and ridges. */
export function ridged(x: number, y: number, z: number, seed = 0, octaves = 4): number {
	return 1 - Math.abs(fbm(x, y, z, seed, octaves) * 2 - 1);
}
