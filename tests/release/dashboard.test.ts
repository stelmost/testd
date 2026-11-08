import { expect, test } from 'bun:test';

const dashboard = await Bun.file(new URL('../../crates/testd-daemon/dashboard.html', import.meta.url)).text();
const commandSource = dashboard.slice(dashboard.indexOf('function parseCommand('), dashboard.indexOf('\n\tfunction saveLaunch('));
// Exercise the shipped inline parser, not a copy of its implementation.
const parseCommand = new Function(`return (${commandSource});`)() as (text: string) => string[];

test('quick-launch commands preserve Windows executable paths', () => {
	expect(parseCommand(String.raw`"C:\Program Files\nodejs\node.exe" app.js`))
		.toEqual([String.raw`C:\Program Files\nodejs\node.exe`, 'app.js']);
	expect(parseCommand(String.raw`"\\server\share\app.exe"`)).toEqual([String.raw`\\server\share\app.exe`]);
});

test('quick-launch commands preserve quoted and escaped arguments', () => {
	expect(parseCommand(`program 'two words' ""`)).toEqual(['program', 'two words', '']);
	expect(parseCommand(String.raw`program two\ words "say \"hello\""`))
		.toEqual(['program', 'two words', 'say "hello"']);
	expect(() => parseCommand(`program 'unfinished`)).toThrow('Unclosed quote');
});
