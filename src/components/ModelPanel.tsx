import { Dialog, Field } from "@ark-ui/solid";
import { faDiamond } from "@fortawesome/sharp-solid-svg-icons";
import { createResource, createSignal, For, Show } from "solid-js";
import type { ArgAnalysis } from "../lib/args";
import { analyzeProviderArgs } from "../lib/args";
import { resolveModelName } from "../lib/grouping";
import { getModel, saveModel, testModel } from "../lib/tauri";
import type {
	ModelConfig,
	ProviderConfig,
	TestModelResult,
} from "../lib/types";
import Icon from "./Icon";
import InlineSpinner from "./InlineSpinner";

interface ModelPanelProps {
	mode: "add" | "edit";
	poolCommands: string[];
	modelNames: string[];
	editModelName?: string;
	group?: string;
	onSave: (config: ModelConfig) => void;
	onClose: () => void;
}

interface ProviderForm {
	command: string;
	analysis: ArgAnalysis;
	variableValues: Record<string, string>;
}

export default function ModelPanel(props: ModelPanelProps) {
	const isAddToGroup = () => props.mode === "add" && !!props.group;
	const [facetName, setFacetName] = createSignal("");
	const [modelName, setModelName] = createSignal(props.editModelName ?? "");
	const [providerForms, setProviderForms] = createSignal<ProviderForm[]>([]);
	const [saving, setSaving] = createSignal(false);
	const [testing, setTesting] = createSignal(false);
	const [testResult, setTestResult] = createSignal<TestModelResult | null>(
		null,
	);
	const [error, setError] = createSignal<string | null>(null);

	const effectiveName = () => {
		if (isAddToGroup() && props.group) {
			const f = facetName().trim();
			return f ? resolveModelName(props.group, f) : "";
		}
		return modelName().trim();
	};

	const [existingModels] = createResource(async () => {
		const models: ModelConfig[] = [];
		for (const name of props.modelNames) {
			try {
				const m = await getModel(name);
				models.push(m);
			} catch {
				// skip models that fail to load
			}
		}
		return models;
	});

	function buildProviderForms(models: ModelConfig[]) {
		const forms: ProviderForm[] = [];

		for (const cmd of props.poolCommands) {
			const entries: { modelName: string; args: string[] }[] = [];
			for (const model of models) {
				for (const provider of model.providers) {
					if (provider.command === cmd) {
						entries.push({ modelName: model.name, args: provider.args });
					}
				}
			}

			const analysis = analyzeProviderArgs(entries);

			const variableValues: Record<string, string> = {};
			if (props.mode === "edit" && props.editModelName) {
				for (const v of analysis.variable) {
					if (v.examples[props.editModelName]) {
						variableValues[v.key] = v.examples[props.editModelName];
					}
				}
			} else {
				for (const v of analysis.variable) {
					variableValues[v.key] = "";
				}
			}

			forms.push({ command: cmd, analysis, variableValues });
		}

		setProviderForms(forms);
	}

	createResource(
		() => existingModels(),
		(models) => {
			if (models) buildProviderForms(models);
			return undefined;
		},
	);

	function buildModelConfig(): ModelConfig {
		const name = effectiveName();
		const providers: ProviderConfig[] = [];

		for (const form of providerForms()) {
			const args: string[] = [];
			for (const common of form.analysis.common) {
				if (common.key.startsWith("--dangerously-") || common.key === "--yolo")
					continue;
				if (common.key) args.push(common.key);
				if (common.value !== null) args.push(common.value);
			}
			for (const v of form.analysis.variable) {
				const val = form.variableValues[v.key];
				if (v.key) args.push(v.key);
				if (val) args.push(val);
			}

			providers.push({ command: form.command, args });
		}

		return { name, prompt_mode: "stdin", providers };
	}

	async function handleSaveAndTest() {
		const name = effectiveName();
		if (!name) {
			setError(
				isAddToGroup() ? "Facet name is required" : "Model name is required",
			);
			return;
		}

		setError(null);
		setSaving(true);
		setTestResult(null);

		try {
			const config = buildModelConfig();
			await saveModel(config);
			setSaving(false);

			setTesting(true);
			try {
				const result = await testModel(name);
				setTestResult(result);
			} catch (err) {
				setTestResult({
					success: false,
					stdout: "",
					stderr: String(err),
					exit_code: -1,
				});
			}
			setTesting(false);

			props.onSave(buildModelConfig());
		} catch (err) {
			setError(String(err));
			setSaving(false);
		}
	}

	function updateVariableValue(formIndex: number, key: string, value: string) {
		setProviderForms((prev) => {
			const next = [...prev];
			next[formIndex] = {
				...next[formIndex],
				variableValues: { ...next[formIndex].variableValues, [key]: value },
			};
			return next;
		});
	}

	function firstExample(examples: Record<string, string>): string {
		const values = Object.values(examples);
		return values[0] ?? "";
	}

	return (
		<Dialog.Root
			open={true}
			onOpenChange={(e) => {
				if (!e.open) props.onClose();
			}}
			closeOnEscape
		>
			<Dialog.Backdrop class="fixed inset-0 z-40 bg-black/40" />
			<Dialog.Positioner class="fixed inset-0 z-50 flex justify-end">
				<Dialog.Content class="animate-slide-in relative flex h-full w-96 flex-col shadow-lg">
					{/* Docking indicator */}
					<div class="absolute left-0 top-0 h-full w-0.5 bg-accent" />

					<div class="flex h-full w-full flex-col bg-surface pl-1">
						{/* Header */}
						<div class="border-b border-border px-4 py-3">
							<Dialog.Title class="text-sm font-medium text-accent">
								<Show
									when={props.mode === "add"}
									fallback={`Edit Model: ${props.editModelName}`}
								>
									<Show when={isAddToGroup()} fallback="Add Model">
										{`Add to ${props.group}`}
									</Show>
								</Show>
							</Dialog.Title>
						</div>

						{/* Body */}
						{/* TODO(design): Panel has large blank space when model has few options. */}
						{/* Need subtle background treatment (very low opacity geometric pattern). */}
						{/* Still needs design from designer. */}
						<div class="flex-1 overflow-y-auto px-4 py-4">
							{/* Facet name (add to group) */}
							<Show when={isAddToGroup()}>
								<Field.Root class="mb-4">
									<Field.Label class="mb-1 block text-xs text-text-dim">
										Facet name
									</Field.Label>
									<div class="flex items-center gap-1">
										<span class="text-xs font-mono text-text-faint">
											{props.group}~
										</span>
										<Field.Input
											type="text"
											class="flex-1 rounded border border-border bg-surface-alt px-3 py-2 text-sm font-mono text-text outline-none transition-colors focus:border-accent"
											value={facetName()}
											onInput={(e) => setFacetName(e.currentTarget.value)}
											placeholder="e.g. high, low, medium"
											ref={(el) => requestAnimationFrame(() => el.focus())}
										/>
									</div>
								</Field.Root>
							</Show>

							{/* Model name (add mode, not in group) */}
							<Show when={props.mode === "add" && !props.group}>
								<Field.Root class="mb-4">
									<Field.Label class="mb-1 block text-xs text-text-dim">
										Model name
									</Field.Label>
									<Field.Input
										type="text"
										class="w-full rounded border border-border bg-surface-alt px-3 py-2 text-sm font-mono text-text outline-none transition-colors focus:border-accent"
										value={modelName()}
										onInput={(e) => setModelName(e.currentTarget.value)}
										ref={(el) => requestAnimationFrame(() => el.focus())}
									/>
								</Field.Root>
							</Show>

							{/* Provider forms */}
							<Show
								when={!existingModels.loading}
								fallback={
									<p class="text-xs text-text-dim">Loading model data...</p>
								}
							>
								<For each={providerForms()}>
									{(form, formIndex) => (
										<div class="mb-5">
											{/* Provider divider */}
											<div class="mb-3 flex items-center gap-2">
												<div class="h-px flex-1 bg-border" />
												<span class="text-xs font-mono text-text-dim">
													{form.command}
												</span>
												<div class="h-px flex-1 bg-border" />
											</div>

											{/* Variable args */}
											<For each={form.analysis.variable}>
												{(v) => (
													<Field.Root class="mb-3">
														<Field.Label class="mb-1 block text-xs text-text-dim">
															{v.key || "positional"}
														</Field.Label>
														<Field.Input
															type="text"
															class="w-full rounded border border-border bg-surface-alt px-3 py-1.5 text-sm font-mono text-text outline-none transition-colors focus:border-accent"
															value={form.variableValues[v.key] ?? ""}
															placeholder={firstExample(v.examples)}
															onInput={(e) =>
																updateVariableValue(
																	formIndex(),
																	v.key,
																	e.currentTarget.value,
																)
															}
														/>
													</Field.Root>
												)}
											</For>

											<Show
												when={
													form.analysis.common.length === 0 &&
													form.analysis.variable.length === 0
												}
											>
												<p class="text-xs text-text-faint">
													No args configured for this provider
												</p>
											</Show>
										</div>
									)}
								</For>
							</Show>

							{/* Error */}
							<Show when={error()}>
								<div class="mb-3 rounded border-l-[3px] border-error bg-error/15 p-2 text-xs text-error">
									{error()}
								</div>
							</Show>

							{/* Test result */}
							<Show when={testing()}>
								<div class="mb-3 flex items-center gap-2 text-xs text-text-dim">
									<InlineSpinner size={14} />
									Testing model...
								</div>
							</Show>

							<Show when={testResult()}>
								{(result) => (
									<div
										class={`mb-3 rounded p-2 text-xs ${
											result().success
												? "bg-success/15 text-success"
												: "bg-error/15 text-error"
										}`}
									>
										<div class="mb-1 font-medium">
											{result().success ? "Test passed" : "Test failed"}
											{result().exit_code !== 0 &&
												` (exit ${result().exit_code})`}
										</div>
										<Show when={result().stdout}>
											<pre class="max-h-24 overflow-auto whitespace-pre-wrap font-mono text-[10px] opacity-80">
												{result().stdout.slice(0, 500)}
											</pre>
										</Show>
										<Show when={result().stderr && !result().success}>
											<pre class="max-h-24 overflow-auto whitespace-pre-wrap font-mono text-[10px] opacity-80">
												{result().stderr.slice(0, 500)}
											</pre>
										</Show>
									</div>
								)}
							</Show>
						</div>

						{/* Footer */}
						<div class="border-t border-border px-4 py-3">
							<button
								type="button"
								class="flex w-full items-center justify-center gap-2 rounded-lg bg-accent py-2.5 text-sm font-medium text-black transition-colors hover:bg-accent-hover disabled:opacity-50"
								onClick={handleSaveAndTest}
								disabled={saving() || testing()}
							>
								{saving()
									? "Saving..."
									: testing()
										? "Testing..."
										: "Save & Test"}
								<Show when={!saving() && !testing()}>
									<Icon icon={faDiamond} size={14} />
								</Show>
							</button>
						</div>
					</div>
				</Dialog.Content>
			</Dialog.Positioner>
		</Dialog.Root>
	);
}
