/** TypeScript interfaces mirroring Rust types from src-tauri/src/setup/actions.rs */

export interface SelectOption {
	value: string;
	label: string;
}

export interface FormField {
	name: string;
	label: string;
	field_type: string;
	required: boolean;
	default_value: string | null;
	options: SelectOption[] | null;
	placeholder: string | null;
	help_text: string | null;
}

export interface FormAction {
	title: string;
	description: string | null;
	fields: FormField[];
	form_id: string;
	submit_label: string | null;
}

export interface WizardStep {
	label: string;
	description: string | null;
	form: FormAction;
}

export interface WizardAction {
	title: string;
	steps: WizardStep[];
	current_step: number;
	wizard_id: string;
}

export interface CliOption {
	name: string;
	installed: boolean;
	description: string;
}

export type Action =
	| ({ type: "form" } & FormAction)
	| ({ type: "wizard" } & WizardAction)
	| {
			type: "confirm";
			title: string;
			message: string;
			confirm_id: string;
			confirm_label: string | null;
			cancel_label: string | null;
	  }
	| {
			type: "oauth_flow";
			provider: string;
			login_command: string;
			instructions: string;
	  }
	| {
			type: "api_key_entry";
			provider: string;
			env_var: string;
			help_url: string | null;
	  }
	| {
			type: "cli_selection";
			available: CliOption[];
			message: string;
	  };

export interface CliSummaryItem {
	name: string;
	installed: boolean;
	version: string | null;
	authenticated: boolean;
	wrapper_count: number;
}

export type ResultContent =
	| {
			type: "command_output";
			command: string;
			stdout: string;
			stderr: string;
			exit_code: number;
	  }
	| { type: "detection_summary"; clis: CliSummaryItem[] }
	| { type: "config_written"; path: string; description: string }
	| { type: "test_result"; model: string; success: boolean; output: string };

export type SetupEvent =
	| { event: "status"; data: { message: string } }
	| {
			event: "progress";
			data: { message: string; percent: number | null; detail: string | null };
	  }
	| { event: "need_input"; data: { action: Action } }
	| { event: "show_result"; data: { content: ResultContent } }
	| {
			event: "complete";
			data: { summary: string; items_configured: string[] };
	  }
	| { event: "error"; data: { message: string; recoverable: boolean } };

export interface TestModelResult {
	success: boolean;
	stdout: string;
	stderr: string;
	exit_code: number;
}

export interface ProviderConfig {
	command: string;
	args: string[];
}

export type PromptMode = "stdin" | "arg";

export interface ModelConfig {
	name: string;
	prompt_mode: PromptMode;
	providers: ProviderConfig[];
}

export interface ModelSummary {
	name: string;
	prompt_mode: PromptMode;
	provider_count: number;
}

export type ParamType =
	| { type: "enum"; options: string[] }
	| { type: "string" }
	| { type: "number"; min?: number; max?: number }
	| { type: "boolean" };

export interface Parameter {
	name: string;
	display_name: string;
	param_type: ParamType;
	description: string;
}

export interface ModelGroup {
	group: string;
	facets: string[];
	modelNames: string[];
	standalone: boolean;
}

export interface PoolSummary {
	commands: string[];
	model_count: number;
	model_names: string[];
}

export interface CliInfo {
	name: string;
	installed: boolean;
	path: string | null;
	version: string | null;
	authenticated: boolean;
	config_dir: string | null;
}

export interface DetectionReport {
	clis: CliInfo[];
	os: { os_type: string; arch: string };
	wrappers: { name: string; path: string; target_cli: string | null }[];
}

export interface ChatMessage {
	role: "user" | "assistant";
	content: string;
}

export type ChatStreamEvent =
	| { event: "delta"; data: { text: string } }
	| { event: "done"; data: Record<string, never> }
	| { event: "error"; data: { message: string } };

export type UserResponse =
	| {
			type: "form_submit";
			form_id: string;
			values: Record<string, string>;
	  }
	| {
			type: "wizard_step";
			wizard_id: string;
			step: number;
			values: Record<string, string>;
	  }
	| { type: "confirm"; confirm_id: string; confirmed: boolean }
	| { type: "oauth_complete"; provider: string; success: boolean }
	| { type: "api_key"; provider: string; key: string }
	| { type: "cli_selection"; selected: string[] }
	| { type: "skip"; reason: string | null }
	| { type: "cancel" };
