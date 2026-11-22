import { expect, test } from 'bun:test';

function inlineFunction(page: string, name: string, next: string): string {
	return page.slice(page.indexOf(`function ${name}(`), page.indexOf(`\n\tfunction ${next}(`));
}

const dashboard = await Bun.file(new URL('../../crates/testd-daemon/dashboard.html', import.meta.url)).text();
const commandSource = dashboard.slice(dashboard.indexOf('function parseCommand('), dashboard.indexOf('\n\tfunction saveLaunch('));
// Exercise the shipped inline parser, not a copy of its implementation.
const parseCommand = new Function(`return (${commandSource});`)() as (text: string) => string[];

function submitForm(name: string, values: Record<string, string>) {
	let saved: any;
	const save = new Function('document', 'parseCommand', 'post', 'notice', 'refreshState',
		inlineFunction(dashboard, 'saveLaunch', 'ago') + '\nreturn ' + name + ';')(
		{ getElementById: (id: string) => ({ value: values[id] }) }, parseCommand,
		async (_path: string, body: unknown) => { saved = body; }, () => {}, () => {},
	);
	save({ preventDefault() {}, target: { reset() {} } });
	return saved;
}

test('quick launches save the selected project directory', () => {
	const values: Record<string, string> = {
		'launch-command': 'npm run dev',
		'launch-label': 'App',
		'launch-domain': 'app.example',
		'launch-target': '',
		'launch-cwd': '/home/me/project',
	};
	const saved = submitForm('saveLaunch', values);
	expect(saved.cwd).toBe('/home/me/project');
	expect(saved.program).toBe('npm');
	expect(saved.args).toEqual(['run', 'dev']);
	expect(dashboard).toContain('id="launch-cwd"');
});

test('SSH apps save the alias and remote port', () => {
	const saved = submitForm('saveSshApp', {
		'ssh-mode': 'forward',
		'ssh-label': 'Web',
		'ssh-domain': 'app.example',
		'ssh-host': 'devbox',
		'ssh-port': '3000',
	});
	expect(saved).toEqual({ label: 'Web', entries: [{ domain: 'app.example' }], ssh: { host: 'devbox', port: 3000 } });
});

test('SSH apps save the remote command as argv and its working directory', () => {
	const saved = submitForm('saveSshApp', {
		'ssh-mode': 'command',
		'ssh-label': 'Web',
		'ssh-domain': 'app.example',
		'ssh-host': 'devbox',
		'ssh-port': '3000',
		'ssh-command': 'program "two words" "$literal"',
		'ssh-cwd': '/srv/web app',
	});
	expect(saved.ssh).toEqual({
		host: 'devbox', port: 3000,
		command: { program: 'program', args: ['two words', '$literal'], cwd: '/srv/web app' },
	});
});

test('saved SSH forwards render in SSH apps', () => {
	const groups = new Map();
	const renderLaunches = new Function('renderLaunchRows',
		`return (${inlineFunction(dashboard, 'renderLaunches', 'renderLaunchRows')});`)(
		(id: string, launches: unknown) => groups.set(id, launches),
	);
	const local = { id: 'local', program: 'npm' };
	const remote = { id: 'remote', ssh: { host: 'devbox', port: 3000 } };
	renderLaunches([local, remote]);
	expect(groups.get('launch')).toEqual([local]);
	expect(groups.get('ssh-app')).toEqual([remote]);
});

test('one click opens an SSH app when its registered HTTPS URL is ready', async () => {
	const started: string[] = [];
	const visited: string[] = [];
	const blank = {
		opener: true, document: { title: '', body: { textContent: '' } },
		closed: false, location: { replace: (url: string) => visited.push(url) },
	};
	const start = dashboard.indexOf('const sshTabs =');
	const end = dashboard.indexOf('\n\tfunction sshPromptForm(');
	const openSshLaunch = new Function('window', 'document', 'post', 'getJson',
		dashboard.slice(start, end) + '\nreturn openSshLaunch;')(
		{ open: () => blank }, { getElementById: () => ({ children: [] }) },
		async (_path: string, body: { id: string }) => { started.push(body.id); },
		async () => ({ launches: [{ id: 'one', running: true, url: 'https://app.example.test:8443' }] }),
	);
	await openSshLaunch('one', { disabled: false });
	expect(started).toEqual(['one']);
	expect(blank.opener).toBeNull();
	expect(visited).toEqual(['https://app.example.test:8443']);
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

test('dashboard bootstrap survives unavailable session storage and removes the credential fragment', () => {
	const source = dashboard.slice(dashboard.lastIndexOf('<script>') + '<script>'.length,
		dashboard.indexOf('\n\tfunction setConnection('));
	for (const token of ['fixture-owner', '']) {
		let message = '';
		let replaced = '';
		const notice = {
			className: '',
			replaceChildren(...children: { textContent: string }[]) { message = children[0].textContent; },
		};
		const document = {
			documentElement: { dataset: {} },
			querySelectorAll: () => [],
			getElementById: () => notice,
			createElement: () => ({ textContent: '', setAttribute() {}, addEventListener() {} }),
		};
		const unavailable = () => { throw new Error('Storage blocked'); };
		const bootstrap = new Function('window', 'document', 'sessionStorage', 'history', 'location',
			'clearTimeout', 'setTimeout', source + '\nreturn dashboardToken;');
		expect(bootstrap(
			{ matchMedia: () => ({ matches: false, addEventListener() {} }) }, document,
			{ setItem: unavailable, getItem: unavailable },
			{ replaceState: (_state: unknown, _title: string, url: string) => { replaced = url; } },
			{ hash: token ? '#token=' + token : '', pathname: '/', search: '' }, () => {}, () => 0,
		)).toBe(token);
		expect(message).toContain('Session storage is unavailable');
		expect(replaced).not.toContain('token=');
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

test('setup navigates its own tab only to the returned loopback dashboard', async () => {
	const setup = await Bun.file(new URL('../../crates/testd-daemon/setup.html', import.meta.url)).text();
	const source = setup.slice(setup.indexOf('async function finishSetup('),
		setup.indexOf("\n\tdocument.getElementById('secret-input').addEventListener"));
	for (const url of ['http://127.0.0.1:7737/#token=fixture-owner', 'https://foreign.example/#token=fixture-owner']) {
		let navigated = '';
		const finish = new Function('document', 'hideNotice', 'authorizedFetch', 'setPill', 'notice', 'location',
			`return (${source});`)(
			{ getElementById: () => ({ value: 'fixture-secret', disabled: false, textContent: '' }) },
			() => {}, async () => ({ ok: true, json: async () => ({ url }) }), () => {}, () => {},
			{ replace: (target: string) => { navigated = target; } },
		);
		await finish();
		expect(navigated).toBe(url.startsWith('http://127.') ? url : '');
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
