import { Channel, invoke as tauriInvoke } from "@tauri-apps/api/core";
import type {
	ChatStreamEvent,
	DetectionReport,
	ModelConfig,
	ModelSummary,
	PoolSummary,
	SetupEvent,
	TestModelResult,
	UserResponse,
} from "./types";

export { Channel };

export function checkSetupNeeded(): Promise<boolean> {
	return tauriInvoke<boolean>("check_setup_needed");
}

export function startSetup(channel: Channel<SetupEvent>): Promise<string> {
	return tauriInvoke<string>("start_setup", { onEvent: channel });
}

export function setupRespond(response: UserResponse): Promise<void> {
	return tauriInvoke<void>("setup_respond", { response });
}

export function cancelSetup(): Promise<void> {
	return tauriInvoke<void>("cancel_setup");
}

export function listModels(): Promise<ModelSummary[]> {
	return tauriInvoke<ModelSummary[]>("list_models");
}

export function getModel(name: string): Promise<ModelConfig> {
	return tauriInvoke<ModelConfig>("get_model", { name });
}

export function saveModel(model: ModelConfig): Promise<void> {
	return tauriInvoke<void>("save_model", { model });
}

export function deleteModel(name: string): Promise<void> {
	return tauriInvoke<void>("delete_model", { name });
}

export function detectClis(): Promise<DetectionReport> {
	return tauriInvoke<DetectionReport>("detect_clis");
}

export function listPools(): Promise<PoolSummary[]> {
	return tauriInvoke<PoolSummary[]>("list_pools");
}

export function updatePool(
	originalCommands: string[],
	newCommands: string[],
): Promise<void> {
	return tauriInvoke<void>("update_pool", { originalCommands, newCommands });
}

export function startCliSetup(
	cliName: string,
	channel: Channel<SetupEvent>,
): Promise<string> {
	return tauriInvoke<string>("start_cli_setup", { cliName, onEvent: channel });
}

export function reloadModels(): Promise<void> {
	return tauriInvoke<void>("reload_models");
}

export function testModel(name: string): Promise<TestModelResult> {
	return tauriInvoke<TestModelResult>("test_model", { name });
}

export function chatSend(
	message: string,
	context: string,
	onEvent: Channel<ChatStreamEvent>,
): Promise<void> {
	return tauriInvoke<void>("chat_send", { message, context, onEvent });
}
