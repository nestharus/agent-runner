import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { beforeEach, describe, expect, it, vi } from "vitest";

beforeEach(() => {
	cleanup();
	vi.clearAllMocks();
});

describe("provider settings JSON Schema renderer", () => {
	// risk: JSON value type loss in the renderer; level: Solid component; source: contract "JSON Schema renderer subset"
	it("renders supported primitive, nested, enum, array, nullable, default, required, and description fields without stringifying JSON values", async () => {
		const JsonSchemaRenderer = await loadRenderer();
		const onChange = vi.fn();

		render(() => (
			<JsonSchemaRenderer
				schema={exampleSchema()}
				ui={exampleUi()}
				values={{
					endpoint: "https://example.test",
					enabled: true,
					limit: 3,
					mode: "standard",
					tags: ["alpha"],
					nested: { threshold: 1.5 },
					nullableSetting: null,
					secret: "masked-value",
				}}
				diagnostics={[]}
				onChange={onChange}
			/>
		));

		expect(screen.getByText("Connection")).toBeTruthy();
		expect(screen.getByText("Endpoint")).toBeTruthy();
		expect(screen.getByText("Neutral endpoint description")).toBeTruthy();
		expect(screen.getByText("Enabled")).toBeTruthy();
		expect(screen.getByText("Limit")).toBeTruthy();
		expect(screen.getByText("Mode")).toBeTruthy();
		expect(screen.getByText("Tags")).toBeTruthy();
		expect(screen.getByText("Threshold")).toBeTruthy();
		expect(screen.getByText("Nullable Setting")).toBeTruthy();
		expect(screen.getByText("Secret")).toBeTruthy();

		const endpoint = screen.getByLabelText("Endpoint") as HTMLInputElement;
		fireEvent.input(endpoint, {
			target: { value: "https://example.test/next" },
		});
		expect(onChange).toHaveBeenLastCalledWith(
			expect.objectContaining({ endpoint: "https://example.test/next" }),
		);

		const enabled = screen.getByLabelText("Enabled") as HTMLInputElement;
		fireEvent.click(enabled);
		expect(onChange).toHaveBeenLastCalledWith(
			expect.objectContaining({ enabled: false }),
		);

		const limit = screen.getByLabelText("Limit") as HTMLInputElement;
		fireEvent.input(limit, { target: { value: "5" } });
		expect(onChange).toHaveBeenLastCalledWith(
			expect.objectContaining({ limit: 5 }),
		);
	});

	// risk: JSON value type loss in the renderer; level: renderer helper; source: contract "default"
	it("seeds create drafts from defaults without overwriting provider record values", async () => {
		const { buildInitialSettingsValues } = await import(
			"../lib/providerSettingsSchema"
		);

		expect(buildInitialSettingsValues(exampleSchema(), undefined)).toEqual({
			endpoint: "https://default.example.test",
			enabled: true,
		});
		expect(
			buildInitialSettingsValues(exampleSchema(), {
				endpoint: "https://provider-record.test",
				enabled: false,
				limit: 7,
			}),
		).toEqual({
			endpoint: "https://provider-record.test",
			enabled: false,
			limit: 7,
		});
	});

	// risk: JSON value type loss in the renderer; level: Solid component; source: contract "required"
	it("renders required-field affordances and local missing-value hints", async () => {
		const JsonSchemaRenderer = await loadRenderer();
		render(() => (
			<JsonSchemaRenderer
				schema={exampleSchema()}
				ui={exampleUi()}
				values={{ enabled: true }}
				diagnostics={[]}
				onChange={vi.fn()}
			/>
		));

		const endpoint = screen.getByLabelText("Endpoint");
		expect(endpoint.getAttribute("aria-required")).toBe("true");
		expect(screen.getByText(/required/i)).toBeTruthy();
	});

	// risk: JSON value type loss in the renderer; level: Solid component; source: contract "enum"
	it("uses provider-supplied enum labels and preserves selected enum values", async () => {
		const JsonSchemaRenderer = await loadRenderer();
		const onChange = vi.fn();
		render(() => (
			<JsonSchemaRenderer
				schema={exampleSchema()}
				ui={exampleUi()}
				values={{ mode: "standard" }}
				diagnostics={[]}
				onChange={onChange}
			/>
		));

		const mode = screen.getByLabelText("Mode") as HTMLSelectElement;
		expect(screen.getByText("Standard label")).toBeTruthy();
		expect(screen.getByText("Expanded label")).toBeTruthy();
		fireEvent.change(mode, { target: { value: "expanded" } });
		expect(onChange).toHaveBeenLastCalledWith(
			expect.objectContaining({ mode: "expanded" }),
		);
	});

	// risk: JSON value type loss in the renderer; level: Solid component; source: contract "array"
	it("edits simple arrays while preserving JSON array values", async () => {
		const JsonSchemaRenderer = await loadRenderer();
		const onChange = vi.fn();
		render(() => (
			<JsonSchemaRenderer
				schema={exampleSchema()}
				ui={exampleUi()}
				values={{ tags: ["alpha"] }}
				diagnostics={[]}
				onChange={onChange}
			/>
		));

		fireEvent.click(screen.getByRole("button", { name: /add tag/i }));
		fireEvent.input(screen.getByLabelText("Tags 2"), {
			target: { value: "beta" },
		});
		expect(onChange).toHaveBeenLastCalledWith(
			expect.objectContaining({ tags: ["alpha", "beta"] }),
		);
	});

	// risk: JSON value type loss in the renderer; level: Solid component; source: contract "nullable"
	it("supports nullable clear and unset affordances without coercing null to a string", async () => {
		const JsonSchemaRenderer = await loadRenderer();
		const onChange = vi.fn();
		render(() => (
			<JsonSchemaRenderer
				schema={exampleSchema()}
				ui={exampleUi()}
				values={{ nullableSetting: "present" }}
				diagnostics={[]}
				onChange={onChange}
			/>
		));

		fireEvent.click(
			screen.getByRole("button", { name: /clear nullable setting/i }),
		);
		expect(onChange).toHaveBeenLastCalledWith(
			expect.objectContaining({ nullableSetting: null }),
		);
		fireEvent.click(
			screen.getByRole("button", { name: /unset nullable setting/i }),
		);
		const latest = onChange.mock.calls.at(-1)?.[0] as Record<string, unknown>;
		expect(Object.hasOwn(latest, "nullableSetting")).toBe(false);
	});

	// risk: JSON value type loss in the renderer; level: Solid component; source: contract "secret/masked hints"
	it("renders secret fields masked and does not expose the secret as visible text", async () => {
		const JsonSchemaRenderer = await loadRenderer();
		render(() => (
			<JsonSchemaRenderer
				schema={exampleSchema()}
				ui={exampleUi()}
				values={{ secret: "masked-value" }}
				diagnostics={[]}
				onChange={vi.fn()}
			/>
		));

		const secret = screen.getByLabelText("Secret") as HTMLInputElement;
		expect(secret.type).toBe("password");
		expect(screen.queryByText("masked-value")).toBeNull();
	});

	// risk: JSON value type loss in the renderer; level: Solid component; source: contract "Unsupported constructs degrade generically"
	it("degrades unsupported schema constructs generically and preserves raw JSON safely", async () => {
		const JsonSchemaRenderer = await loadRenderer();
		const onChange = vi.fn();
		const values = { endpoint: "https://example.test", opaque: { keep: true } };

		render(() => (
			<JsonSchemaRenderer
				schema={{
					type: "object",
					properties: {
						endpoint: { type: "string", title: "Endpoint" },
						opaque: {
							oneOf: [{ type: "string" }, { type: "object" }],
							title: "Opaque",
						},
						mapSetting: {
							type: "object",
							additionalProperties: true,
							title: "Map Setting",
						},
					},
				}}
				ui={{}}
				values={values}
				diagnostics={[]}
				onChange={onChange}
			/>
		));

		expect(screen.getByText("Unsupported setting")).toBeTruthy();
		expect(screen.getByText("Opaque")).toBeTruthy();
		expect(screen.getByText("Map Setting")).toBeTruthy();
		expect(findPreformattedJson(values.opaque)).toBeTruthy();
	});

	// risk: Provider diagnostics loss; level: renderer helper; source: contract "Diagnostic JSON Pointer mapping"
	it("maps exact, nested, array, hidden, unsupported, and unknown JSON Pointer diagnostics correctly", async () => {
		const { mapProviderDiagnostics } = await import(
			"../lib/providerSettingsSchema"
		);

		const unsupportedSchema = {
			...exampleSchema(),
			properties: {
				...exampleSchema().properties,
				opaque: { oneOf: [{ type: "string" }, { type: "object" }] },
			},
		};
		const mapped = mapProviderDiagnostics(unsupportedSchema, exampleUi(), [
			{ severity: "error", message: "Endpoint is required", path: "/endpoint" },
			{
				severity: "warning",
				message: "Threshold is high",
				path: "/nested/threshold",
			},
			{ severity: "info", message: "First tag note", path: "/tags/0" },
			{ severity: "warning", message: "Hidden note", path: "/secret" },
			{
				severity: "warning",
				message: "Unsupported field note",
				path: "/opaque",
			},
			{ severity: "error", message: "Unknown note", path: "/unknown" },
		]);

		expect(mapped.fields["/endpoint"][0].message).toBe("Endpoint is required");
		expect(mapped.fields["/nested/threshold"][0].message).toBe(
			"Threshold is high",
		);
		expect(mapped.fields["/tags/0"][0].message).toBe("First tag note");
		expect(mapped.summary.map((diagnostic: any) => diagnostic.message)).toEqual(
			["Hidden note", "Unsupported field note", "Unknown note"],
		);
	});
});

function findPreformattedJson(value: unknown): HTMLElement {
	return screen.getByText((_content, element) => {
		return (
			isPreElement(element) &&
			compactText(element.textContent) === compactJson(value)
		);
	});
}

function isPreElement(element: Element | null): element is HTMLElement {
	return element?.tagName.toLowerCase() === "pre";
}

function compactText(text: string | null): string {
	return (text ?? "").replace(/\s+/g, "");
}

function compactJson(value: unknown): string {
	return JSON.stringify(value).replace(/\s+/g, "");
}

async function loadRenderer() {
	const module = await import(
		"../components/provider-settings/JsonSchemaRenderer"
	);
	const component = module.JsonSchemaRenderer ?? module.default;
	expect(
		typeof component,
		"JsonSchemaRenderer must be exported from provider settings components",
	).toBe("function");
	return component;
}

function exampleSchema() {
	return {
		type: "object",
		required: ["endpoint", "enabled"],
		properties: {
			endpoint: {
				type: "string",
				title: "Endpoint",
				description: "Neutral endpoint description",
				default: "https://default.example.test",
			},
			enabled: { type: "boolean", title: "Enabled", default: true },
			limit: { type: "integer", title: "Limit", minimum: 1, maximum: 9 },
			mode: {
				type: "string",
				title: "Mode",
				enum: ["standard", "expanded"],
			},
			tags: {
				type: "array",
				title: "Tags",
				items: { type: "string" },
			},
			nested: {
				type: "object",
				title: "Nested",
				properties: {
					threshold: { type: "number", title: "Threshold" },
				},
			},
			nullableSetting: {
				type: ["string", "null"],
				title: "Nullable Setting",
			},
			secret: {
				type: "string",
				title: "Secret",
			},
		},
	};
}

function exampleUi() {
	return {
		sections: [
			{
				id: "connection",
				title: "Connection",
				order: ["endpoint", "enabled", "secret"],
			},
			{
				id: "advanced",
				title: "Advanced",
				order: ["limit", "mode", "tags", "nested", "nullableSetting"],
			},
		],
		order: ["endpoint", "enabled", "limit", "mode", "tags", "nested"],
		fields: {
			mode: {
				enumLabels: {
					standard: "Standard label",
					expanded: "Expanded label",
				},
			},
			secret: { secret: true, masked: true, hiddenDiagnostics: true },
		},
	};
}
