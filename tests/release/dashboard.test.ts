import { expect, test } from 'bun:test';

function inlineFunction(page: string, name: string, next: string): string {
	return page.slice(page.indexOf(`function ${name}(`), page.indexOf(`\n\tfunction ${next}(`));
}

const dashboard = await Bun.file(new URL('../../crates/testd-daemon/dashboard.html', import.meta.url)).text();
const commandSource = dashboard.slice(dashboard.indexOf('function parseCommand('), dashboard.indexOf('\n\tfunction saveLaunch('));
// Exercise the shipped inline parser, not a copy of its implementation.
const parseCommand = new Function(`return (${commandSource});`)() as (text: string) => string[];

function launchForm(prefix: 'launch' | 'ssh', values: Record<string, string | boolean> = {}) {
	function input(initial: string | boolean = '') {
		let value = String(initial);
		return {
			get value() { return value; },
			set value(next) { value = String(next).replace(/[\r\n]/g, ''); },
			checked: initial === true,
			disabled: false,
			hidden: false,
			required: false,
			focused: false,
			focus() { this.focused = true; },
		};
	}
	const fields = new Map<string, ReturnType<typeof input>>();
	const field = (id: string) => {
		if (!fields.has(id)) fields.set(id, input(values[id]));
		return fields.get(id)!;
	};
	const cancel = { disabled: false, hidden: true };
	const submit = { disabled: false };
	const controls: Record<string, object> = {
		'[type="reset"]': cancel,
		'[type="submit"]': submit,
		'[data-create-label]': { hidden: false },
		'[data-edit-label]': { hidden: true },
	};
	const form = {
		id: prefix + '-form',
		closest: (): { open: boolean } | null => null,
		querySelector: (selector: string) => controls[selector],
		elements: {
			namedItem: () => ({
				set value(mode: string) { field('ssh-mode-command').checked = mode === 'command'; },
			}),
		},
		reset() {
			handlers.resetLaunchForm({ target: form });
			for (const [id, element] of fields) {
				element.value = String(values[id] ?? '');
				element.checked = values[id] === true;
			}
		},
	};
	const saved: any[] = [];
	const handlers = new Function('document', 'parseCommand', 'post', 'notice', 'refreshState',
		inlineFunction(dashboard, 'saveLaunch', 'ago') +
		'\nreturn { saveLaunch, saveSshApp, editLaunch, resetLaunchForm, formatCommand };')(
		{ getElementById: (id: string) => id === form.id ? form : field(id) }, parseCommand,
		async (_path: string, body: unknown) => { saved.push(body); }, () => {}, () => {},
	);
	const save = () => (prefix === 'ssh' ? handlers.saveSshApp : handlers.saveLaunch)({
		preventDefault() {}, target: form,
	});
	return { field, form, cancel, save, saved, edit: handlers.editLaunch, formatCommand: handlers.formatCommand };
}

function submitForm(name: string, values: Record<string, string | boolean>) {
	const ui = launchForm(name === 'saveSshApp' ? 'ssh' : 'launch', values);
	ui.save();
	return ui.saved[0];
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

test('SSH app mode radios control the remote command fields', () => {
	const radios = dashboard.match(/<input type="radio" name="ssh-mode"[^>]*>/g) || [];
	expect(radios).toHaveLength(2);
	expect(radios[0]).toContain('value="forward" checked');
	expect(radios[1]).toContain('value="command"');
	const mode = { checked: false };
	const command = { disabled: true, required: false };
	const cwd = { disabled: true, required: false };
	const commandField = { hidden: true };
	const cwdField = { hidden: true };
	const elements: Record<string, object> = {
		'ssh-mode-command': mode, 'ssh-command': command, 'ssh-cwd': cwd,
		'ssh-command-field': commandField, 'ssh-cwd-field': cwdField,
	};
	const update = new Function('document', `return (${inlineFunction(dashboard, 'setSshFields', 'saveSshApp')});`)(
		{ getElementById: (id: string) => elements[id] },
	);
	for (const checked of [true, false]) {
		mode.checked = checked;
		update();
		expect(command).toEqual({ disabled: !checked, required: checked });
		expect(cwd).toEqual({ disabled: !checked, required: false });
		expect(commandField.hidden).toBe(!checked);
		expect(cwdField.hidden).toBe(!checked);
	}
});

test('SSH apps save the alias and remote port', () => {
	const saved = submitForm('saveSshApp', {
		'ssh-mode-command': false,
		'ssh-label': 'Web',
		'ssh-domain': 'app.example',
		'ssh-host': 'devbox',
		'ssh-port': '3000',
	});
	expect(saved).toEqual({ label: 'Web', entries: [{ domain: 'app.example' }], ssh: { host: 'devbox', port: 3000 } });
});

test('SSH apps save the remote command as argv and its working directory', () => {
	const saved = submitForm('saveSshApp', {
		'ssh-mode-command': true,
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

test('editing a quick launch preserves its ID, arguments, extra domains and options', async () => {
	const ui = launchForm('launch');
	const launch = {
		id: 'local-web',
		label: 'Web',
		program: 'C:\\Program Files\\runner.exe',
		args: ['', 'C:\\dev\\', 'a "quoted" word', "it's quoted", 'line\nbreak'],
		cwd: null,
		entries: [
			{ domain: 'app.example', target: '3000' },
			{ domain: 'admin.example', target: '3001' },
		],
		env: [['MODE', 'dev']],
		share: true,
		running: true,
	};
	ui.edit(launch);
	expect(ui.field('launch-label').focused).toBe(true);
	expect(ui.field('launch-cwd').required).toBe(false);
	expect(ui.cancel.hidden).toBe(false);
	expect(parseCommand(ui.formatCommand(launch))).toEqual([launch.program, ...launch.args]);

	ui.field('launch-label').value = 'Updated web';
	ui.field('launch-target').value = '4000';
	await ui.save();
	expect(ui.saved).toEqual([{
		id: launch.id, label: 'Updated web', program: launch.program, args: launch.args, cwd: undefined,
		entries: [{ domain: 'app.example', target: '4000' }, launch.entries[1]],
		env: launch.env, share: true,
	}]);
	expect(ui.cancel.hidden).toBe(true);
	expect(ui.field('launch-cwd').required).toBe(true);
});

test('editing an SSH app fills its mode and updates the same ID when switching to forwarding', async () => {
	const ui = launchForm('ssh');
	const disclosure = { open: false };
	ui.form.closest = () => disclosure;
	const launch = {
		id: 'remote-web', label: 'Remote web', entries: [{ domain: 'app.example', target: '' }], env: [], share: false,
		ssh: { host: 'devbox', port: 3000, command: { program: 'npm', args: ['run', 'dev'], cwd: '/srv/web' } },
	};
	ui.edit(launch);
	expect(disclosure.open).toBe(true);
	expect(ui.field('ssh-mode-command').checked).toBe(true);
	expect(ui.field('ssh-command').value).toBe('npm run dev');
	expect(ui.field('ssh-command').disabled).toBe(false);
	expect(ui.field('ssh-cwd').value).toBe('/srv/web');

	ui.field('ssh-mode-command').checked = false;
	ui.field('ssh-port').value = '4000';
	await ui.save();
	expect(ui.saved).toEqual([{
		id: launch.id, label: launch.label, entries: launch.entries, env: [], share: false,
		ssh: { host: 'devbox', port: 4000 },
	}]);
	expect(ui.field('ssh-command').disabled).toBe(true);
	expect(disclosure.open).toBe(false);
});

test('canceling an edit clears the saved ID before creating another quick launch', async () => {
	const ui = launchForm('launch');
	ui.edit({
		id: 'old', label: 'Old', program: 'npm', args: ['run', 'dev'], cwd: 'C:\\dev',
		entries: [{ domain: 'old.example', target: '' }],
	});
	ui.form.reset();
	expect(ui.cancel.hidden).toBe(true);
	expect(ui.field('launch-label').value).toBe('');

	ui.field('launch-label').value = 'New';
	ui.field('launch-domain').value = 'new.example';
	ui.field('launch-cwd').value = 'C:\\next';
	ui.field('launch-command').value = 'npm run start';
	await ui.save();
	expect(ui.saved[0].id).toBeUndefined();
	expect(ui.saved[0].entries).toEqual([{ domain: 'new.example', target: '' }]);
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

test('SSH host-key confirmation exposes trust and cancel actions', () => {
	const source = inlineFunction(dashboard, 'sshPromptForm', 'parseCommand');
	const document = {
		createElement(tag: string) {
			return {
				tag, textContent: '', dataset: {}, children: [] as any[],
				events: new Map<string, (event?: any) => void>(),
				append(...children: any[]) { this.children.push(...children); },
				addEventListener(event: string, callback: (event?: any) => void) { this.events.set(event, callback); },
			};
		},
	};
	const sent: unknown[] = [];
	const render = new Function('document', 'post', 'refreshState', 'notice', source + '\nreturn sshPromptForm;')(
		document, async (_path: string, body: unknown) => sent.push(body), () => {}, () => {},
	);
	const launch = { id: 'devbox' };
	const prompt = { id: 'host-key', kind: 'confirm', message: 'ED25519 key fingerprint is SHA256:fixture.' };
	const confirmation = render(launch, prompt);
	expect(confirmation.children.map((child: { tag: string }) => child.tag)).toEqual(['p', 'div']);
	expect(confirmation.children[0].textContent).toBe(prompt.message);
	const [trust, cancel] = confirmation.children[1].children;
	expect([trust.textContent, cancel.textContent]).toEqual(['Trust and connect', 'Cancel']);
	confirmation.events.get('submit')({ preventDefault() {} });
	render(launch, prompt).children[1].children[1].events.get('click')();
	expect(sent).toEqual([
		{ id: 'devbox', prompt: 'host-key', answer: 'yes' },
		{ id: 'devbox', prompt: 'host-key', answer: null },
	]);
	const password = render(launch, { id: 'password', kind: 'response', message: 'SSH password:' });
	expect(password.children[1].type).toBe('password');
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

function firefoxTrust(userAgent = 'Firefox/145.0') {
	const fields = new Map<string, any>();
	const field = (id: string) => {
		if (!fields.has(id)) fields.set(id, {
			hidden: true, disabled: false, textContent: '', className: '',
			dataset: { probeUrl: 'https://testd.example:9443/ok.svg' },
		});
		return fields.get(id)!;
	};
	const images: ProbeImage[] = [];
	class ProbeImage {
		src = '';
		referrerPolicy = '';
		onload: (() => void) | null = null;
		onerror: (() => void) | null = null;
		constructor() { images.push(this); }
	}
	const timers = new Map<number, () => void>();
	let nextTimer = 0;
	const source = dashboard.slice(dashboard.indexOf('\tlet isCheckingFirefoxTrust = false;'),
		dashboard.indexOf('\n\tasync function certificateState('));
	const check = new Function('document', 'navigator', 'Image', 'setTimeout', 'clearTimeout',
		source + '\nreturn checkFirefoxTrust;')(
		{ getElementById: field }, { userAgent }, ProbeImage,
		(callback: () => void, delay: number) => {
			expect(delay).toBe(5000);
			timers.set(++nextTimer, callback);
			return nextTimer;
		},
		(timer: number) => timers.delete(timer),
	);
	return { check, field, images, timers };
}

test('Firefox trust succeeds only after the browser loads the HTTPS probe', () => {
	const ui = firefoxTrust();
	ui.check();
	ui.check();
	expect(ui.images).toHaveLength(1);
	expect(ui.field('firefox-trust').hidden).toBe(false);
	expect(ui.field('firefox-trust-check').disabled).toBe(true);
	expect(ui.field('firefox-trust-badge').textContent).toBe('Checking...');
	expect(ui.images[0].src).toMatch(/^https:\/\/testd\.example:9443\/ok\.svg\?check=\d+$/);
	expect(ui.images[0].referrerPolicy).toBe('no-referrer');
	ui.images[0].onload!();
	expect(ui.field('firefox-trust-badge').textContent).toBe('Connected');
	expect(ui.field('firefox-import').hidden).toBe(true);
	expect(ui.field('firefox-trust-check').disabled).toBe(false);
	expect(ui.timers.size).toBe(0);
});

test('a failed Firefox probe shows import guidance and can recover after import', () => {
	const ui = firefoxTrust();
	ui.check();
	ui.images[0].onerror!();
	expect(ui.field('firefox-trust-badge').textContent).toBe('Not confirmed');
	expect(ui.field('firefox-trust-detail').textContent).toContain('DNS');
	expect(ui.field('firefox-import').hidden).toBe(false);
	expect(ui.field('firefox-trust-check').disabled).toBe(false);
	ui.check();
	expect(ui.images).toHaveLength(2);
	expect(ui.images[1].src).toStartWith('https:');
	ui.images[1].onload!();
	expect(ui.field('firefox-trust-badge').textContent).toBe('Connected');
	expect(ui.field('firefox-import').hidden).toBe(true);
});

test('a stalled Firefox probe times out without claiming trust or holding the retry button', () => {
	const ui = firefoxTrust();
	ui.check();
	ui.timers.values().next().value!();
	expect(ui.field('firefox-trust-badge').textContent).toBe('Not confirmed');
	expect(ui.field('firefox-import').hidden).toBe(false);
	expect(ui.field('firefox-trust-check').disabled).toBe(false);
	expect(ui.images[0].onload).toBeNull();
	expect(ui.images[0].onerror).toBeNull();
	ui.check();
	expect(ui.images).toHaveLength(2);
});

test('other browsers do not receive Firefox import instructions or probes', () => {
	const ui = firefoxTrust('Chrome/140.0');
	ui.check();
	expect(ui.field('firefox-trust').hidden).toBe(true);
	expect(ui.images).toHaveLength(0);
	expect(ui.timers.size).toBe(0);
});

test('system certificate trust refreshes the independent Firefox HTTPS check', () => {
	const ui = firefoxTrust();
	let checks = 0;
	const source = dashboard.slice(dashboard.indexOf('function renderCertificate('),
		dashboard.indexOf('\n\tlet isCheckingFirefoxTrust'));
	const render = new Function('document', 'checkFirefoxTrust', `return (${source});`)(
		{ getElementById: ui.field }, () => { checks++; },
	);
	for (const trusted of [true, false]) {
		render({ trusted });
		expect(ui.field('certificate-badge').textContent).toBe(trusted ? 'System trusted' : 'System not trusted');
	}
	expect(checks).toBe(2);
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
	const source = `return (${inlineFunction(setup, 'setReady', 'updateConnection')});`;
	const setReady = new Function('document', 'isSettingUpResolver', source)({ getElementById: () => button }, false);
	for (const [id, label] of [['resolver-action', 'Set up resolver'], ['certificate-action', 'Install certificate']]) {
		setReady(id, true);
		expect(button.disabled).toBe(true);
		setReady(id, false);
		expect(button.disabled).toBe(false);
		expect(button.textContent).toBe(label);
	}
});

test('setup confirms resolver readiness from the refreshed state', async () => {
	const setup = await Bun.file(new URL('../../crates/testd-daemon/setup.html', import.meta.url)).text();
	const source = setup.slice(setup.indexOf('function setReady('), setup.indexOf('\n\tasync function setupCertificate('));
	for (const [resolver, expectedNotice] of [
		[true, ''],
		[false, 'Could not verify the system DNS route after setup.'],
		[undefined, 'Cannot read setup state: unavailable'],
	] as const) {
		const button = { disabled: false, textContent: '' };
		const document = { getElementById: (id: string) => id === 'resolver-action' ? button : { value: '' } };
		let message = '';
		const run = new Function('document', 'request', 'getJson', 'setPill', 'notice', 'hideNotice', `
			let resolverReady = false;
			let certificateReady = false;
			let isSettingUpResolver = false;
			${source}
			return setupResolver;
		`)(
			document, async () => ({ ok: true }),
			async () => {
				if (resolver === undefined) throw new Error('unavailable');
				return { resolver };
			},
			() => {}, (value: string) => { message = value; }, () => {},
		);
		await run();
		expect(button.disabled).toBe(resolver === true);
		expect(button.textContent).toBe(resolver ? 'Done' : 'Set up resolver');
		expect(message).toBe(expectedNotice);
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
