/*
	Dots: the one drawing primitive. A scene is sampled on a fine grid and every sample becomes a
	dot whose size follows its tone, like a halftone. All sizes are device pixels, so the pitch is
	the same on every screen. `DotBuffer` is DOM-free so scenes can also be rendered headlessly.
*/

/** Device pixels between dot centres. */
export const PITCH = 3;

/** The largest dot radius: leaves a gap between neighbours even at full tone. */
export const MAX_RADIUS = PITCH * 0.42;

/** Samples dimmer than this leave the ground empty. */
export const MIN_TONE = 0.08;

export type Rgb = readonly [number, number, number];

export interface DotSink {
	dot(x: number, y: number, radius: number, color: Rgb, alpha: number): void;
	glow(x: number, y: number, radius: number, color: Rgb, alpha: number): void;
}

export function hex(value: string): Rgb {
	const bits = Number.parseInt(value.replace('#', ''), 16);
	return [(bits >> 16) & 255, (bits >> 8) & 255, bits & 255];
}

export function mix(from: Rgb, to: Rgb, amount: number): Rgb {
	const t = Math.min(Math.max(amount, 0), 1);
	return [from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t, from[2] + (to[2] - from[2]) * t];
}

export function smoothstep(low: number, high: number, value: number): number {
	const t = Math.min(Math.max((value - low) / (high - low), 0), 1);
	return t * t * (3 - 2 * t);
}

/** Place the dot for a sample of the given tone (0..1), or nothing when it is too dim to show. */
export function toneDot(sink: DotSink, x: number, y: number, tone: number, color: Rgb, alpha = 1): void {
	if (tone < MIN_TONE) return;
	sink.dot(x, y, MAX_RADIUS * Math.sqrt(Math.min(tone, 1)), color, alpha);
}

/** Dots stamped into RGBA pixels with sub-pixel coverage, so even tiny dots stay soft. */
export class DotBuffer implements DotSink {
	readonly data: Uint8ClampedArray<ArrayBuffer>;

	constructor(
		readonly width: number,
		readonly height: number,
	) {
		this.data = new Uint8ClampedArray(width * height * 4);
	}

	dot(x: number, y: number, radius: number, color: Rgb, alpha: number): void {
		const reach = Math.ceil(radius + 1);
		const firstCol = Math.max(0, Math.floor(x) - reach);
		const lastCol = Math.min(this.width - 1, Math.floor(x) + reach);
		const firstRow = Math.max(0, Math.floor(y) - reach);
		const lastRow = Math.min(this.height - 1, Math.floor(y) + reach);
		// A dot smaller than a pixel dims instead of shrinking, as a real small dot would.
		const area = Math.min(1, Math.PI * radius * radius);
		for (let row = firstRow; row <= lastRow; row++) {
			for (let col = firstCol; col <= lastCol; col++) {
				const distance = Math.hypot(col + 0.5 - x, row + 0.5 - y);
				const coverage = Math.min(Math.max(radius + 0.5 - distance, 0), 1) * area * alpha;
				this.blend((row * this.width + col) * 4, color, coverage);
			}
		}
	}

	glow(x: number, y: number, radius: number, color: Rgb, alpha: number): void {
		for (let ring = 6; ring >= 1; ring--) {
			const fraction = ring / 6;
			this.dot(x, y, radius * fraction, color, (alpha * (1 - fraction) ** 2) / 2);
		}
	}

	// Keep the stronger of the existing and incoming coverage; dots rarely overlap at this pitch.
	private blend(offset: number, color: Rgb, coverage: number): void {
		const existing = this.data[offset + 3] / 255;
		if (coverage <= existing) return;
		this.data[offset] = color[0];
		this.data[offset + 1] = color[1];
		this.data[offset + 2] = color[2];
		this.data[offset + 3] = coverage * 255;
	}
}

/** Dots drawn as arcs on a canvas: for the few hundred dots that change every frame. */
export class CanvasSink implements DotSink {
	constructor(private readonly context: CanvasRenderingContext2D) {}

	dot(x: number, y: number, radius: number, color: Rgb, alpha: number): void {
		const { context } = this;
		context.fillStyle = `rgb(${color[0]} ${color[1]} ${color[2]} / ${alpha})`;
		context.beginPath();
		context.arc(x, y, radius, 0, Math.PI * 2);
		context.fill();
	}

	glow(x: number, y: number, radius: number, color: Rgb, alpha: number): void {
		const { context } = this;
		const gradient = context.createRadialGradient(x, y, 0, x, y, radius);
		gradient.addColorStop(0, `rgb(${color[0]} ${color[1]} ${color[2]} / ${alpha})`);
		gradient.addColorStop(1, `rgb(${color[0]} ${color[1]} ${color[2]} / 0)`);
		context.fillStyle = gradient;
		context.fillRect(x - radius, y - radius, radius * 2, radius * 2);
	}
}
