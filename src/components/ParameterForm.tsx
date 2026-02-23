import { Field, Switch } from "@ark-ui/solid";
import { For, Show } from "solid-js";
import type { Parameter } from "../lib/types";

interface ParameterFormProps {
	parameters: Parameter[];
	values: Record<string, string>;
	onChange: (values: Record<string, string>) => void;
}

export default function ParameterForm(props: ParameterFormProps) {
	function updateValue(name: string, value: string) {
		props.onChange({ ...props.values, [name]: value });
	}

	return (
		<div class="space-y-4">
			<For each={props.parameters}>
				{(param) => (
					<ParameterField
						param={param}
						value={props.values[param.name] ?? ""}
						onValueChange={(v) => updateValue(param.name, v)}
					/>
				)}
			</For>
		</div>
	);
}

interface ParameterFieldProps {
	param: Parameter;
	value: string;
	onValueChange: (value: string) => void;
}

function ParameterField(props: ParameterFieldProps) {
	const pt = () => props.param.param_type;

	return (
		<div class="mb-3">
			<Show when={pt().type === "enum"}>
				<EnumField
					param={props.param}
					value={props.value}
					onValueChange={props.onValueChange}
				/>
			</Show>
			<Show when={pt().type === "boolean"}>
				<BooleanField
					param={props.param}
					value={props.value}
					onValueChange={props.onValueChange}
				/>
			</Show>
			<Show when={pt().type === "number"}>
				<NumberField
					param={props.param}
					value={props.value}
					onValueChange={props.onValueChange}
				/>
			</Show>
			<Show when={pt().type === "string"}>
				<StringField
					param={props.param}
					value={props.value}
					onValueChange={props.onValueChange}
				/>
			</Show>
		</div>
	);
}

function EnumField(props: ParameterFieldProps) {
	const options = () => {
		const pt = props.param.param_type;
		return pt.type === "enum" ? pt.options : [];
	};

	return (
		<Field.Root>
			<Field.Label class="mb-1.5 block text-xs font-medium text-text">
				{props.param.display_name}
			</Field.Label>
			<div class="flex flex-wrap gap-1.5">
				<For each={options()}>
					{(option) => (
						<label
							class={`cursor-pointer rounded-full border px-3 py-1 text-xs font-medium transition-colors ${
								props.value === option
									? "border-accent bg-accent/20 text-accent"
									: "border-border bg-surface-alt text-text-dim hover:border-text-faint hover:text-text"
							}`}
						>
							<input
								type="radio"
								name={props.param.name}
								value={option}
								checked={props.value === option}
								class="sr-only"
								onChange={() => props.onValueChange(option)}
							/>
							{option}
						</label>
					)}
				</For>
			</div>
			<Show when={props.param.description}>
				<Field.HelperText class="mt-1 text-[10px] text-text-faint">
					{props.param.description}
				</Field.HelperText>
			</Show>
		</Field.Root>
	);
}

function BooleanField(props: ParameterFieldProps) {
	const checked = () => props.value === "true";

	return (
		<Switch.Root
			checked={checked()}
			onCheckedChange={(e) => props.onValueChange(e.checked ? "true" : "false")}
			class="flex cursor-pointer items-center justify-between gap-3"
		>
			<div>
				<Switch.Label class="text-xs font-medium text-text">
					{props.param.display_name}
				</Switch.Label>
				<Show when={props.param.description}>
					<div class="text-[10px] text-text-faint">
						{props.param.description}
					</div>
				</Show>
			</div>
			<Switch.Control class="relative h-5 w-9 rounded-full bg-border transition-colors data-[state=checked]:bg-accent">
				<Switch.Thumb class="absolute left-0.5 top-0.5 h-4 w-4 rounded-full bg-text-dim transition-transform data-[state=checked]:translate-x-4 data-[state=checked]:bg-white" />
			</Switch.Control>
			<Switch.HiddenInput />
		</Switch.Root>
	);
}

function NumberField(props: ParameterFieldProps) {
	const pt = () => props.param.param_type;
	const min = () => {
		const p = pt();
		return p.type === "number" ? p.min : undefined;
	};
	const max = () => {
		const p = pt();
		return p.type === "number" ? p.max : undefined;
	};

	return (
		<Field.Root>
			<Field.Label class="mb-1 block text-xs font-medium text-text">
				{props.param.display_name}
			</Field.Label>
			<Field.Input
				type="number"
				min={min()}
				max={max()}
				class="w-full rounded border border-border bg-surface-alt px-3 py-1.5 text-sm font-mono text-text outline-none transition-colors focus:border-accent"
				value={props.value}
				onInput={(e) => props.onValueChange(e.currentTarget.value)}
			/>
			<Show when={props.param.description}>
				<Field.HelperText class="mt-1 text-[10px] text-text-faint">
					{props.param.description}
				</Field.HelperText>
			</Show>
		</Field.Root>
	);
}

function StringField(props: ParameterFieldProps) {
	return (
		<Field.Root>
			<Field.Label class="mb-1 block text-xs font-medium text-text">
				{props.param.display_name}
			</Field.Label>
			<Field.Input
				type="text"
				class="w-full rounded border border-border bg-surface-alt px-3 py-1.5 text-sm font-mono text-text outline-none transition-colors focus:border-accent"
				value={props.value}
				onInput={(e) => props.onValueChange(e.currentTarget.value)}
			/>
			<Show when={props.param.description}>
				<Field.HelperText class="mt-1 text-[10px] text-text-faint">
					{props.param.description}
				</Field.HelperText>
			</Show>
		</Field.Root>
	);
}
