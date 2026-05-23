import type { ModelConfig, ProviderConfig } from "./types";

/**
 * Form-controlled fields the panel can edit.
 * Anything not in this shape is preserved from `existing` verbatim.
 */
export interface ModelConfigFormInputs {
	name: string;
	providers: ProviderConfig[];
}

export function preserveModelConfig(
	existing: ModelConfig | undefined,
	formInputs: ModelConfigFormInputs,
): ModelConfig {
	if (!existing) {
		return {
			name: formInputs.name,
			prompt_mode: "stdin",
			providers: formInputs.providers,
			inputs: [],
		};
	}

	return {
		...existing,
		name: formInputs.name,
		providers: formInputs.providers,
	};
}
