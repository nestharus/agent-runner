import {
	cleanup,
	fireEvent,
	render,
	screen,
	waitFor,
} from "@solidjs/testing-library";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SetupEvent } from "../lib/types";
import SetupView from "../views/SetupView";

// Access mock-only exports via the mocked module
const tauriMock = await vi.importMock<any>("@tauri-apps/api/core");
const setHandler = tauriMock.__setHandler as (
	cmd: string,
	handler: (args?: any) => Promise<unknown>,
) => void;
const clearHandlers = tauriMock.__clearHandlers as () => void;
const getChannelCallback = tauriMock.__getChannelCallback as () =>
	| ((event: SetupEvent) => void)
	| null;

function sendEvent(event: SetupEvent) {
	const cb = getChannelCallback();
	if (cb) cb(event);
}

const noop = () => {};

beforeEach(() => {
	cleanup();
	clearHandlers();
	vi.clearAllMocks();
	setHandler("start_setup", () => Promise.resolve("session-1"));
});

describe("Setup Flow", () => {
	// Test 1
	it("status event updates status bar", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({ event: "status", data: { message: "Detecting CLIs..." } });

		await waitFor(() => {
			expect(screen.getByText("Detecting CLIs...")).toBeTruthy();
		});
	});

	// Test 2
	it("progress event updates progress indicator", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
			event: "progress",
			data: { message: "Installing...", percent: 50, detail: "step 3/6" },
		});

		await waitFor(() => {
			expect(screen.getByText("Installing...")).toBeTruthy();
			expect.soft(screen.getByText("step 3/6")).toBeTruthy();
		});
	});

	// Test 3
	it("detection summary renders table", async () => {
		const { container } = render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
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
							installed: false,
							version: null,
							authenticated: false,
							wrapper_count: 0,
						},
					],
				},
			},
		});

		await waitFor(() => {
			expect(screen.getByText("Detected CLIs")).toBeTruthy();
			expect.soft(screen.getByText("claude")).toBeTruthy();
			expect.soft(screen.getByText("codex")).toBeTruthy();
			const rows = container.querySelectorAll("tbody tr");
			expect.soft(rows.length).toBe(2);
		});
	});

	// Test 4
	it("form renders fields and submits values", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
			event: "need_input",
			data: {
				action: {
					type: "form",
					title: "Configure Model",
					description: "Enter model details",
					form_id: "model-form",
					fields: [
						{
							name: "model_name",
							label: "Model Name",
							field_type: "text",
							required: true,
							default_value: "gpt-4",
							options: null,
							placeholder: null,
							help_text: null,
						},
						{
							name: "provider",
							label: "Provider",
							field_type: "select",
							required: false,
							default_value: null,
							options: [
								{ label: "OpenAI", value: "openai" },
								{ label: "Anthropic", value: "anthropic" },
							],
							placeholder: "Choose...",
							help_text: null,
						},
					],
					submit_label: "Save",
				},
			},
		});

		await waitFor(() => {
			expect(screen.getByText("Configure Model")).toBeTruthy();
		});

		const nameInput = screen.getByDisplayValue("gpt-4") as HTMLInputElement;
		expect(nameInput).toBeTruthy();

		fireEvent.click(screen.getByText("Save"));

		await waitFor(() => {
			expect(invoke).toHaveBeenCalledWith(
				"setup_respond",
				expect.objectContaining({
					response: expect.objectContaining({
						type: "form_submit",
						form_id: "model-form",
					}),
				}),
			);
		});
	});

	// Test 5
	it("confirm dialog renders and sends response", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
			event: "need_input",
			data: {
				action: {
					type: "confirm",
					title: "Delete Config?",
					message: "This will remove the existing configuration.",
					confirm_id: "delete-confirm",
					confirm_label: "Delete",
					cancel_label: "Keep",
				},
			},
		});

		await waitFor(() => {
			expect(screen.getByText("Delete Config?")).toBeTruthy();
			expect.soft(screen.getByText("Delete")).toBeTruthy();
			expect.soft(screen.getByText("Keep")).toBeTruthy();
		});

		fireEvent.click(screen.getByText("Delete"));

		await waitFor(() => {
			expect(invoke).toHaveBeenCalledWith(
				"setup_respond",
				expect.objectContaining({
					response: expect.objectContaining({
						type: "confirm",
						confirm_id: "delete-confirm",
						confirmed: true,
					}),
				}),
			);
		});
	});

	// Test 6
	it("oauth flow renders and sends completion", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
			event: "need_input",
			data: {
				action: {
					type: "oauth_flow",
					provider: "claude",
					login_command: "claude login",
					instructions: "Run claude login in your terminal",
				},
			},
		});

		await waitFor(() => {
			expect(screen.getByText(/Authentication Required: claude/)).toBeTruthy();
		});

		fireEvent.click(screen.getByText("I've logged in"));

		await waitFor(() => {
			expect(invoke).toHaveBeenCalledWith(
				"setup_respond",
				expect.objectContaining({
					response: expect.objectContaining({
						type: "oauth_complete",
						provider: "claude",
						success: true,
					}),
				}),
			);
		});
	});

	// Test 7
	it("api key entry renders and submits", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
			event: "need_input",
			data: {
				action: {
					type: "api_key_entry",
					provider: "openai",
					env_var: "OPENAI_API_KEY",
					help_url: "https://platform.openai.com/api-keys",
				},
			},
		});

		await waitFor(() => {
			expect(screen.getByText("API Key: openai")).toBeTruthy();
			expect.soft(screen.getByText("Get API key")).toBeTruthy();
		});

		const input = screen.getByPlaceholderText("sk-...") as HTMLInputElement;
		fireEvent.input(input, { target: { value: "sk-test123" } });
		fireEvent.click(screen.getByText("Submit"));

		await waitFor(() => {
			expect(invoke).toHaveBeenCalledWith(
				"setup_respond",
				expect.objectContaining({
					response: expect.objectContaining({
						type: "api_key",
						provider: "openai",
						key: "sk-test123",
					}),
				}),
			);
		});
	});

	// Test 8
	it("cli selection renders checkboxes and submits selected", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
			event: "need_input",
			data: {
				action: {
					type: "cli_selection",
					message: "Select CLIs to configure",
					available: [
						{ name: "claude", installed: true, description: "Anthropic CLI" },
						{ name: "codex", installed: true, description: "OpenAI Codex CLI" },
						{
							name: "gemini",
							installed: false,
							description: "Google Gemini CLI",
						},
					],
				},
			},
		});

		await waitFor(() => {
			expect(screen.getByText("Select CLIs to configure")).toBeTruthy();
		});

		const geminiCheckbox = screen.getByDisplayValue(
			"gemini",
		) as HTMLInputElement;
		expect(geminiCheckbox.disabled).toBe(true);

		fireEvent.click(screen.getByText("Continue"));

		await waitFor(() => {
			expect(invoke).toHaveBeenCalledWith(
				"setup_respond",
				expect.objectContaining({
					response: expect.objectContaining({
						type: "cli_selection",
					}),
				}),
			);
		});
	});

	// Test 9
	it("complete event renders summary and items", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
			event: "complete",
			data: {
				summary: "All CLIs configured successfully.",
				items_configured: ["claude", "codex", "3 models"],
			},
		});

		await waitFor(() => {
			expect(screen.getByText("Setup Complete")).toBeTruthy();
			expect
				.soft(screen.getByText("All CLIs configured successfully."))
				.toBeTruthy();
			expect.soft(screen.getByText("claude")).toBeTruthy();
			expect.soft(screen.getByText("codex")).toBeTruthy();
			expect.soft(screen.getByText("3 models")).toBeTruthy();
		});
	});

	// Test 10
	it("non-recoverable error shows retry button", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
			event: "error",
			data: { message: "Claude CLI crashed", recoverable: false },
		});

		await waitFor(() => {
			expect(screen.getByText("Claude CLI crashed")).toBeTruthy();
			expect.soft(screen.getByText("Retry Setup")).toBeTruthy();
		});
	});

	// Test 11
	it("recoverable error shows message without retry button", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
			event: "error",
			data: { message: "Command timed out, retrying...", recoverable: true },
		});

		await waitFor(() => {
			expect(screen.getByText("Command timed out, retrying...")).toBeTruthy();
		});
		expect(screen.queryByText("Retry Setup")).toBeNull();
	});

	// Test 12
	it("stale session shows fresh start when respond fails", async () => {
		setHandler("setup_respond", () =>
			Promise.reject("No active setup session"),
		);

		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
			event: "need_input",
			data: {
				action: {
					type: "confirm",
					title: "Test",
					message: "Test confirm",
					confirm_id: "test-1",
					confirm_label: null,
					cancel_label: null,
				},
			},
		});

		await waitFor(() => expect(screen.getByText("Confirm")).toBeTruthy());
		fireEvent.click(screen.getByText("Confirm"));

		await waitFor(() => {
			expect(
				screen.getByText("The setup session is no longer active."),
			).toBeTruthy();
			expect.soft(screen.getByText("Start Fresh")).toBeTruthy();
		});
	});

	// Test 13
	it("command output renders correctly", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
			event: "show_result",
			data: {
				content: {
					type: "command_output",
					command: "which claude",
					stdout: "/usr/local/bin/claude",
					stderr: "",
					exit_code: 0,
				},
			},
		});

		await waitFor(() => {
			expect(screen.getByText(/which claude/)).toBeTruthy();
			expect.soft(screen.getByText(/\/usr\/local\/bin\/claude/)).toBeTruthy();
		});
	});

	// Test 14
	it("test result renders pass/fail", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
			event: "show_result",
			data: {
				content: {
					type: "test_result",
					model: "claude-sonnet",
					success: true,
					output: "Hello! I'm working correctly.",
				},
			},
		});

		await waitFor(() => {
			expect(screen.getByText(/claude-sonnet.*PASS/)).toBeTruthy();
		});
	});

	// Test 15
	it("config written renders checkmark", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
			event: "show_result",
			data: {
				content: {
					type: "config_written",
					path: "~/.config/oulipoly-agent-runner/models/claude.toml",
					description: "Created model configuration for Claude",
				},
			},
		});

		await waitFor(() => {
			expect(
				screen.getByText("Created model configuration for Claude"),
			).toBeTruthy();
		});
	});

	// Test 16
	it("status bar and progress hide on complete", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({ event: "status", data: { message: "Working..." } });
		sendEvent({
			event: "progress",
			data: { message: "Step 1", percent: 25, detail: null },
		});

		await waitFor(() => {
			expect(screen.getByText("Working...")).toBeTruthy();
			expect.soft(screen.getByText("Step 1")).toBeTruthy();
		});

		sendEvent({
			event: "complete",
			data: { summary: "Done", items_configured: [] },
		});

		await waitFor(() => {
			expect(screen.getByText("Setup Complete")).toBeTruthy();
			expect.soft(screen.queryByText("Working...")).toBeNull();
		});
	});

	// Test 17
	it("results accumulate in results area", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
			event: "show_result",
			data: {
				content: {
					type: "command_output",
					command: "cmd1",
					stdout: "out1",
					stderr: "",
					exit_code: 0,
				},
			},
		});
		sendEvent({
			event: "show_result",
			data: {
				content: {
					type: "command_output",
					command: "cmd2",
					stdout: "out2",
					stderr: "",
					exit_code: 0,
				},
			},
		});

		await waitFor(() => {
			expect(screen.getByText(/cmd1/)).toBeTruthy();
			expect.soft(screen.getByText(/cmd2/)).toBeTruthy();
		});
	});

	// Test 18
	it("start_setup failure shows error message", async () => {
		setHandler("start_setup", () =>
			Promise.reject("Failed to spawn setup task"),
		);

		render(() => <SetupView onComplete={noop} />);

		await waitFor(() => {
			expect(screen.getByText(/Setup failed/)).toBeTruthy();
			expect.soft(screen.getByText("Retry Setup")).toBeTruthy();
		});
	});

	// Test 19
	it("wizard renders step indicators and form", async () => {
		render(() => <SetupView onComplete={noop} />);
		await waitFor(() =>
			expect(invoke).toHaveBeenCalledWith("start_setup", expect.anything()),
		);

		sendEvent({
			event: "need_input",
			data: {
				action: {
					type: "wizard",
					title: "Provider Setup",
					wizard_id: "provider-wiz",
					current_step: 0,
					steps: [
						{
							label: "API Key",
							description: null,
							form: {
								title: "Enter API Key",
								description: null,
								fields: [
									{
										name: "key",
										label: "API Key",
										field_type: "password",
										required: true,
										default_value: null,
										options: null,
										placeholder: "sk-...",
										help_text: null,
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
								submit_label: "Finish",
							},
						},
					],
				},
			},
		});

		await waitFor(() => {
			expect(screen.getAllByText("API Key").length).toBeGreaterThanOrEqual(1);
			expect.soft(screen.getByText("Model")).toBeTruthy();
			expect.soft(screen.getByText("Enter API Key")).toBeTruthy();
		});
	});
});
