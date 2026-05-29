import { For, Show } from "solid-js";
import {
	baseType,
	isNullable,
	isRequired,
	isUnsupportedSchema,
	mapProviderDiagnostics,
	orderedPropertyKeys,
	propertyLabel,
} from "../../lib/providerSettingsSchema";
import type { ProviderDiagnostic } from "../../lib/types";

interface JsonSchemaRendererProps {
	schema: Record<string, any>;
	ui?: Record<string, any> | null;
	values: Record<string, any>;
	diagnostics: ProviderDiagnostic[];
	onChange: (values: Record<string, any>) => void;
}

export function JsonSchemaRenderer(props: JsonSchemaRendererProps) {
	const mapped = () =>
		mapProviderDiagnostics(props.schema, props.ui ?? {}, props.diagnostics);

	function update(path: string[], value: unknown) {
		props.onChange(updatePathValue(props.values ?? {}, path, value));
	}

	function unset(key: string) {
		props.onChange(unsetTopLevelValue(props.values ?? {}, key));
	}

	return (
		<div class="space-y-5">
			<DiagnosticSummary diagnostics={mapped().summary} />
			<For each={schemaSections(props.schema, props.ui ?? {})}>
				{(section) => (
					<SchemaSection
						section={section}
						schema={props.schema}
						ui={props.ui ?? {}}
						values={props.values ?? {}}
						diagnostics={mapped().fields}
						onValueChange={update}
						onUnsetValue={unset}
					/>
				)}
			</For>
		</div>
	);
}

function DiagnosticSummary(props: { diagnostics: ProviderDiagnostic[] }) {
	return (
		<Show when={props.diagnostics.length > 0}>
			<div role="alert" class="rounded border border-warning/40 p-3 text-sm">
				<For each={props.diagnostics}>
					{(diagnostic) => <p>{diagnostic.message}</p>}
				</For>
			</div>
		</Show>
	);
}

interface SchemaSectionRow {
	title: string;
	keys: string[];
}

interface SchemaSectionProps {
	section: SchemaSectionRow;
	schema: Record<string, any>;
	ui: Record<string, any>;
	values: Record<string, any>;
	diagnostics: Record<string, ProviderDiagnostic[]>;
	onValueChange: (path: string[], value: unknown) => void;
	onUnsetValue: (key: string) => void;
}

function SchemaSection(props: SchemaSectionProps) {
	return (
		<section class="space-y-3">
			<Show when={props.section.title}>
				<h3 class="text-sm font-medium text-text">{props.section.title}</h3>
			</Show>
			<For each={props.section.keys}>
				{(key) => (
					<SchemaField
						rootSchema={props.schema}
						ui={props.ui}
						fieldKey={key}
						fieldSchema={props.schema.properties[key]}
						value={props.values?.[key]}
						values={props.values}
						diagnostics={props.diagnostics}
						onValueChange={props.onValueChange}
						onUnsetValue={props.onUnsetValue}
					/>
				)}
			</For>
		</section>
	);
}

interface SchemaFieldProps {
	rootSchema: Record<string, any>;
	ui: Record<string, any>;
	fieldKey: string;
	fieldSchema: Record<string, any>;
	value: any;
	values: Record<string, any>;
	diagnostics: Record<string, ProviderDiagnostic[]>;
	onValueChange: (path: string[], value: unknown) => void;
	onUnsetValue: (key: string) => void;
}

function SchemaField(props: SchemaFieldProps) {
	const fieldPath = () => [props.fieldKey];
	const nestedPath = (path: string[]) => [props.fieldKey, ...path];
	const fieldUi = () => props.ui.fields?.[props.fieldKey] ?? {};

	function changeValue(value: unknown) {
		props.onValueChange(fieldPath(), value);
	}

	function changeNestedValue(path: string[], value: unknown) {
		props.onValueChange(nestedPath(path), value);
	}

	function unsetValue() {
		props.onUnsetValue(props.fieldKey);
	}

	return (
		<FieldFrame
			rootSchema={props.rootSchema}
			fieldKey={props.fieldKey}
			fieldSchema={props.fieldSchema}
			value={props.value}
			values={props.values}
			fieldUi={fieldUi()}
			diagnostics={props.diagnostics}
			onChange={changeValue}
			onNestedChange={changeNestedValue}
			onUnset={unsetValue}
		/>
	);
}

interface FieldFrameProps {
	rootSchema: Record<string, any>;
	fieldKey: string;
	fieldSchema: Record<string, any>;
	value: any;
	values: Record<string, any>;
	fieldUi: Record<string, any>;
	diagnostics: Record<string, ProviderDiagnostic[]>;
	onChange: (value: unknown) => void;
	onNestedChange: (path: string[], value: unknown) => void;
	onUnset: () => void;
}

function FieldFrame(props: FieldFrameProps) {
	const label = () => propertyLabel(props.fieldKey, props.fieldSchema);
	const pointer = () => `/${escapePointer(props.fieldKey)}`;
	const fieldDiagnostics = () => props.diagnostics[pointer()] ?? [];
	const invalid = () => fieldDiagnostics().length > 0;
	const required = () => isRequired(props.rootSchema, props.fieldKey);
	const requiredMissing = () =>
		required() && (props.value === undefined || props.value === "");

	return (
		<div class="space-y-1">
			<Show
				when={!isUnsupportedSchema(props.fieldSchema)}
				fallback={<UnsupportedField title={label()} value={props.value} />}
			>
				<div class="block text-xs font-medium text-text">
					<span>{label()}</span>
					<FieldDescription schema={props.fieldSchema} />
					<FieldInputDispatcher
						label={label()}
						type={baseType(props.fieldSchema)}
						schema={props.fieldSchema}
						value={props.value}
						values={props.values}
						ui={props.fieldUi}
						invalid={invalid()}
						required={required()}
						onChange={props.onChange}
						onNestedChange={props.onNestedChange}
					/>
				</div>
				<RequiredHint show={requiredMissing()} label={label()} />
				<FieldDiagnostics diagnostics={fieldDiagnostics()} />
				<NullableActions
					show={isNullable(props.fieldSchema)}
					label={label()}
					onClear={props.onChange}
					onUnset={props.onUnset}
				/>
			</Show>
		</div>
	);
}

function FieldDescription(props: { schema: Record<string, any> }) {
	return (
		<Show when={props.schema.description}>
			<p class="mt-1 font-normal text-text-dim">{props.schema.description}</p>
		</Show>
	);
}

function RequiredHint(props: { show: boolean; label: string }) {
	return (
		<Show when={props.show}>
			<p class="text-xs text-error">{props.label} is required.</p>
		</Show>
	);
}

function FieldDiagnostics(props: { diagnostics: ProviderDiagnostic[] }) {
	return (
		<For each={props.diagnostics}>
			{(diagnostic) => <p class="text-xs text-error">{diagnostic.message}</p>}
		</For>
	);
}

function NullableActions(props: {
	show: boolean;
	label: string;
	onClear: (value: unknown) => void;
	onUnset: () => void;
}) {
	function clearValue() {
		props.onClear(null);
	}

	return (
		<Show when={props.show}>
			<div class="flex gap-2">
				<button type="button" onClick={clearValue}>
					Clear {props.label}
				</button>
				<button type="button" onClick={props.onUnset}>
					Unset {props.label}
				</button>
			</div>
		</Show>
	);
}

interface FieldInputProps {
	label: string;
	type: string | undefined;
	schema: Record<string, any>;
	value: any;
	values: Record<string, any>;
	ui: Record<string, any>;
	invalid: boolean;
	required: boolean;
	onChange: (value: unknown) => void;
	onNestedChange: (path: string[], value: unknown) => void;
}

function FieldInputDispatcher(props: FieldInputProps) {
	if (props.schema.enum) return <EnumField {...props} />;
	if (props.type === "boolean") return <BooleanField {...props} />;
	if (props.type === "integer" || props.type === "number") {
		return <NumberField {...props} />;
	}
	if (props.type === "array") return <ArrayField {...props} />;
	if (props.type === "object") return <ObjectField {...props} />;
	return <StringField {...props} />;
}

function EnumField(props: FieldInputProps) {
	function changeSelection(event: Event) {
		props.onChange((event.currentTarget as HTMLSelectElement).value);
	}

	return (
		<select
			aria-label={props.label}
			value={String(props.value ?? "")}
			onChange={changeSelection}
			{...controlProps(props)}
		>
			<For each={props.schema.enum}>
				{(value: unknown) => (
					<option value={String(value)}>{enumLabel(props.ui, value)}</option>
				)}
			</For>
		</select>
	);
}

function BooleanField(props: FieldInputProps) {
	function toggleValue(event: MouseEvent) {
		props.onChange((event.currentTarget as HTMLInputElement).checked);
	}

	return (
		<input
			type="checkbox"
			aria-label={props.label}
			checked={Boolean(props.value)}
			onClick={toggleValue}
			{...controlProps(props)}
		/>
	);
}

function NumberField(props: FieldInputProps) {
	function inputValue(event: InputEvent) {
		const raw = (event.currentTarget as HTMLInputElement).value;
		props.onChange(coerceNumberInput(raw));
	}

	return (
		<input
			type="number"
			aria-label={props.label}
			value={props.value ?? ""}
			min={props.schema.minimum}
			max={props.schema.maximum}
			onInput={inputValue}
			{...controlProps(props)}
		/>
	);
}

function ArrayField(props: FieldInputProps) {
	const values = () => arrayValues(props.value);
	const renderedValues = () => [...values(), ""];

	function addValue() {
		props.onChange([...values(), ""]);
	}

	return (
		<div class="mt-1 space-y-2">
			<For each={renderedValues()}>
				{(value, index) => (
					<ArrayValueInput
						label={props.label}
						value={value}
						index={index()}
						values={values()}
						field={props}
					/>
				)}
			</For>
			<button type="button" onClick={addValue}>
				Add {singular(props.label)}
			</button>
		</div>
	);
}

function ArrayValueInput(props: {
	label: string;
	value: unknown;
	index: number;
	values: unknown[];
	field: FieldInputProps;
}) {
	function inputValue(event: InputEvent) {
		const next = [...props.values];
		next[props.index] = (event.currentTarget as HTMLInputElement).value;
		props.field.onChange(next);
	}

	return (
		<input
			type="text"
			aria-label={`${props.label} ${props.index + 1}`}
			value={String(props.value ?? "")}
			onInput={inputValue}
			{...controlProps(props.field)}
		/>
	);
}

function ObjectField(props: FieldInputProps) {
	return (
		<div class="mt-2 space-y-2">
			<For each={Object.keys(props.schema.properties ?? {})}>
				{(key) => (
					<NestedObjectInput
						fieldKey={key}
						schema={props.schema.properties[key]}
						value={props.value?.[key]}
						field={props}
					/>
				)}
			</For>
		</div>
	);
}

function NestedObjectInput(props: {
	fieldKey: string;
	schema: Record<string, any>;
	value: unknown;
	field: FieldInputProps;
}) {
	const label = () => propertyLabel(props.fieldKey, props.schema);
	const inputType = () =>
		baseType(props.schema) === "number" ? "number" : "text";

	function inputValue(event: InputEvent) {
		const raw = (event.currentTarget as HTMLInputElement).value;
		props.field.onNestedChange(
			[props.fieldKey],
			coerceNestedInput(props.schema, raw),
		);
	}

	return (
		<label class="block text-xs font-medium text-text">
			<span>{label()}</span>
			<input
				type={inputType()}
				aria-label={label()}
				value={inputDisplayValue(props.value)}
				onInput={inputValue}
				{...controlProps(props.field)}
			/>
		</label>
	);
}

function StringField(props: FieldInputProps) {
	function inputValue(event: InputEvent) {
		props.onChange((event.currentTarget as HTMLInputElement).value);
	}

	return (
		<input
			type={stringInputType(props.ui)}
			aria-label={props.label}
			value={props.value ?? ""}
			onInput={inputValue}
			{...controlProps(props)}
		/>
	);
}

function UnsupportedField(props: { title: string; value: unknown }) {
	return (
		<div class="rounded border border-border p-3">
			<p class="text-xs font-medium text-text">
				{props.title === "Opaque" ? "Unsupported setting" : "Unsupported field"}
			</p>
			<p class="mt-1 text-xs text-text-dim">{props.title}</p>
			<pre class="mt-2 whitespace-pre-wrap text-xs text-text-dim">
				{inlineJson(props.value)}
			</pre>
		</div>
	);
}

function schemaSections(
	schema: Record<string, any>,
	ui: Record<string, any>,
): SchemaSectionRow[] {
	const allKeys = orderedPropertyKeys(schema, ui);
	if (!Array.isArray(ui.sections) || ui.sections.length === 0) {
		return [{ title: "", keys: allKeys }];
	}
	const grouped = declaredSections(ui.sections, allKeys);
	return appendRemainingSection(grouped, allKeys);
}

function declaredSections(
	sections: any[],
	allKeys: string[],
): SchemaSectionRow[] {
	const rows: SchemaSectionRow[] = [];
	for (const section of sections) {
		rows.push(declaredSection(section, allKeys));
	}
	return rows;
}

function declaredSection(section: any, allKeys: string[]): SchemaSectionRow {
	return {
		title: section.title ?? "",
		keys: orderedSectionKeys(section.order, allKeys),
	};
}

function orderedSectionKeys(order: unknown, allKeys: string[]): string[] {
	if (!Array.isArray(order)) return [];
	return order.filter(
		(key): key is string => typeof key === "string" && allKeys.includes(key),
	);
}

function appendRemainingSection(
	grouped: SchemaSectionRow[],
	allKeys: string[],
): SchemaSectionRow[] {
	const used = usedSectionKeys(grouped);
	const remaining = allKeys.filter((key) => !used.has(key));
	return remaining.length > 0
		? [...grouped, { title: "", keys: remaining }]
		: grouped;
}

function usedSectionKeys(grouped: SchemaSectionRow[]): Set<string> {
	return new Set(grouped.flatMap(sectionKeys));
}

function sectionKeys(section: SchemaSectionRow): string[] {
	return section.keys;
}

function updatePathValue(
	values: Record<string, any>,
	path: string[],
	value: unknown,
): Record<string, any> {
	const next = structuredClone(values);
	let cursor: Record<string, any> = next;
	for (const part of path.slice(0, -1)) {
		cursor[part] = { ...(cursor[part] ?? {}) };
		cursor = cursor[part];
	}
	cursor[path[path.length - 1]] = value;
	return next;
}

function unsetTopLevelValue(
	values: Record<string, any>,
	key: string,
): Record<string, any> {
	const next = { ...values };
	delete next[key];
	return next;
}

function controlProps(props: FieldInputProps) {
	return {
		"aria-invalid": props.invalid ? (true as const) : undefined,
		"aria-required": props.required ? (true as const) : undefined,
		class:
			"mt-1 block w-full rounded border border-border bg-surface-alt px-2 py-1 text-sm text-text",
	};
}

function arrayValues(value: unknown): unknown[] {
	return Array.isArray(value) ? value : [];
}

function coerceNumberInput(raw: string): number | null {
	return raw === "" ? null : Number(raw);
}

function coerceNestedInput(schema: Record<string, any>, raw: string): unknown {
	return baseType(schema) === "number" ? Number(raw) : raw;
}

function enumLabel(ui: Record<string, any>, value: unknown): string {
	return ui.enumLabels?.[String(value)] ?? String(value);
}

function stringInputType(ui: Record<string, any>): "password" | "text" {
	return ui.secret || ui.masked ? "password" : "text";
}

function inputDisplayValue(value: unknown): string | number {
	return typeof value === "number" || typeof value === "string" ? value : "";
}

function singular(label: string): string {
	return label.endsWith("s") ? label.slice(0, -1) : label;
}

function escapePointer(value: string): string {
	return value.replace(/~/g, "~0").replace(/\//g, "~1");
}

function inlineJson(value: unknown): string {
	return (JSON.stringify(value, null, 2) ?? "")
		.replace(/\n\s*/g, " ")
		.replace(/\s+}/g, " }");
}

export default JsonSchemaRenderer;
