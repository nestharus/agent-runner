import { expect, test } from "@playwright/test";
import { getCalls, injectMock, navigateAndWait } from "./fixtures/tauri-mock";

test("provider settings visible flow uses neutral Tauri commands and preserves returned versions", async ({
	page,
}) => {
	await injectMock(page, {
		checkSetup: false,
		strictCommands: true,
		pools: [
			{
				commands: ["provider-a"],
				model_count: 1,
				model_names: ["example-model"],
			},
		],
		models: [
			{
				name: "example-model",
				prompt_mode: "arg",
				providers: [{ command: "provider-a", args: [] }],
			},
		],
		providerSettings: {
			targets: [
				{
					modelName: "example-model",
					displayName: "Example Model",
					settingsSupported: true,
					schemaId: "example.settings/v1",
				},
			],
			schema: {
				schemaId: "example.settings/v1",
				schema: {
					type: "object",
					required: ["endpoint"],
					properties: {
						endpoint: {
							type: "string",
							title: "Endpoint",
							description: "Neutral endpoint",
						},
						enabled: { type: "boolean", title: "Enabled" },
					},
				},
				ui: { order: ["endpoint", "enabled"] },
			},
			records: [
				{
					id: "record",
					displayName: "Record",
					version: "opaque-version",
					summary: { endpoint: "https://example.test" },
				},
			],
			record: {
				id: "record",
				displayName: "Record",
				version: "opaque-version",
				values: { endpoint: "https://example.test", enabled: true },
			},
			validate: {
				valid: false,
				diagnostics: [
					{
						severity: "warning",
						message: "Endpoint note",
						path: "/endpoint",
					},
				],
			},
			migrate: {
				actions: [{ kind: "would-write", target: "record" }],
				warnings: ["Review before applying"],
				requiresUserInput: false,
				diagnostics: [],
			},
		},
	});

	await navigateAndWait(page, "/");

	await page.getByRole("button", { name: /provider settings/i }).click();
	await page.getByText("Example Model").click();
	await expect(page.getByLabel("Endpoint")).toHaveValue("https://example.test");

	await page.getByRole("button", { name: /validate/i }).click();
	await expect(page.getByText("Endpoint note")).toBeVisible();

	await page.getByLabel("Endpoint").fill("https://example.test/next");
	await page.getByRole("button", { name: /^save$/i }).click();
	await expect(page.getByText("provider-updated-version")).toBeVisible();

	const calls = await getCalls(page);
	expect(calls.map((call) => call.cmd)).toEqual(
		expect.arrayContaining([
			"list_provider_settings_targets",
			"get_provider_settings_schema",
			"list_provider_settings",
			"get_provider_settings",
			"validate_provider_settings",
			"update_provider_settings",
		]),
	);
	expect(
		calls.find((call) => call.cmd === "update_provider_settings")?.args,
	).toMatchObject({
		modelName: "example-model",
		id: "record",
		version: "opaque-version",
		values: { endpoint: "https://example.test/next" },
	});
});
