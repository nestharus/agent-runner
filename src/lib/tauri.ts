import { Channel, invoke as tauriInvoke } from "@tauri-apps/api/core";
import type {
	ChatStreamEvent,
	DetectionReport,
	ModelConfig,
	ModelSummary,
	PoolSummary,
	ProviderSettingsDelete,
	ProviderSettingsGet,
	ProviderSettingsList,
	ProviderSettingsMigrate,
	ProviderSettingsSchema,
	ProviderSettingsTarget,
	ProviderSettingsValidate,
	ProviderSettingsWrite,
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

export function listProviderSettingsTargets(): Promise<
	ProviderSettingsTarget[]
> {
	return tauriInvoke<ProviderSettingsTarget[]>(
		"list_provider_settings_targets",
	);
}

export function getProviderSettingsSchema(
	accountName: string,
	schemaId: string,
): Promise<ProviderSettingsSchema> {
	return tauriInvoke<ProviderSettingsSchema>("get_provider_settings_schema", {
		accountName,
		schemaId,
	});
}

export function listProviderSettings(
	accountName: string,
): Promise<ProviderSettingsList> {
	return tauriInvoke<ProviderSettingsList>("list_provider_settings", {
		accountName,
	});
}

export function getProviderSettings(
	accountName: string,
	id: string,
): Promise<ProviderSettingsGet> {
	return tauriInvoke<ProviderSettingsGet>("get_provider_settings", {
		accountName,
		id,
	});
}

export function createProviderSettings(
	accountName: string,
	displayName: string | null,
	values: Record<string, unknown>,
): Promise<ProviderSettingsWrite> {
	return tauriInvoke<ProviderSettingsWrite>("create_provider_settings", {
		accountName,
		displayName,
		values,
	});
}

export function updateProviderSettings(
	accountName: string,
	id: string,
	version: string,
	values: Record<string, unknown>,
): Promise<ProviderSettingsWrite> {
	return tauriInvoke<ProviderSettingsWrite>("update_provider_settings", {
		accountName,
		id,
		version,
		values,
	});
}

export function deleteProviderSettings(
	accountName: string,
	id: string,
	version: string,
): Promise<ProviderSettingsDelete> {
	return tauriInvoke<ProviderSettingsDelete>("delete_provider_settings", {
		accountName,
		id,
		version,
	});
}

export function validateProviderSettings(
	accountName: string,
	values: Record<string, unknown>,
): Promise<ProviderSettingsValidate> {
	return tauriInvoke<ProviderSettingsValidate>("validate_provider_settings", {
		accountName,
		values,
	});
}

export function migrateProviderSettings(
	accountName: string,
	dryRun: boolean,
	legacy: Record<string, unknown> | null,
): Promise<ProviderSettingsMigrate> {
	return tauriInvoke<ProviderSettingsMigrate>("migrate_provider_settings", {
		accountName,
		dryRun,
		legacy,
	});
}
