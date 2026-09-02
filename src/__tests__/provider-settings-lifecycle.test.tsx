import {
	cleanup,
	fireEvent,
	render,
	screen,
	waitFor,
} from "@solidjs/testing-library";
import { QueryClient, QueryClientProvider } from "@tanstack/solid-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import PoolsView from "../views/PoolsView";

const tauriMock = await vi.importMock<any>("@tauri-apps/api/core");
const setHandler = tauriMock.__setHandler as (
	cmd: string,
	handler: (args?: any) => Promise<unknown>,
) => void;
const clearHandlers = tauriMock.__clearHandlers as () => void;

beforeEach(() => {
	cleanup();
	clearHandlers();
	vi.clearAllMocks();
});

describe("provider settings Configure lifecycle", () => {
	// risk: Cross-language IPC drift; level: Configure UI; source: contract "Provider settings entry is reachable from Configure"
	it("exposes a separate provider settings entry from Configure without replacing pool flag settings", async () => {
		setHandler("list_pools", () =>
			Promise.resolve([
				{
					commands: ["provider-a"],
					model_count: 1,
					model_names: ["example-model"],
				},
			]),
		);
		setHandler("list_models", () =>
			Promise.resolve([
				{
					name: "example-model",
					prompt_mode: "arg",
					provider_count: 1,
				},
			]),
		);

		renderWithQuery(() => <PoolsView onRunSetup={() => {}} />);

		await waitFor(() => {
			expect(screen.getAllByText("provider-a").length).toBeGreaterThan(0);
		});
		expect(
			screen.getByRole("button", { name: /provider settings/i }),
		).toBeTruthy();
		expect(screen.getByRole("button", { name: /pool settings/i })).toBeTruthy();
	});

	// risk: Opaque version and conflict data loss; level: provider settings UI; source: contract "Create/update success replaces local baseline"
	it("loads target/schema/records and replaces displayed state with provider-returned create and update records", async () => {
		const ProviderSettingsPanel = await loadPanel();
		const calls: { command: string; args?: any }[] = [];
		installHappyPathHandlers(calls);

		render(() => <ProviderSettingsPanel />);

		await chooseTarget();
		await waitFor(() => {
			expect(screen.getByDisplayValue("https://example.test")).toBeTruthy();
		});

		fireEvent.input(screen.getByLabelText("Endpoint"), {
			target: { value: "https://example.test/created" },
		});
		fireEvent.click(screen.getByRole("button", { name: /create/i }));

		await waitFor(() => {
			expect(screen.getByText("provider-created-version")).toBeTruthy();
		});

		fireEvent.input(screen.getByLabelText("Endpoint"), {
			target: { value: "https://example.test/updated" },
		});
		fireEvent.click(screen.getByRole("button", { name: /^save$/i }));

		await waitFor(() => {
			expect(screen.getByText("provider-updated-version")).toBeTruthy();
			expect(
				screen.getByDisplayValue("https://provider-returned.test"),
			).toBeTruthy();
		});

		const update = calls.find(
			(call) => call.command === "update_provider_settings",
		);
		expect(update?.args.version).toBe("provider-created-version");
	});

	// risk: Provider diagnostics loss; level: provider settings UI; source: contract "Validate displays provider diagnostics"
	it("renders validation diagnostics generically using JSON Pointer paths", async () => {
		const ProviderSettingsPanel = await loadPanel();
		installHappyPathHandlers([]);
		setHandler("validate_provider_settings", () =>
			Promise.resolve({
				valid: false,
				diagnostics: [
					{
						severity: "error",
						message: "Endpoint is required",
						path: "/endpoint",
						code: "required",
					},
					{
						severity: "warning",
						message: "Remote note",
						path: "/unknown",
						code: "note",
					},
				],
			}),
		);

		render(() => <ProviderSettingsPanel />);
		await chooseTarget();
		fireEvent.click(screen.getByRole("button", { name: /validate/i }));

		await waitFor(() => {
			expect(screen.getByRole("alert").textContent).toContain("Remote note");
			expect(
				screen.getByLabelText("Endpoint").getAttribute("aria-invalid"),
			).toBe("true");
			expect(screen.getByText("Endpoint is required")).toBeTruthy();
			expect(screen.getByText("Remote note")).toBeTruthy();
		});
	});

	// risk: Opaque version and conflict data loss; level: provider settings UI; source: contract "Delete success removes or reloads state only after provider success"
	it("removes or reloads a deleted record only after provider delete succeeds", async () => {
		const ProviderSettingsPanel = await loadPanel();
		const calls: { command: string; args?: any }[] = [];
		let deleteSucceeded = false;
		const deleteDeferred = deferred<{ deleted: boolean; id: string }>();
		installHappyPathHandlers(calls);
		setHandler("list_provider_settings", (args) => {
			calls.push({ command: "list_provider_settings", args });
			return Promise.resolve({
				records: deleteSucceeded
					? []
					: [
							{
								id: "record",
								displayName: "Record",
								version: "opaque-version",
								summary: { endpoint: "https://example.test" },
							},
						],
			});
		});
		setHandler("delete_provider_settings", (args) => {
			calls.push({ command: "delete_provider_settings", args });
			return deleteDeferred.promise;
		});

		render(() => <ProviderSettingsPanel />);
		await chooseTarget();
		await waitFor(() => {
			expect(screen.getByText("Record")).toBeTruthy();
		});

		fireEvent.click(screen.getByRole("button", { name: /delete/i }));

		expect(
			calls.filter((call) => call.command === "list_provider_settings"),
		).toHaveLength(1);
		expect(screen.getByText("Record")).toBeTruthy();

		deleteSucceeded = true;
		deleteDeferred.resolve({ deleted: true, id: "record" });

		await waitFor(() => {
			expect(
				calls.filter((call) => call.command === "list_provider_settings"),
			).toHaveLength(2);
		});
		expect(
			calls.find((call) => call.command === "delete_provider_settings")?.args,
		).toMatchObject({
			accountName: "provider-a",
			id: "record",
			version: "opaque-version",
		});
	});

	// risk: Opaque version and conflict data loss; level: provider settings UI; source: contract "Conflict preserves the local draft"
	it("preserves local draft on conflict, reloads remote, and retries with the newly loaded version", async () => {
		const ProviderSettingsPanel = await loadPanel();
		const calls: { command: string; args?: any }[] = [];
		installHappyPathHandlers(calls);
		let updateAttempts = 0;
		setHandler("update_provider_settings", (args) => {
			calls.push({ command: "update_provider_settings", args });
			updateAttempts += 1;
			if (updateAttempts === 1) {
				return Promise.reject({
					category: "conflict",
					code: "settings_conflict",
					message: "record changed",
					retryable: false,
					details: { remoteVersion: "remote-version" },
					diagnostics: [
						{
							severity: "warning",
							message: "Reload before saving",
							path: "/endpoint",
						},
					],
				});
			}
			return Promise.resolve({
				record: providerRecord("provider-reapplied-version", {
					endpoint: "https://local-draft.test",
				}),
				diagnostics: [],
			});
		});
		setHandler("get_provider_settings", (args) => {
			calls.push({ command: "get_provider_settings", args });
			return Promise.resolve({
				record: providerRecord("remote-version", {
					endpoint: "https://remote.test",
				}),
			});
		});

		render(() => <ProviderSettingsPanel />);
		await chooseTarget();
		fireEvent.input(screen.getByLabelText("Endpoint"), {
			target: { value: "https://local-draft.test" },
		});
		fireEvent.click(screen.getByRole("button", { name: /^save$/i }));

		await waitFor(() => {
			expect(screen.getByText(/conflict/i)).toBeTruthy();
			expect(screen.getByDisplayValue("https://local-draft.test")).toBeTruthy();
			expect(screen.getAllByText("remote-version").length).toBeGreaterThan(0);
		});

		fireEvent.click(screen.getByRole("button", { name: /reapply/i }));

		await waitFor(() => {
			expect(screen.getByText("provider-reapplied-version")).toBeTruthy();
		});
		expect(
			calls.filter((call) => call.command === "update_provider_settings")[1]
				.args.version,
		).toBe("remote-version");
	});

	// risk: Opaque version and conflict data loss; level: provider settings UI; source: contract "Conflict ... lets the user keep remote ... or abandon draft"
	it("offers neutral conflict keep-remote and abandon-draft resolution paths", async () => {
		const ProviderSettingsPanel = await loadPanel();
		installHappyPathHandlers([]);
		setHandler("update_provider_settings", () =>
			Promise.reject({
				category: "conflict",
				code: "settings_conflict",
				message: "record changed",
				retryable: false,
				details: { remoteVersion: "remote-version" },
				diagnostics: [],
			}),
		);
		setHandler("get_provider_settings", () =>
			Promise.resolve({
				record: providerRecord("remote-version", {
					endpoint: "https://remote.test",
				}),
			}),
		);

		render(() => <ProviderSettingsPanel />);
		await chooseTarget();
		fireEvent.input(screen.getByLabelText("Endpoint"), {
			target: { value: "https://local-draft.test" },
		});
		fireEvent.click(screen.getByRole("button", { name: /^save$/i }));

		await waitFor(() => {
			expect(screen.getByText(/conflict/i)).toBeTruthy();
			expect(screen.getByRole("button", { name: /keep remote/i })).toBeTruthy();
			expect(
				screen.getByRole("button", { name: /abandon draft/i }),
			).toBeTruthy();
		});

		fireEvent.click(screen.getByRole("button", { name: /keep remote/i }));
		await waitFor(() => {
			expect(screen.getByDisplayValue("https://remote.test")).toBeTruthy();
			expect(screen.getByText("remote-version")).toBeTruthy();
		});

		fireEvent.input(screen.getByLabelText("Endpoint"), {
			target: { value: "https://second-local-draft.test" },
		});
		fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
		await waitFor(() => {
			expect(screen.getByText(/conflict/i)).toBeTruthy();
		});
		fireEvent.click(screen.getByRole("button", { name: /abandon draft/i }));
		await waitFor(() => {
			expect(screen.getByDisplayValue("https://remote.test")).toBeTruthy();
			expect(
				screen.queryByDisplayValue("https://second-local-draft.test"),
			).toBeNull();
		});
	});

	// risk: Migration mutating or interpreting central config; level: provider settings UI; source: contract "Migration shows provider-returned actions"
	it("shows migration dry-run actions and warnings returned by the provider", async () => {
		const ProviderSettingsPanel = await loadPanel();
		const calls: { command: string; args?: any }[] = [];
		installHappyPathHandlers(calls);
		setHandler("migrate_provider_settings", (args) => {
			calls.push({ command: "migrate_provider_settings", args });
			return Promise.resolve({
				actions: [{ kind: "would-write", target: "record" }],
				warnings: ["Review before applying"],
				requiresUserInput: false,
				diagnostics: [
					{ severity: "info", message: "Legacy payload inspected", path: "/" },
				],
			});
		});

		render(() => <ProviderSettingsPanel />);
		await chooseTarget();
		fireEvent.click(screen.getByRole("button", { name: /migration/i }));
		fireEvent.click(screen.getByRole("button", { name: /dry run/i }));

		await waitFor(() => {
			expect(screen.getByText("would-write")).toBeTruthy();
			expect(screen.getByText("Review before applying")).toBeTruthy();
			expect(screen.getByText("Legacy payload inspected")).toBeTruthy();
		});
		expect(
			calls.find((call) => call.command === "migrate_provider_settings")?.args,
		).toEqual({
			accountName: "provider-a",
			dryRun: true,
			legacy: null,
		});
	});
});

function renderWithQuery(ui: () => any) {
	const queryClient = new QueryClient({
		defaultOptions: { queries: { retry: false } },
	});
	return render(() => (
		<QueryClientProvider client={queryClient}>{ui()}</QueryClientProvider>
	));
}

async function loadPanel() {
	const module = await import(
		"../components/provider-settings/ProviderSettingsPanel"
	);
	const component = module.ProviderSettingsPanel ?? module.default;
	expect(
		typeof component,
		"ProviderSettingsPanel must be exported from provider settings components",
	).toBe("function");
	return component;
}

async function chooseTarget() {
	await waitFor(() => {
		expect(screen.getByText("Provider A")).toBeTruthy();
	});
	fireEvent.click(screen.getByText("Provider A"));
}

function installHappyPathHandlers(calls: { command: string; args?: any }[]) {
	setHandler("list_provider_settings_targets", (args) => {
		calls.push({ command: "list_provider_settings_targets", args });
		return Promise.resolve([
			{
				accountName: "provider-a",
				displayName: "Provider A",
				settingsSupported: true,
				schemaId: "example.settings/v1",
			},
		]);
	});
	setHandler("get_provider_settings_schema", (args) => {
		calls.push({ command: "get_provider_settings_schema", args });
		return Promise.resolve({
			schemaId: "example.settings/v1",
			schema: {
				type: "object",
				required: ["endpoint"],
				properties: {
					endpoint: { type: "string", title: "Endpoint" },
					enabled: { type: "boolean", title: "Enabled" },
				},
			},
			ui: { order: ["endpoint", "enabled"] },
		});
	});
	setHandler("list_provider_settings", (args) => {
		calls.push({ command: "list_provider_settings", args });
		return Promise.resolve({
			records: [
				{
					id: "record",
					displayName: "Record",
					version: "opaque-version",
					summary: { endpoint: "https://example.test" },
				},
			],
		});
	});
	setHandler("get_provider_settings", (args) => {
		calls.push({ command: "get_provider_settings", args });
		return Promise.resolve({ record: providerRecord("opaque-version") });
	});
	setHandler("create_provider_settings", (args) => {
		calls.push({ command: "create_provider_settings", args });
		return Promise.resolve({
			record: providerRecord("provider-created-version", args.values),
			diagnostics: [],
		});
	});
	setHandler("update_provider_settings", (args) => {
		calls.push({ command: "update_provider_settings", args });
		return Promise.resolve({
			record: providerRecord("provider-updated-version", {
				endpoint: "https://provider-returned.test",
			}),
			diagnostics: [],
		});
	});
	setHandler("delete_provider_settings", (args) => {
		calls.push({ command: "delete_provider_settings", args });
		return Promise.resolve({ deleted: true, id: args.id });
	});
	setHandler("validate_provider_settings", (args) => {
		calls.push({ command: "validate_provider_settings", args });
		return Promise.resolve({ valid: true, diagnostics: [] });
	});
	setHandler("migrate_provider_settings", (args) => {
		calls.push({ command: "migrate_provider_settings", args });
		return Promise.resolve({
			actions: [],
			warnings: [],
			requiresUserInput: false,
			diagnostics: [],
		});
	});
}

function providerRecord(
	version: string,
	values = { endpoint: "https://example.test" },
) {
	return {
		id: "record",
		displayName: "Record",
		version,
		values,
	};
}

function deferred<T>() {
	let resolve!: (value: T) => void;
	let reject!: (reason?: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}
