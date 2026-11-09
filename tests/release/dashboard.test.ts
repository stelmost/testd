import { expect, test } from 'bun:test';

function inlineFunction(page: string, name: string, next: string): string {
	return page.slice(page.indexOf(`function ${name}(`), page.indexOf(`\n\tfunction ${next}(`));
}

const dashboard = await Bun.file(new URL('../../crates/testd-daemon/dashboard.html', import.meta.url)).text();
const commandSource = dashboard.slice(dashboard.indexOf('function parseCommand('), dashboard.indexOf('\n\tfunction saveLaunch('));
// Exercise the shipped inline parser, not a copy of its implementation.
const parseCommand = new Function(`return (${commandSource});`)() as (text: string) => string[];

test('quick launches save the selected project directory', async () => {
	const values: Record<string, string> = {
		'launch-command': 'npm run dev',
		'launch-label': 'App',
		'launch-domain': 'app.example',
		'launch-target': '',
		'launch-cwd': '/home/me/project',
	};
	let saved: any;
	const saveLaunch = new Function('document', 'parseCommand', 'post', 'notice', 'refreshState',
		`return (${inlineFunction(dashboard, 'saveLaunch', 'ago')});`)(
		{ getElementById: (id: string) => ({ value: values[id] }) }, parseCommand,
		async (_path: string, body: unknown) => { saved = body; }, () => {}, () => {},
	);
	saveLaunch({ preventDefault() {}, target: { reset() {} } });
	await Promise.resolve();
	expect(saved.cwd).toBe('/home/me/project');
	expect(saved.program).toBe('npm');
	expect(saved.args).toEqual(['run', 'dev']);
	expect(dashboard).toContain('id="launch-cwd"');
});

test('quick launches display captured output as text', () => {
	const elements: any[] = [];
	const document = {
		createElement(tag: string) {
			const element = { tag, textContent: '', append() {}, addEventListener() {} };
			elements.push(element);
			return element;
		},
	};
	const launchRow = new Function('document', 'launchAction',
		`return (${inlineFunction(dashboard, 'launchRow', 'renderLaunches')});`)(document, () => {});
	launchRow({ label: 'App', program: 'npm', running: false, logs: [{ line: '<script>bad</script>' }] });
	expect(elements.find((element) => element.tag === 'pre').textContent).toBe('<script>bad</script>');
});

test('certificate status refreshes on focus and periodically', () => {
	const source = dashboard.slice(dashboard.indexOf('if (dashboardToken) {'),
		dashboard.lastIndexOf('</script>'));
	let refreshes = 0;
	let focused: (() => void) | undefined;
	let tick: (() => void) | undefined;
	new Function('window', 'setInterval', 'certificateState', 'connectEvents', 'dashboardToken', source)(
		{ addEventListener: (_event: string, callback: () => void) => { focused = callback; } },
		(callback: () => void) => { tick = callback; }, () => { refreshes++; }, () => {}, 'fixture-owner',
	);
	focused!();
	tick!();
	expect(refreshes).toBe(3);
});

test('local pages remove fragment credentials and authenticate requests without cross-port cookies', async () => {
	const setup = await Bun.file(new URL('../../crates/testd-daemon/setup.html', import.meta.url)).text();
	for (const [page, tokenName, next] of [[dashboard, 'dashboardToken', 'post'], [setup, 'setupToken', 'request']]) {
		const storage = new Map<string, string>();
		let replaced = '';
		const source = page.slice(page.indexOf('function sessionToken('), page.indexOf(`\n\tconst ${tokenName}`));
		const sessionToken = new Function('location', 'sessionStorage', 'history', 'notice', `return (${source});`)(
			{ hash: '#token=fixture-owner&view=launches', pathname: '/', search: '' },
			{ setItem: (key: string, value: string) => storage.set(key, value), getItem: (key: string) => storage.get(key) },
			{ replaceState: (_state: unknown, _title: string, url: string) => { replaced = url; } }, () => {},
		);
		expect(sessionToken()).toBe('fixture-owner');
		expect(replaced).not.toContain('token=');
		expect([...storage.values()]).toEqual(['fixture-owner']);
		let options: any;
		const fetchSource = page.slice(page.indexOf('function authorizedFetch('), page.indexOf(`\n\tasync function ${next}(`));
		const authorizedFetch = new Function('fetch', tokenName, `return (${fetchSource});`)(
			async (_path: string, value: unknown) => { options = value; }, 'fixture-owner',
		);
		await authorizedFetch('/api/local/state', { headers: { 'X-Testd-Request': '1' } });
		expect(options.headers).toEqual({ Authorization: 'Bearer fixture-owner', 'X-Testd-Request': '1' });
		expect(page).not.toContain('document.cookie');
	}
});

test('setup repair buttons regain their names when a prerequisite disappears', async () => {
	const setup = await Bun.file(new URL('../../crates/testd-daemon/setup.html', import.meta.url)).text();
	const button = { disabled: false, textContent: '' };
	const setReady = new Function('document', `return (${inlineFunction(setup, 'setReady', 'updateConnection')});`)(
		{ getElementById: () => button },
	);
	for (const [id, label] of [['resolver-action', 'Set up resolver'], ['certificate-action', 'Install certificate']]) {
		setReady(id, true);
		expect(button.disabled).toBe(true);
		setReady(id, false);
		expect(button.disabled).toBe(false);
		expect(button.textContent).toBe(label);
	}
});

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
