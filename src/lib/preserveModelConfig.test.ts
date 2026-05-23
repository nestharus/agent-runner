import { describe, expect, it } from "vitest";
import { preserveModelConfig } from "./preserveModelConfig";
import type {
	InputDef,
	ModelConfig,
	ProviderConfig,
	ProviderImplementationRef,
} from "./types";

function providerForms(): ProviderConfig[] {
	return [{ name: "provider-a", args: ["--mode", "after"] }];
}

function existingModel(
	provider: ProviderImplementationRef | undefined,
): ModelConfig {
	return {
		name: "model-before",
		prompt_mode: "arg",
		providers: [{ name: "provider-a", args: ["--mode", "before"] }],
		inputs: inputDefs(),
		provider,
	};
}

function inputDefs(): InputDef[] {
	return [
		{
			name: "topic",
			input_type: { kind: "string" },
			required: true,
			default_input: true,
			description: "Prompt text",
			flag: "--topic",
		},
	];
}

function formInputs() {
	return {
		name: "model-after",
		providers: providerForms(),
	};
}

describe("preserveModelConfig", () => {
	// Risk: Test-intent item 6, unit level, A13/A178-4.
	it("preserves_provider_field_when_editing_existing_model", () => {
		const cases: ProviderImplementationRef[] = [
			{ crate: "agent-runner-example", version: "0.1" },
			{ path: "./agent-runner-example" },
			{ binary: "/usr/local/bin/example-provider" },
			{ script: "scripts/example-provider-locate" },
		];

		for (const provider of cases) {
			const existing = existingModel(provider);

			const result = preserveModelConfig(existing, formInputs());

			expect(result.provider).toEqual(existing.provider);
		}
	});

	// Risk: Test-intent item 6, unit level, A13/A178-4.
	it("preserves_prompt_mode_and_inputs_when_editing_existing_model", () => {
		const existing = existingModel({
			binary: "/usr/local/bin/example-provider",
		});

		const result = preserveModelConfig(existing, formInputs());

		expect(result.prompt_mode).toBe(existing.prompt_mode);
		expect(result.inputs).toEqual(existing.inputs);
	});

	// Risk: Test-intent item 6, unit level, A13/A178-4.
	it("applies_form_inputs_for_name_and_providers", () => {
		const existing = existingModel({
			script: "scripts/example-provider-locate",
		});
		const inputs = formInputs();

		const result = preserveModelConfig(existing, inputs);

		expect(result.name).toBe(inputs.name);
		expect(result.providers).toEqual(inputs.providers);
	});

	// Risk: Test-intent item 6, unit level, A13/A178-4.
	it("defaults_for_new_model", () => {
		const result = preserveModelConfig(undefined, formInputs());

		expect(result.provider).toBeUndefined();
		expect(result.prompt_mode).toBe("stdin");
		expect(result.inputs).toEqual([]);
	});

	// Risk: Test-intent item 6 and A13 falsifiability binding, unit level.
	it("roundtrip_through_buildModelConfig", () => {
		const fetchedModel = existingModel({
			path: "./agent-runner-example",
		});
		function buildModelConfig(): ModelConfig {
			return preserveModelConfig(fetchedModel, formInputs());
		}

		const savePayload = buildModelConfig();

		expect(savePayload.provider).toEqual(fetchedModel.provider);
		expect(savePayload.name).toBe("model-after");
		expect(savePayload.providers).toEqual(providerForms());
	});
});
