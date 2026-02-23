import type { Page } from "@playwright/test";

export interface PoolMock {
	commands: string[];
	model_count: number;
	model_names: string[];
}

export interface ModelMock {
	name: string;
	prompt_mode: "stdin" | "arg";
	providers: { command: string; args: string[] }[];
}

export interface SetupEventMock {
	event: string;
	data: Record<string, unknown>;
}

export interface MockConfig {
	checkSetup?: boolean;
	pools?: PoolMock[];
	models?: ModelMock[];
	events?: SetupEventMock[];
	eventDelay?: number;
	respondFails?: boolean;
	testModelResult?: {
		success: boolean;
		stdout: string;
		stderr: string;
		exit_code: number;
	};
}

/**
 * Build a __TAURI_INTERNALS__ mock injection script.
 *
 * Supports:
 * - transformCallback / unregisterCallback for Channel simulation
 * - invoke() command routing based on config
 * - Stateful call tracking via window.__TAURI_CALLS__
 * - Delayed event sequences through Channels
 */
export function buildMockScript(config: MockConfig): string {
	const poolsJSON = JSON.stringify(config.pools ?? []);
	const modelsJSON = JSON.stringify(config.models ?? []);
	const eventsJSON = JSON.stringify(config.events ?? []);
	const eventDelay = config.eventDelay ?? 200;
	const testResult = JSON.stringify(
		config.testModelResult ?? {
			success: true,
			stdout: "Hello!",
			stderr: "",
			exit_code: 0,
		},
	);

	return `
		(() => {
			const callbacks = {};
			let nextId = 1;
			window.__TAURI_CALLS__ = [];

			window.__TAURI_INTERNALS__ = {
				transformCallback: (cb, once) => {
					const id = nextId++;
					const prop = '_' + id;
					Object.defineProperty(window, prop, {
						value: (result) => {
							if (once) { delete window[prop]; }
							return cb(result);
						},
						writable: true,
						configurable: true,
					});
					callbacks[id] = window[prop];
					return id;
				},
				unregisterCallback: (id) => {
					delete window['_' + id];
					delete callbacks[id];
				},
				invoke: (cmd, args) => {
					window.__TAURI_CALLS__.push({ cmd, args: args ? JSON.parse(JSON.stringify(args)) : undefined });

					if (cmd === 'plugin:event|listen') return Promise.resolve(0);
					if (cmd === 'plugin:event|unlisten') return Promise.resolve();

					if (cmd === 'check_setup_needed') return Promise.resolve(${config.checkSetup ?? false});

					if (cmd === 'list_pools') return Promise.resolve(${poolsJSON});

					if (cmd === 'list_models') {
						const models = ${modelsJSON};
						return Promise.resolve(models.map(m => ({
							name: m.name,
							prompt_mode: m.prompt_mode,
							provider_count: m.providers.length,
						})));
					}

					if (cmd === 'get_model') {
						const models = ${modelsJSON};
						const found = models.find(m => m.name === (args && args.name));
						return found ? Promise.resolve(found) : Promise.reject('Model not found');
					}

					if (cmd === 'start_setup' || cmd === 'start_cli_setup') {
						const channel = args?.onEvent;
						const channelId = channel?.id;
						if (channelId != null) {
							const channelCb = window['_' + channelId];
							if (channelCb) {
								const events = ${eventsJSON};
								events.forEach((evt, i) => {
									setTimeout(() => channelCb({ message: evt, index: i }), 300 + i * ${eventDelay});
								});
							}
						}
						return Promise.resolve('mock-session');
					}

					if (cmd === 'setup_respond') {
						return ${config.respondFails ? "Promise.reject('No active setup session')" : "Promise.resolve()"};
					}

					if (cmd === 'cancel_setup') return Promise.resolve();
					if (cmd === 'save_model') return Promise.resolve();
					if (cmd === 'delete_model') return Promise.resolve();
					if (cmd === 'update_pool') return Promise.resolve();
					if (cmd === 'reload_models') return Promise.resolve();
					if (cmd === 'detect_clis') return Promise.resolve({ clis: [], os: { os_type: 'linux', arch: 'x86_64' }, wrappers: [] });

					if (cmd === 'test_model') return Promise.resolve(${testResult});

					if (cmd === 'chat_send') {
						const channel = args?.onEvent;
						const channelId = channel?.id;
						if (channelId != null) {
							const channelCb = window['_' + channelId];
							if (channelCb) {
								setTimeout(() => channelCb({ message: { event: 'delta', data: { text: 'Hello from mock!' } }, index: 0 }), 100);
								setTimeout(() => channelCb({ message: { event: 'done', data: {} }, index: 1 }), 200);
							}
						}
						return Promise.resolve();
					}

					return Promise.resolve();
				},
				metadata: {
					currentWindow: { label: 'main' },
					currentWebview: { label: 'main' },
				},
				convertFileSrc: (src) => src,
			};
		})();
	`;
}

/**
 * Inject the Tauri mock into a Playwright page via addInitScript.
 */
export async function injectMock(page: Page, config: MockConfig): Promise<void> {
	await page.addInitScript(buildMockScript(config));
}

/**
 * Get recorded Tauri command invocations from the page.
 */
export async function getCalls(
	page: Page,
): Promise<{ cmd: string; args?: Record<string, unknown> }[]> {
	return page.evaluate(() => (window as any).__TAURI_CALLS__ ?? []);
}

/**
 * Navigate to a route and wait for content to stabilize.
 */
export async function navigateAndWait(
	page: Page,
	route: string,
	waitMs = 1500,
): Promise<void> {
	await page.goto(route, { waitUntil: "networkidle" });
	await page.waitForTimeout(waitMs);
}

/**
 * Capture a screenshot to the catalog directory.
 */
export async function catalogScreenshot(
	page: Page,
	category: string,
	name: string,
): Promise<void> {
	await page.screenshot({
		path: `/tmp/oulipoly-e2e/catalog/${category}/${name}.png`,
		fullPage: true,
	});
}
