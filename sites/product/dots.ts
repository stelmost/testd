/*
	Dot-matrix renderer shared by the lease diagram and the footer limb.
	A scene is drawn into a small offscreen canvas, one pixel per dot cell; each cell is then
	thresholded against a Bayer matrix and painted as a round dot on the visible canvas.
*/

export const CELL = 4;

export const LIGHT = '#ecf2f9';
export const BLUE = '#5ce1ff';
export const AMBER = '#ff7828';

const TAU = Math.PI * 2;
const LIGHT_DOT_ALPHA = 0.62;
const GAIN = 1.5;
const BAYER = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5].map((level) => (level + 0.5) / 16);

type Tint = 'light' | 'blue' | 'amber';

function tintOf(red: number, blue: number): Tint {
	if (red > 200 && blue < 140) return 'amber';
	if (blue > 200 && red < 160) return 'blue';
	return 'light';
}

export class DotGrid {
	readonly source: CanvasRenderingContext2D;
	readonly display: CanvasRenderingContext2D;
	cols = 0;
	rows = 0;
	private width = 0;
	private height = 0;

	constructor(private readonly canvas: HTMLCanvasElement) {
		const offscreen = document.createElement('canvas');
		this.source = offscreen.getContext('2d', { willReadFrequently: true }) as CanvasRenderingContext2D;
		this.display = canvas.getContext('2d') as CanvasRenderingContext2D;
	}

	/** Match both canvases to the element's CSS size. Returns false while the element has no size. */
	resize(): boolean {
		const { width, height } = this.canvas.getBoundingClientRect();
		if (!width || !height) return false;

		const ratio = window.devicePixelRatio || 1;
		this.width = width;
		this.height = height;
		this.canvas.width = Math.round(width * ratio);
		this.canvas.height = Math.round(height * ratio);
		this.display.setTransform(ratio, 0, 0, ratio, 0, 0);
		this.cols = Math.floor(width / CELL);
		this.rows = Math.floor(height / CELL);
		this.source.canvas.width = this.cols;
		this.source.canvas.height = this.rows;
		return true;
	}

	clear(): void {
		this.source.clearRect(0, 0, this.cols, this.rows);
	}

	/** Threshold the offscreen scene and paint it as dots. Labels may be drawn on `display` afterwards. */
	paint(): void {
		const { cols, rows, display } = this;
		const pixels = this.source.getImageData(0, 0, cols, rows).data;
		const paths: Record<Tint, Path2D> = { light: new Path2D(), blue: new Path2D(), amber: new Path2D() };

		for (let row = 0; row < rows; row++) {
			for (let col = 0; col < cols; col++) {
				const offset = (row * cols + col) * 4;
				const alpha = pixels[offset + 3] / 255;
				if (alpha * GAIN <= BAYER[((row & 3) << 2) | (col & 3)]) continue;

				const radius = CELL * (0.2 + 0.2 * Math.min(alpha, 1));
				const x = (col + 0.5) * CELL;
				const y = (row + 0.5) * CELL;
				const path = paths[tintOf(pixels[offset], pixels[offset + 2])];
				path.moveTo(x + radius, y);
				path.arc(x, y, radius, 0, TAU);
			}
		}

		display.clearRect(0, 0, this.width, this.height);
		display.globalAlpha = LIGHT_DOT_ALPHA;
		display.fillStyle = LIGHT;
		display.fill(paths.light);
		display.globalAlpha = 1;
		this.fillGlowing(paths.blue, BLUE);
		this.fillGlowing(paths.amber, AMBER);
	}

	// The helm core and the corona node are the only elements that glow.
	private fillGlowing(path: Path2D, color: string): void {
		const { display } = this;
		display.shadowColor = color;
		display.shadowBlur = 10;
		display.fillStyle = color;
		display.fill(path);
		display.shadowBlur = 0;
	}
}
