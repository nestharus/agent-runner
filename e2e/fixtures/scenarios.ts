import type { MockConfig } from "./tauri-mock";

/** Fresh user — setup needed, no pools, no models. Triggers wizard. */
export const FRESH_USER: MockConfig = {
	checkSetup: true,
	pools: [],
	models: [],
	events: [
		{
			event: "status",
			data: { message: "Detecting installed CLIs..." },
		},
	],
};

/** Configured user — 3 CLI tools with faceted model tiers. Shows dashboard. */
export const CONFIGURED_USER: MockConfig = {
	checkSetup: false,
	pools: [
		{
			commands: ["claude"],
			model_count: 3,
			model_names: ["claude~high", "claude~medium", "claude~low"],
		},
		{
			commands: ["codex"],
			model_count: 4,
			model_names: [
				"codex~high",
				"codex~medium",
				"codex~low",
				"codex~mini",
			],
		},
		{
			commands: ["gemini"],
			model_count: 2,
			model_names: ["gemini~high", "gemini~low"],
		},
	],
	models: [
		{
			name: "claude~high",
			prompt_mode: "stdin",
			providers: [
				{
					command: "claude",
					args: ["-p", "--model", "opus", "--dangerously-skip-permissions"],
				},
			],
		},
		{
			name: "claude~medium",
			prompt_mode: "stdin",
			providers: [
				{
					command: "claude",
					args: ["-p", "--model", "sonnet"],
				},
			],
		},
		{
			name: "claude~low",
			prompt_mode: "stdin",
			providers: [
				{
					command: "claude",
					args: ["-p", "--model", "haiku"],
				},
			],
		},
	],
};

/** Single CLI — only claude installed with 3 faceted tiers. */
export const SINGLE_CLI: MockConfig = {
	checkSetup: false,
	pools: [
		{
			commands: ["claude"],
			model_count: 3,
			model_names: ["claude~high", "claude~medium", "claude~low"],
		},
	],
	models: [
		{
			name: "claude~medium",
			prompt_mode: "stdin",
			providers: [
				{
					command: "claude",
					args: ["-p", "--model", "sonnet"],
				},
			],
		},
	],
};

/** Error setup — setup that fails mid-stream with error event. */
export const ERROR_SETUP: MockConfig = {
	checkSetup: true,
	pools: [],
	models: [],
	events: [
		{
			event: "status",
			data: { message: "Detecting installed CLIs..." },
		},
		{
			event: "progress",
			data: {
				message: "Configuring providers...",
				percent: 35,
				detail: "Provider 1 of 3",
			},
		},
		{
			event: "error",
			data: {
				message:
					"Claude CLI crashed unexpectedly. Exit code: 137 (killed by signal).",
				recoverable: false,
			},
		},
	],
	eventDelay: 300,
};

/** Empty pools — setup complete but no pools configured yet. */
export const EMPTY_POOLS: MockConfig = {
	checkSetup: false,
	pools: [],
	models: [],
};

/** Status detecting — shows spinner with status message. */
export const STATUS_DETECTING: MockConfig = {
	checkSetup: true,
	events: [
		{
			event: "status",
			data: { message: "Detecting installed CLIs..." },
		},
	],
};

/** Progress bar — shows progress at 65%. */
export const PROGRESS_BAR: MockConfig = {
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
};

/** Detection summary — shows CLI detection table. */
export const DETECTION_SUMMARY: MockConfig = {
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
};

/** CLI selection — shows checkbox selection for available CLIs. */
export const CLI_SELECTION: MockConfig = {
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
							description: "Google Gemini CLI (not installed)",
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
};

/** Form — text, select, password, textarea fields. */
export const FORM_FULL: MockConfig = {
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
							help_text: "A friendly name for this model pool",
						},
						{
							name: "model",
							label: "Model",
							field_type: "select",
							required: true,
							default_value: null,
							options: [
								{ label: "Claude Sonnet 4", value: "claude-sonnet-4" },
								{ label: "GPT-4o", value: "gpt-4o" },
								{ label: "Gemini 2.5 Pro", value: "gemini-2.5-pro" },
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
							help_text: "Your API key will be stored securely",
						},
						{
							name: "notes",
							label: "Notes",
							field_type: "textarea",
							required: false,
							default_value: null,
							options: null,
							placeholder: "Optional notes about this pool",
							help_text: null,
						},
					],
					submit_label: "Save Configuration",
				},
			},
		},
	],
};

/** Form — checkbox and multi_select fields. */
export const FORM_CHECKBOXES: MockConfig = {
	checkSetup: true,
	events: [
		{
			event: "need_input",
			data: {
				action: {
					type: "form",
					title: "Feature Settings",
					description: "Configure optional features for this pool.",
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
								{ label: "Code Generation", value: "codegen" },
								{ label: "Code Review", value: "review" },
								{ label: "Documentation", value: "docs" },
								{ label: "Testing", value: "testing" },
							],
							placeholder: null,
							help_text: "Select which capabilities this pool supports",
						},
					],
					submit_label: "Save",
				},
			},
		},
	],
};

/** Confirm dialog. */
export const CONFIRM_DIALOG: MockConfig = {
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
};

/** OAuth flow. */
export const OAUTH_FLOW: MockConfig = {
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
};

/** API key entry. */
export const API_KEY_ENTRY: MockConfig = {
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
};

/** Wizard stepper — 3-step wizard. */
export const WIZARD_STEPPER: MockConfig = {
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
											{ label: "API Key", value: "api_key" },
											{ label: "OAuth", value: "oauth" },
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
};

/** Setup complete event. */
export const SETUP_COMPLETE: MockConfig = {
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
};

/** Stale session — respond fails. */
export const STALE_SESSION: MockConfig = {
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
};

/** Results display — multiple result types. */
export const RESULTS_DISPLAY: MockConfig = {
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
					description: "Created model config for Claude Sonnet 4",
				},
			},
		},
	],
	eventDelay: 150,
};

/** Pool with flags — for pool settings panel testing. */
export const POOL_WITH_FLAGS: MockConfig = {
	checkSetup: false,
	pools: [
		{
			commands: ["claude"],
			model_count: 2,
			model_names: ["claude~high", "claude~low"],
		},
	],
	models: [
		{
			name: "claude~high",
			prompt_mode: "stdin",
			providers: [
				{
					command: "claude",
					args: ["-p", "--model", "opus", "--dangerously-skip-permissions"],
				},
			],
		},
		{
			name: "claude~low",
			prompt_mode: "stdin",
			providers: [
				{
					command: "claude",
					args: ["-p", "--model", "haiku", "--dangerously-skip-permissions"],
				},
			],
		},
	],
};

/** Model panel test — edit model scenario. */
export const MODEL_EDIT: MockConfig = {
	...CONFIGURED_USER,
	testModelResult: {
		success: true,
		stdout: "Hello! I can help you with coding tasks.",
		stderr: "",
		exit_code: 0,
	},
};

/** Model panel test — test failure scenario. */
export const MODEL_TEST_FAILURE: MockConfig = {
	...CONFIGURED_USER,
	testModelResult: {
		success: false,
		stdout: "",
		stderr: "Error: Invalid API key. Please check your credentials.",
		exit_code: 1,
	},
};
