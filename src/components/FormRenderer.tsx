import { Field } from "@ark-ui/solid";
import { createSignal, For, Show } from "solid-js";
import { button, card, input } from "../lib/styles";
import type { FormAction, FormField as FormFieldType } from "../lib/types";

interface FormRendererProps {
	form: FormAction;
	onSubmit: (values: Record<string, string>) => void;
}

export default function FormRenderer(props: FormRendererProps) {
	const [errors, setErrors] = createSignal<Record<string, boolean>>({});

	let formRef!: HTMLDivElement;

	function collectValues(): Record<string, string> {
		const values: Record<string, string> = {};
		for (const f of props.form.fields) {
			if (f.field_type === "checkbox") {
				const cb = formRef.querySelector<HTMLInputElement>(
					`[name="${f.name}"]`,
				);
				values[f.name] = cb?.checked ? "true" : "false";
			} else if (f.field_type === "multi_select") {
				const checked: string[] = [];
				for (const cb of formRef.querySelectorAll<HTMLInputElement>(
					`[name="${f.name}"]:checked`,
				)) {
					checked.push(cb.value);
				}
				values[f.name] = checked.join(",");
			} else {
				const el = formRef.querySelector<
					HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement
				>(`[name="${f.name}"]`);
				values[f.name] = el?.value ?? "";
			}
		}
		return values;
	}

	function validate(): boolean {
		const errs: Record<string, boolean> = {};
		let valid = true;
		for (const f of props.form.fields) {
			if (!f.required) continue;
			const el = formRef.querySelector<HTMLInputElement>(`[name="${f.name}"]`);
			if (!el || !el.value.trim()) {
				errs[f.name] = true;
				valid = false;
			}
		}
		setErrors(errs);
		return valid;
	}

	function handleSubmit() {
		if (validate()) {
			props.onSubmit(collectValues());
		}
	}

	return (
		<div ref={formRef} class={card()}>
			<h3 class="mb-1 text-text">{props.form.title}</h3>
			<Show when={props.form.description}>
				<p class="mb-4 text-[13px] text-text-dim">{props.form.description}</p>
			</Show>

			<For each={props.form.fields}>
				{(f) => <FormFieldRenderer field={f} hasError={!!errors()[f.name]} />}
			</For>

			<div class="mt-4 flex justify-end gap-2">
				<button
					type="button"
					class={button({ intent: "primary" })}
					onClick={handleSubmit}
				>
					{props.form.submit_label || "Submit"}
				</button>
			</div>
		</div>
	);
}

function FormFieldRenderer(props: { field: FormFieldType; hasError: boolean }) {
	const inputClass = () => input({ error: props.hasError });

	if (
		props.field.field_type === "text" ||
		props.field.field_type === "password"
	) {
		return (
			<Field.Root
				invalid={props.hasError}
				required={props.field.required}
				class="mb-4"
			>
				<Field.Label class="mb-1 block text-[13px] font-medium text-text">
					{props.field.label}
					<Show when={props.field.required}>
						<span class="text-error"> *</span>
					</Show>
				</Field.Label>
				<Field.Input
					class={inputClass()}
					type={props.field.field_type}
					name={props.field.name}
					value={props.field.default_value ?? ""}
					placeholder={props.field.placeholder ?? ""}
				/>
				<Show when={props.hasError}>
					<Field.ErrorText class="mt-0.5 text-[11px] text-error">
						This field is required
					</Field.ErrorText>
				</Show>
				<Show when={props.field.help_text}>
					<Field.HelperText class="mt-0.5 text-[11px] text-text-dim">
						{props.field.help_text}
					</Field.HelperText>
				</Show>
			</Field.Root>
		);
	}

	if (props.field.field_type === "textarea") {
		return (
			<Field.Root
				invalid={props.hasError}
				required={props.field.required}
				class="mb-4"
			>
				<Field.Label class="mb-1 block text-[13px] font-medium text-text">
					{props.field.label}
					<Show when={props.field.required}>
						<span class="text-error"> *</span>
					</Show>
				</Field.Label>
				<Field.Textarea
					class={`${inputClass()} min-h-[80px] resize-y`}
					name={props.field.name}
					placeholder={props.field.placeholder ?? ""}
				>
					{props.field.default_value ?? ""}
				</Field.Textarea>
				<Show when={props.field.help_text}>
					<Field.HelperText class="mt-0.5 text-[11px] text-text-dim">
						{props.field.help_text}
					</Field.HelperText>
				</Show>
			</Field.Root>
		);
	}

	if (props.field.field_type === "select") {
		return (
			<Field.Root
				invalid={props.hasError}
				required={props.field.required}
				class="mb-4"
			>
				<Field.Label class="mb-1 block text-[13px] font-medium text-text">
					{props.field.label}
					<Show when={props.field.required}>
						<span class="text-error"> *</span>
					</Show>
				</Field.Label>
				<Field.Select
					class={`${inputClass()} cursor-pointer`}
					name={props.field.name}
				>
					<Show when={props.field.placeholder}>
						<option value="" disabled selected>
							{props.field.placeholder}
						</option>
					</Show>
					<For each={props.field.options ?? []}>
						{(opt) => (
							<option
								value={opt.value}
								selected={opt.value === props.field.default_value}
							>
								{opt.label}
							</option>
						)}
					</For>
				</Field.Select>
				<Show when={props.field.help_text}>
					<Field.HelperText class="mt-0.5 text-[11px] text-text-dim">
						{props.field.help_text}
					</Field.HelperText>
				</Show>
			</Field.Root>
		);
	}

	if (props.field.field_type === "checkbox") {
		return (
			<div class="mb-4">
				<label class="flex cursor-pointer items-center gap-2 text-sm">
					<input
						type="checkbox"
						name={props.field.name}
						checked={props.field.default_value === "true"}
					/>
					{props.field.label}
				</label>
				<Show when={props.field.help_text}>
					<div class="mt-0.5 text-[11px] text-text-dim">
						{props.field.help_text}
					</div>
				</Show>
			</div>
		);
	}

	if (props.field.field_type === "multi_select") {
		return (
			<div class="mb-4">
				<span class="mb-1 block text-[13px] font-medium text-text">
					{props.field.label}
				</span>
				<For each={props.field.options ?? []}>
					{(opt) => (
						<label class="flex cursor-pointer items-center gap-2 text-sm">
							<input
								type="checkbox"
								name={props.field.name}
								value={opt.value}
							/>
							{opt.label}
						</label>
					)}
				</For>
				<Show when={props.field.help_text}>
					<div class="mt-0.5 text-[11px] text-text-dim">
						{props.field.help_text}
					</div>
				</Show>
			</div>
		);
	}

	return null;
}
