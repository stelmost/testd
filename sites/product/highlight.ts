/*
	Shell highlighting for `[data-shell]` blocks: the command, its subcommand, flags, and values.
	The text itself is never changed, so the copy buttons and the page without scripts keep working.
*/

const WORD = /^[a-z][a-z-]*$/;

interface LineState {
	/** Position of the next word on the line: 0 is the command, 1 its subcommand. */
	word: number;
}

function classify(word: string, state: LineState): string | undefined {
	// What follows `|` or `--` starts a new command.
	if (word === '|' || word === '--') {
		state.word = 0;
		return word === '--' ? 'tok-flag' : undefined;
	}
	if (word.startsWith('-')) return 'tok-flag';
	// `sudo` is a prefix: the real command comes next.
	if (word === 'sudo' && state.word === 0) return 'tok-command';

	const position = state.word++;
	if (position === 0) return 'tok-command';
	if (position === 1 && WORD.test(word)) return 'tok-sub';
	return 'tok-value';
}

function highlightText(text: string, state: LineState): DocumentFragment {
	const fragment = document.createDocumentFragment();
	for (const part of text.split(/(\s+)/)) {
		if (!part) continue;
		if (/^\s+$/.test(part)) {
			if (part.includes('\n')) state.word = 0;
			fragment.append(part);
			continue;
		}
		const role = classify(part, state);
		if (!role) {
			fragment.append(part);
			continue;
		}
		const span = document.createElement('span');
		span.className = role;
		span.textContent = part;
		fragment.append(span);
	}
	return fragment;
}

export function highlightShell(block: HTMLElement): void {
	const state: LineState = { word: 0 };
	// Only text nodes are coloured; existing elements, such as comments, stay as written.
	for (const node of Array.from(block.childNodes)) {
		if (node.nodeType === Node.TEXT_NODE) node.replaceWith(highlightText(node.textContent ?? '', state));
	}
}
