/* The planet limb behind the footer: a wide dotted arc that fades inward. Light only; no blue or amber. */

import { DotGrid } from './dots';

const RIM_HEIGHT = 0.28;
const FADE_ROWS = 0.6;

export function mountLimb(canvas: HTMLCanvasElement): void {
	if (!canvas.getContext) return;

	const grid = new DotGrid(canvas);

	function draw(): void {
		if (!grid.resize()) return;

		const { source, cols, rows } = grid;
		const radius = cols * 0.9;
		const centerX = cols / 2;
		const centerY = rows * RIM_HEIGHT + radius;

		grid.clear();
		source.save();
		source.beginPath();
		source.arc(centerX, centerY, radius, 0, Math.PI * 2);
		source.clip();
		const surface = source.createRadialGradient(centerX, centerY, radius - rows * FADE_ROWS, centerX, centerY, radius);
		surface.addColorStop(0, 'rgb(255 255 255 / 0%)');
		surface.addColorStop(1, 'rgb(255 255 255 / 40%)');
		source.fillStyle = surface;
		source.fillRect(0, 0, cols, rows);
		source.restore();

		source.beginPath();
		source.arc(centerX, centerY, radius, 0, Math.PI * 2);
		source.strokeStyle = 'rgb(255 255 255 / 70%)';
		source.lineWidth = 1;
		source.stroke();
		grid.paint();
	}

	new ResizeObserver(draw).observe(canvas);
}
