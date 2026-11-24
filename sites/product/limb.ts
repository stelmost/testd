/*
	The footer's planet limb: the host's world seen from near. The star is behind it, so only a thin
	crescent along the rim is lit and the body stays dark enough for text to sit on.
*/

import { DotBuffer } from './dots';
import { type World, renderWorld } from './worlds';

const RESIZE_DELAY_MS = 150;
/** Where the rim crosses the footer, as a fraction of its height from the top. */
const RIM = 0.2;
/** The radius, in device pixels, at which the surface detail is drawn at its natural scale. */
const REFERENCE_RADIUS = 400;

export function mountLimb(canvas: HTMLCanvasElement): void {
	const context = canvas.getContext('2d');
	if (!context) return;

	let drawnSize = '';
	let timer: number | undefined;

	const draw = (): void => {
		const { width: cssWidth, height: cssHeight } = canvas.getBoundingClientRect();
		if (!cssWidth || !cssHeight) return;

		const ratio = window.devicePixelRatio || 1;
		const [width, height] = [Math.round(cssWidth * ratio), Math.round(cssHeight * ratio)];
		if (drawnSize === `${width}x${height}`) return;

		drawnSize = `${width}x${height}`;
		canvas.width = width;
		canvas.height = height;
		const radius = width * 0.9;
		const world: World = {
			kind: 'host',
			x: width / 2,
			y: height * RIM + radius,
			radius,
			seed: 11,
			light: [0, 0.3, -0.95],
			spin: 1.7,
			detail: radius / REFERENCE_RADIUS,
			gain: 0.6,
		};
		const buffer = new DotBuffer(width, height);
		renderWorld(buffer, world, width, height);
		context.putImageData(new ImageData(buffer.data, width, height), 0, 0);
	};

	new ResizeObserver(() => {
		window.clearTimeout(timer);
		timer = window.setTimeout(draw, RESIZE_DELAY_MS);
	}).observe(canvas);
	draw();
}
