import { chromium } from "playwright";
import path from "path";

const SCREENSHOTS = path.resolve("e2e/screenshots");
const BASE_URL = "http://localhost:5173";

/**
 * Build a __TAURI_INTERNALS__ mock that:
 * 1. Implements transformCallback() so the Channel class can register callbacks
 * 2. Implements invoke() to handle commands and send events via Channel callbacks
 * 3. Sends the specified event(s) through the Channel when start_setup is called
 */
function buildTauriMock(opts: {
	checkSetup?: boolean;
	models?: string[];
	events?: object[];
	respondFails?: boolean;
}) {
	const eventsJSON = JSON.stringify(opts.events ?? []);
	return `
		(() => {
			const callbacks = {};
			let nextId = 1;
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
					if (cmd === 'plugin:event|listen') return Promise.resolve(0);
					if (cmd === 'plugin:event|unlisten') return Promise.resolve();
					if (cmd === 'check_setup_needed') return Promise.resolve(${opts.checkSetup ?? false});
					if (cmd === 'list_models') return Promise.resolve(${JSON.stringify(opts.models ?? [])});
					if (cmd === 'start_setup') {
						// args.onEvent is a Channel object — Tauri invoke() passes args directly
						// Channel.id was set by transformCallback in the Channel constructor
						const channel = args?.onEvent;
						const channelId = channel?.id;
						if (channelId != null) {
							const channelCb = window['_' + channelId];
							if (channelCb) {
								const events = ${eventsJSON};
								events.forEach((evt, i) => {
									setTimeout(() => channelCb({ message: evt, index: i }), 300 + i * 200);
								});
							}
						}
						return Promise.resolve('mock-session');
					}
					if (cmd === 'setup_respond') {
						return ${opts.respondFails ? "Promise.reject('No active setup session')" : "Promise.resolve()"};
					}
					if (cmd === 'cancel_setup') return Promise.resolve();
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

async function screenshot(
	context: Awaited<ReturnType<typeof chromium["launch"]>>,
	name: string,
	mock: string,
	route: string = "/setup",
	viewport?: { width: number; height: number },
	postAction?: (
		page: Awaited<ReturnType<typeof context["newPage"]>>,
	) => Promise<void>,
) {
	let ctx: Awaited<ReturnType<typeof context["newContext"]>>;
	if (viewport) {
		ctx = await context.newContext({ viewport });
	} else {
		ctx = await context.newContext({
			viewport: { width: 900, height: 700 },
		});
	}
	const page = await ctx.newPage();
	page.on("console", (msg) =>
		console.log(`  [browser ${msg.type()}] ${msg.text()}`),
	);
	page.on("pageerror", (err) => console.log(`  [browser ERROR] ${err}`));
	await page.addInitScript(mock);
	await page.goto(`${BASE_URL}${route}`, { waitUntil: "networkidle" });
	await page.waitForTimeout(2000);
	if (postAction) await postAction(page);
	await page.waitForTimeout(500);
	await page.screenshot({
		path: `${SCREENSHOTS}/${name}.png`,
		fullPage: true,
	});
	console.log(`  saved: ${name}.png`);
	await page.close();
	await ctx.close();
}

async function main() {
	const browser = await chromium.launch({ headless: true });

	// ===== 1. Pools View — with models =====
	console.log("1. Pools View (with models)");
	await screenshot(
		browser,
		"01-pools-with-models",
		buildTauriMock({
			models: ["claude-sonnet-4", "gpt-4o", "gemini-2.5-pro"],
		}),
		"/",
	);

	// ===== 2. Pools View — empty =====
	console.log("2. Pools View (empty)");
	await screenshot(
		browser,
		"02-pools-empty",
		buildTauriMock({ models: [] }),
		"/",
	);

	// ===== 3. Setup — Status bar =====
	console.log("3. Setup Status");
	await screenshot(
		browser,
		"03-setup-status",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "status",
					data: { message: "Detecting installed CLIs..." },
				},
			],
		}),
	);

	// ===== 4. Progress bar =====
	console.log("4. Progress Bar");
	await screenshot(
		browser,
		"04-progress-bar",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "progress",
					data: {
						message: "Configuring model pools...",
						percent: 65,
						detail: "Pool 2 of 3: openai-gpt4o",
					},
				},
			],
		}),
	);

	// ===== 5. Detection summary =====
	console.log("5. Detection Summary");
	await screenshot(
		browser,
		"05-detection-summary",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "show_result",
					data: {
						content: {
							type: "detection_summary",
							clis: [
								{
									name: "claude",
									installed: true,
									version: "1.2.3",
									authenticated: true,
									wrapper_count: 2,
								},
								{
									name: "codex",
									installed: true,
									version: "0.5.1",
									authenticated: false,
									wrapper_count: 0,
								},
								{
									name: "gemini",
									installed: false,
									version: null,
									authenticated: false,
									wrapper_count: 0,
								},
							],
						},
					},
				},
			],
		}),
	);

	// ===== 6. Form — text/select/password/textarea =====
	console.log("6. Form (text, select, password, textarea)");
	await screenshot(
		browser,
		"06-form-full",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "need_input",
					data: {
						action: {
							type: "form",
							title: "Configure Model Pool",
							description:
								"Set up your preferred model configuration for this provider.",
							form_id: "model-config",
							fields: [
								{
									name: "pool_name",
									label: "Pool Name",
									field_type: "text",
									required: true,
									default_value: "primary",
									options: null,
									placeholder: "e.g. primary, development",
									help_text:
										"A friendly name for this model pool",
								},
								{
									name: "model",
									label: "Model",
									field_type: "select",
									required: true,
									default_value: null,
									options: [
										{
											label: "Claude Sonnet 4",
											value: "claude-sonnet-4",
										},
										{ label: "GPT-4o", value: "gpt-4o" },
										{
											label: "Gemini 2.5 Pro",
											value: "gemini-2.5-pro",
										},
									],
									placeholder: "Select a model...",
									help_text: null,
								},
								{
									name: "api_key",
									label: "API Key",
									field_type: "password",
									required: true,
									default_value: null,
									options: null,
									placeholder: "sk-...",
									help_text:
										"Your API key will be stored securely",
								},
								{
									name: "notes",
									label: "Notes",
									field_type: "textarea",
									required: false,
									default_value: null,
									options: null,
									placeholder:
										"Optional notes about this pool",
									help_text: null,
								},
							],
							submit_label: "Save Configuration",
						},
					},
				},
			],
		}),
	);

	// ===== 7. Form — checkbox/multi_select =====
	console.log("7. Form (checkbox, multi_select)");
	await screenshot(
		browser,
		"07-form-checkboxes",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "need_input",
					data: {
						action: {
							type: "form",
							title: "Feature Settings",
							description:
								"Configure optional features for this pool.",
							form_id: "features",
							fields: [
								{
									name: "auto_retry",
									label: "Enable auto-retry on failure",
									field_type: "checkbox",
									required: false,
									default_value: "true",
									options: null,
									placeholder: null,
									help_text: null,
								},
								{
									name: "capabilities",
									label: "Capabilities",
									field_type: "multi_select",
									required: false,
									default_value: null,
									options: [
										{
											label: "Code Generation",
											value: "codegen",
										},
										{
											label: "Code Review",
											value: "review",
										},
										{
											label: "Documentation",
											value: "docs",
										},
										{
											label: "Testing",
											value: "testing",
										},
									],
									placeholder: null,
									help_text:
										"Select which capabilities this pool supports",
								},
							],
							submit_label: "Save",
						},
					},
				},
			],
		}),
	);

	// ===== 8. Confirm Dialog =====
	console.log("8. Confirm Dialog");
	await screenshot(
		browser,
		"08-confirm-dialog",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "need_input",
					data: {
						action: {
							type: "confirm",
							title: "Delete Model Pool?",
							message:
								"This will permanently remove the 'primary' pool and all its configuration. This action cannot be undone.",
							confirm_id: "delete-pool",
							confirm_label: "Delete Pool",
							cancel_label: "Cancel",
						},
					},
				},
			],
		}),
	);

	// ===== 9. OAuth Flow =====
	console.log("9. OAuth Flow");
	await screenshot(
		browser,
		"09-oauth-flow",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "need_input",
					data: {
						action: {
							type: "oauth_flow",
							provider: "claude",
							login_command: "claude login",
							instructions:
								"Open your terminal and run 'claude login' to authenticate. A browser window will open for you to sign in with your Anthropic account. Once complete, return here and click the button below.",
						},
					},
				},
			],
		}),
	);

	// ===== 10. API Key Entry =====
	console.log("10. API Key Entry");
	await screenshot(
		browser,
		"10-api-key-entry",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "need_input",
					data: {
						action: {
							type: "api_key_entry",
							provider: "OpenAI",
							env_var: "OPENAI_API_KEY",
							help_url: "https://platform.openai.com/api-keys",
						},
					},
				},
			],
		}),
	);

	// ===== 11. CLI Selection =====
	console.log("11. CLI Selection");
	await screenshot(
		browser,
		"11-cli-selection",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "need_input",
					data: {
						action: {
							type: "cli_selection",
							message:
								"Select which CLIs you'd like to configure. Only installed CLIs can be selected.",
							available: [
								{
									name: "claude",
									installed: true,
									description: "Anthropic Claude CLI",
								},
								{
									name: "codex",
									installed: true,
									description: "OpenAI Codex CLI",
								},
								{
									name: "gemini",
									installed: false,
									description:
										"Google Gemini CLI (not installed)",
								},
								{
									name: "aider",
									installed: false,
									description: "Aider (not installed)",
								},
							],
						},
					},
				},
			],
		}),
	);

	// ===== 12. Wizard =====
	console.log("12. Wizard Stepper");
	await screenshot(
		browser,
		"12-wizard-stepper",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "need_input",
					data: {
						action: {
							type: "wizard",
							title: "Provider Setup",
							wizard_id: "provider-wiz",
							current_step: 0,
							steps: [
								{
									label: "Authentication",
									description: null,
									form: {
										title: "Authenticate with Claude",
										description:
											"Enter your API key or use OAuth to connect.",
										fields: [
											{
												name: "auth_method",
												label: "Auth Method",
												field_type: "select",
												required: true,
												default_value: "api_key",
												options: [
													{
														label: "API Key",
														value: "api_key",
													},
													{
														label: "OAuth",
														value: "oauth",
													},
												],
												placeholder: null,
												help_text: null,
											},
											{
												name: "key",
												label: "API Key",
												field_type: "password",
												required: false,
												default_value: null,
												options: null,
												placeholder: "sk-ant-...",
												help_text:
													"Required if using API Key authentication",
											},
										],
										form_id: "step-0",
										submit_label: "Next",
									},
								},
								{
									label: "Model",
									description: null,
									form: {
										title: "Select Model",
										description: null,
										fields: [
											{
												name: "model",
												label: "Model",
												field_type: "text",
												required: true,
												default_value: null,
												options: null,
												placeholder: null,
												help_text: null,
											},
										],
										form_id: "step-1",
										submit_label: "Next",
									},
								},
								{
									label: "Confirm",
									description: null,
									form: {
										title: "Review",
										description: null,
										fields: [],
										form_id: "step-2",
										submit_label: "Finish",
									},
								},
							],
						},
					},
				},
			],
		}),
	);

	// ===== 13. Setup Complete =====
	console.log("13. Setup Complete");
	await screenshot(
		browser,
		"13-setup-complete",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "complete",
					data: {
						summary:
							"All CLIs configured successfully. 3 model pools created.",
						items_configured: [
							"claude (OAuth)",
							"codex (API key)",
							"claude-sonnet-4",
							"gpt-4o",
							"gemini-2.5-pro",
						],
					},
				},
			],
		}),
	);

	// ===== 14. Error + Retry =====
	console.log("14. Error + Retry");
	await screenshot(
		browser,
		"14-error-retry",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "error",
					data: {
						message:
							"Claude CLI crashed unexpectedly. Exit code: 137 (killed by signal).",
						recoverable: false,
					},
				},
			],
		}),
	);

	// ===== 15. Results display =====
	console.log("15. Results Display");
	await screenshot(
		browser,
		"15-results-display",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "show_result",
					data: {
						content: {
							type: "command_output",
							command: "claude --version",
							stdout: "claude 1.2.3 (anthropic-cli)",
							stderr: "",
							exit_code: 0,
						},
					},
				},
				{
					event: "show_result",
					data: {
						content: {
							type: "test_result",
							model: "claude-sonnet-4",
							success: true,
							output: "Hello! I can help with code.",
						},
					},
				},
				{
					event: "show_result",
					data: {
						content: {
							type: "test_result",
							model: "gpt-4o",
							success: false,
							output: "Error: Invalid API key",
						},
					},
				},
				{
					event: "show_result",
					data: {
						content: {
							type: "config_written",
							path: "~/.config/oulipoly-agent-runner/models/claude-sonnet.toml",
							description:
								"Created model config for Claude Sonnet 4",
						},
					},
				},
			],
		}),
	);

	// ===== 16. Stale Session =====
	console.log("16. Stale Session");
	await screenshot(
		browser,
		"16-stale-session",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "need_input",
					data: {
						action: {
							type: "confirm",
							title: "Test",
							message: "Confirm action",
							confirm_id: "stale-test",
							confirm_label: "Yes",
							cancel_label: "No",
						},
					},
				},
			],
			respondFails: true,
		}),
		"/setup",
		undefined,
		async (page) => {
			const yesBtn = page.getByText("Yes");
			if (await yesBtn.isVisible()) await yesBtn.click();
			await page.waitForTimeout(1000);
		},
	);

	// ===== 17. Narrow viewport =====
	console.log("17. Narrow viewport (480px)");
	await screenshot(
		browser,
		"17-narrow-form",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "need_input",
					data: {
						action: {
							type: "form",
							title: "Configure Model Pool",
							description: "Set up your model configuration.",
							form_id: "narrow-test",
							fields: [
								{
									name: "name",
									label: "Pool Name",
									field_type: "text",
									required: true,
									default_value: "primary",
									options: null,
									placeholder: null,
									help_text: "A friendly name",
								},
								{
									name: "model",
									label: "Model",
									field_type: "select",
									required: true,
									default_value: null,
									options: [
										{ label: "Claude", value: "claude" },
										{ label: "GPT-4o", value: "gpt4o" },
									],
									placeholder: "Choose...",
									help_text: null,
								},
							],
							submit_label: "Save",
						},
					},
				},
			],
		}),
		"/setup",
		{ width: 480, height: 700 },
	);

	// ===== 18. Wide viewport =====
	console.log("18. Wide viewport (1400px)");
	await screenshot(
		browser,
		"18-wide-form",
		buildTauriMock({
			checkSetup: true,
			events: [
				{
					event: "need_input",
					data: {
						action: {
							type: "form",
							title: "Configure Model Pool",
							description: "Set up your model configuration.",
							form_id: "wide-test",
							fields: [
								{
									name: "name",
									label: "Pool Name",
									field_type: "text",
									required: true,
									default_value: "primary",
									options: null,
									placeholder: null,
									help_text: "A friendly name",
								},
								{
									name: "model",
									label: "Model",
									field_type: "select",
									required: true,
									default_value: null,
									options: [
										{ label: "Claude", value: "claude" },
										{ label: "GPT-4o", value: "gpt4o" },
									],
									placeholder: "Choose...",
									help_text: null,
								},
							],
							submit_label: "Save",
						},
					},
				},
			],
		}),
		"/setup",
		{ width: 1400, height: 800 },
	);

	await browser.close();
	console.log("\nAll screenshots saved to e2e/screenshots/");
}

main().catch(console.error);
