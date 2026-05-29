import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as tauriApi from "../lib/tauri";

const tauriMock = await vi.importMock<any>("@tauri-apps/api/core");
const setHandler = tauriMock.__setHandler as (
	cmd: string,
	handler: (args?: unknown) => Promise<unknown>,
) => void;
const clearHandlers = tauriMock.__clearHandlers as () => void;

beforeEach(() => {
	clearHandlers();
	vi.clearAllMocks();
});

describe("provider settings Tauri wrappers", () => {
	// risk: Cross-language IPC drift; level: TypeScript wrapper; source: contract "TypeScript wrappers must call the exact Tauri command names"
	it("preserves exact command names and argument casing for every provider settings command", async () => {
		const calls: { command: string; args: unknown }[] = [];
		for (const command of providerSettingsCommands()) {
			setHandler(command, (args) => {
				calls.push({ command, args });
				return Promise.resolve(successFor(command));
			});
		}

		await wrapper("listProviderSettingsTargets")();
		await wrapper("getProviderSettingsSchema")(
			"example-model",
			"example.settings/v1",
		);
		await wrapper("listProviderSettings")("example-model");
		await wrapper("getProviderSettings")("example-model", "record");
		await wrapper("createProviderSettings")("example-model", "Record", {
			endpoint: "https://example.test",
			enabled: true,
			limit: 3,
		});
		await wrapper("updateProviderSettings")(
			"example-model",
			"record",
			"opaque-version",
			{ endpoint: "https://example.test/updated" },
		);
		await wrapper("deleteProviderSettings")(
			"example-model",
			"record",
			"opaque-version",
		);
		await wrapper("validateProviderSettings")("example-model", {
			endpoint: "https://example.test",
		});
		await wrapper("migrateProviderSettings")("example-model", true, {
			models: { "example-model": { provider: { script: "opaque" } } },
		});

		expect(calls).toEqual([
			{ command: "list_provider_settings_targets", args: undefined },
			{
				command: "get_provider_settings_schema",
				args: {
					modelName: "example-model",
					schemaId: "example.settings/v1",
				},
			},
			{
				command: "list_provider_settings",
				args: { modelName: "example-model" },
			},
			{
				command: "get_provider_settings",
				args: { modelName: "example-model", id: "record" },
			},
			{
				command: "create_provider_settings",
				args: {
					modelName: "example-model",
					displayName: "Record",
					values: {
						endpoint: "https://example.test",
						enabled: true,
						limit: 3,
					},
				},
			},
			{
				command: "update_provider_settings",
				args: {
					modelName: "example-model",
					id: "record",
					version: "opaque-version",
					values: { endpoint: "https://example.test/updated" },
				},
			},
			{
				command: "delete_provider_settings",
				args: {
					modelName: "example-model",
					id: "record",
					version: "opaque-version",
				},
			},
			{
				command: "validate_provider_settings",
				args: {
					modelName: "example-model",
					values: { endpoint: "https://example.test" },
				},
			},
			{
				command: "migrate_provider_settings",
				args: {
					modelName: "example-model",
					dryRun: true,
					legacy: {
						models: { "example-model": { provider: { script: "opaque" } } },
					},
				},
			},
		]);
	});

	// risk: Provider diagnostics loss; level: TypeScript wrapper; source: contract "Structured IPC DTOs"
	it("preserves structured conflict errors without stringifying diagnostics or details", async () => {
		const conflict = {
			category: "conflict",
			code: "settings_conflict",
			message: "record changed",
			retryable: false,
			details: { remoteVersion: "provider-version-2" },
			diagnostics: [
				{
					severity: "warning",
					message: "Reload before saving",
					path: "/endpoint",
					code: "stale",
				},
			],
			processStatus: { exitCode: 17 },
		};
		setHandler("update_provider_settings", () => Promise.reject(conflict));

		await expect(
			wrapper("updateProviderSettings")("example-model", "record", "stale", {
				endpoint: "https://example.test",
			}),
		).rejects.toMatchObject(conflict);
	});

	// risk: Cross-language IPC drift; level: Vitest mock boundary; source: contract "Missing mock handlers ... must fail tests"
	it("rejects missing provider settings mock handlers instead of resolving undefined", async () => {
		await expect(invoke("list_provider_settings_targets")).rejects.toThrow(
			/list_provider_settings_targets/,
		);
	});
});

function wrapper(name: string): (...args: any[]) => Promise<unknown> {
	const value = (tauriApi as Record<string, unknown>)[name];
	expect(typeof value, `${name} must be exported from src/lib/tauri.ts`).toBe(
		"function",
	);
	return value as (...args: any[]) => Promise<unknown>;
}

function providerSettingsCommands(): string[] {
	return [
		"list_provider_settings_targets",
		"get_provider_settings_schema",
		"list_provider_settings",
		"get_provider_settings",
		"create_provider_settings",
		"update_provider_settings",
		"delete_provider_settings",
		"validate_provider_settings",
		"migrate_provider_settings",
	];
}

function successFor(command: string): unknown {
	if (command === "list_provider_settings_targets") {
		return [
			{
				modelName: "example-model",
				displayName: "Example Model",
				settingsSupported: true,
				schemaId: "example.settings/v1",
			},
		];
	}
	if (command === "get_provider_settings_schema") {
		return {
			schemaId: "example.settings/v1",
			schema: { type: "object", properties: {} },
			ui: { order: [] },
		};
	}
	if (command === "list_provider_settings") {
		return { records: [] };
	}
	if (command === "get_provider_settings") {
		return { record: providerRecord("opaque-version") };
	}
	if (command === "delete_provider_settings") {
		return { deleted: true, id: "record" };
	}
	if (command === "validate_provider_settings") {
		return { valid: true, diagnostics: [] };
	}
	if (command === "migrate_provider_settings") {
		return { actions: [], warnings: [], requiresUserInput: false };
	}
	return {
		record: providerRecord("provider-returned-version"),
		diagnostics: [],
	};
}

function providerRecord(version: string) {
	return {
		id: "record",
		displayName: "Record",
		version,
		values: { endpoint: "https://example.test", enabled: true, limit: 3 },
	};
}
