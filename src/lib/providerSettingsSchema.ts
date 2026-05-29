import type { ProviderDiagnostic } from "./types";

export type SettingsValues = Record<string, unknown>;

export interface MappedProviderDiagnostics {
	fields: Record<string, ProviderDiagnostic[]>;
	summary: ProviderDiagnostic[];
}

type JsonSchema = Record<string, any>;
type UiHints = Record<string, any>;

export function buildInitialSettingsValues(
	schema: JsonSchema,
	recordValues?: SettingsValues,
): SettingsValues {
	if (recordValues) return { ...recordValues };
	return Object.fromEntries(
		Object.entries(schema.properties ?? {})
			.filter(hasDefaultProperty)
			.map(defaultPropertyValue),
	);
}

export function mapProviderDiagnostics(
	schema: JsonSchema,
	ui: UiHints | undefined,
	diagnostics: ProviderDiagnostic[],
): MappedProviderDiagnostics {
	const uiHints = ui ?? {};
	return {
		fields: fieldDiagnostics(schema, uiHints, diagnostics),
		summary: summaryDiagnostics(schema, uiHints, diagnostics),
	};
}

export function orderedPropertyKeys(
	schema: JsonSchema,
	ui?: UiHints,
): string[] {
	const properties = Object.keys(schema.properties ?? {});
	const sections = Array.isArray(ui?.sections) ? ui.sections : [];
	const ordered = [
		...sections.flatMap(sectionOrder),
		...(Array.isArray(ui?.order) ? ui.order : []),
		...properties,
	].filter((key): key is string => typeof key === "string");
	return [...new Set(ordered)].filter((key) => properties.includes(key));
}

export function propertyLabel(key: string, schema: JsonSchema): string {
	return typeof schema.title === "string" ? schema.title : titleFromKey(key);
}

export function isRequired(schema: JsonSchema, key: string): boolean {
	return Array.isArray(schema.required) && schema.required.includes(key);
}

export function isNullable(schema: JsonSchema): boolean {
	return Array.isArray(schema.type) && schema.type.includes("null");
}

export function baseType(schema: JsonSchema): string | undefined {
	return Array.isArray(schema.type)
		? schema.type.find((value: string) => value !== "null")
		: schema.type;
}

export function isUnsupportedSchema(schema: JsonSchema): boolean {
	if (schema.oneOf || schema.anyOf || schema.allOf || schema.if) return true;
	if (schema.type === "object" && schema.additionalProperties === true) {
		return true;
	}
	if (schema.type === "array" && Array.isArray(schema.items)) return true;
	return false;
}

function isRenderablePointer(
	schema: JsonSchema,
	ui: UiHints,
	pointer: string,
): boolean {
	const parts = pointerParts(pointer);
	if (parts.length === 0) return false;
	const top = parts[0];
	const fieldUi = ui.fields?.[top] ?? {};
	if (fieldUi.hiddenDiagnostics) return false;
	const property = schema.properties?.[top];
	if (!isPlainObject(property) || isUnsupportedSchema(property)) return false;
	if (parts.length === 1) return true;
	if (property.type === "array" && /^\d+$/.test(parts[1])) return true;
	if (property.type === "object" && isPlainObject(property.properties)) {
		const nested = property.properties[parts[1]];
		return isPlainObject(nested) && !isUnsupportedSchema(nested);
	}
	return false;
}

function pointerParts(pointer: string): string[] {
	return pointer
		.split("/")
		.slice(1)
		.map((part) => part.replace(/~1/g, "/").replace(/~0/g, "~"))
		.filter(Boolean);
}

function sectionOrder(section: any): unknown[] {
	return Array.isArray(section.order) ? section.order : [];
}

function hasDefaultProperty(
	entry: [string, unknown],
): entry is [string, JsonSchema & { default: unknown }] {
	const [, property] = entry;
	return isPlainObject(property) && Object.hasOwn(property, "default");
}

function defaultPropertyValue([key, property]: [
	string,
	JsonSchema & { default: unknown },
]): [string, unknown] {
	return [key, cloneJson(property.default)];
}

function fieldDiagnostics(
	schema: JsonSchema,
	ui: UiHints,
	diagnostics: ProviderDiagnostic[],
): Record<string, ProviderDiagnostic[]> {
	return diagnostics
		.filter((diagnostic) => isFieldDiagnostic(schema, ui, diagnostic))
		.reduce(addFieldDiagnostic, {});
}

function summaryDiagnostics(
	schema: JsonSchema,
	ui: UiHints,
	diagnostics: ProviderDiagnostic[],
): ProviderDiagnostic[] {
	return diagnostics.filter(
		(diagnostic) => !isFieldDiagnostic(schema, ui, diagnostic),
	);
}

function isFieldDiagnostic(
	schema: JsonSchema,
	ui: UiHints,
	diagnostic: ProviderDiagnostic,
): boolean {
	return isRenderablePointer(schema, ui, diagnostic.path ?? "");
}

function addFieldDiagnostic(
	fields: Record<string, ProviderDiagnostic[]>,
	diagnostic: ProviderDiagnostic,
): Record<string, ProviderDiagnostic[]> {
	const path = diagnostic.path ?? "";
	return { ...fields, [path]: [...(fields[path] ?? []), diagnostic] };
}

function titleFromKey(key: string): string {
	return key
		.replace(/([a-z0-9])([A-Z])/g, "$1 $2")
		.replace(/[-_]/g, " ")
		.replace(/\b\w/g, (char) => char.toUpperCase());
}

function cloneJson(value: unknown): unknown {
	return value == null ? value : JSON.parse(JSON.stringify(value));
}

function isPlainObject(value: unknown): value is Record<string, any> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}
