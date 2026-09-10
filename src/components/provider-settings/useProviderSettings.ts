import { createSignal, onMount } from "solid-js";
import { buildInitialSettingsValues } from "../../lib/providerSettingsSchema";
import {
	createProviderSettings,
	deleteProviderSettings,
	getProviderSettings,
	getProviderSettingsSchema,
	listProviderSettings,
	listProviderSettingsTargets,
	migrateProviderSettings,
	updateProviderSettings,
	validateProviderSettings,
} from "../../lib/tauri";
import type {
	ProviderDiagnostic,
	ProviderSettingsMigrate,
	ProviderSettingsRecord,
	ProviderSettingsRecordSummary,
	ProviderSettingsSchema,
	ProviderSettingsTarget,
} from "../../lib/types";

export function useProviderSettings() {
	const [targets, setTargets] = createSignal<ProviderSettingsTarget[]>([]);
	const [target, setTarget] = createSignal<ProviderSettingsTarget | null>(null);
	const [schema, setSchema] = createSignal<ProviderSettingsSchema | null>(null);
	const [records, setRecords] = createSignal<ProviderSettingsRecordSummary[]>(
		[],
	);
	const [record, setRecord] = createSignal<ProviderSettingsRecord | null>(null);
	const [draft, setDraft] = createSignal<Record<string, any>>({});
	const [diagnostics, setDiagnostics] = createSignal<ProviderDiagnostic[]>([]);
	const [conflict, setConflict] = createSignal<ProviderSettingsRecord | null>(
		null,
	);
	const [migrationOpen, setMigrationOpen] = createSignal(false);
	const [migration, setMigration] =
		createSignal<ProviderSettingsMigrate | null>(null);

	onMount(loadTargets);

	async function loadTargets() {
		setTargets(await listProviderSettingsTargets());
	}

	async function chooseTarget(nextTarget: ProviderSettingsTarget) {
		resetTargetState(nextTarget);
		const loadedSchema = await loadSchema(nextTarget);
		const listed = await listProviderSettings(nextTarget.accountName);
		setRecords(listed.records);
		await chooseInitialRecord(nextTarget, loadedSchema, listed.records);
	}

	async function loadRecord(accountName: string, id: string) {
		const loaded = await getProviderSettings(accountName, id);
		replaceRecord(loaded.record);
	}

	async function createRecord() {
		const current = target();
		if (!current) return;
		const result = await createProviderSettings(
			current.accountName,
			"Record",
			draft(),
		);
		acceptWriteResult(result.record, result.diagnostics);
	}

	async function saveRecord(versionOverride?: string) {
		const current = target();
		const currentRecord = await ensureRecord();
		if (!current || !currentRecord) return;
		const localDraft = { ...draft() };
		try {
			const result = await updateProviderSettings(
				current.accountName,
				currentRecord.id,
				versionOverride ?? currentRecord.version,
				localDraft,
			);
			acceptWriteResult(result.record, result.diagnostics);
			setConflict(null);
		} catch (error) {
			await resolveSaveError(
				error,
				current.accountName,
				currentRecord.id,
				localDraft,
			);
		}
	}

	async function deleteRecord() {
		const current = target();
		const currentRecord = record();
		if (!current || !currentRecord) return;
		await deleteProviderSettings(
			current.accountName,
			currentRecord.id,
			currentRecord.version,
		);
		await reloadAfterDelete(current);
	}

	async function validateDraft() {
		const current = target();
		if (!current) return;
		const result = await validateProviderSettings(current.accountName, draft());
		setDiagnostics(result.diagnostics);
	}

	async function runMigration() {
		const current = target();
		if (!current) return;
		setMigration(
			await migrateProviderSettings(current.accountName, true, null),
		);
	}

	function toggleMigration() {
		setMigrationOpen(!migrationOpen());
	}

	function keepRemote() {
		const remote = conflict();
		if (!remote) return;
		replaceRecord(remote);
		setConflict(null);
	}

	return {
		targets,
		target,
		schema,
		records,
		record,
		draft,
		setDraft,
		diagnostics,
		conflict,
		migrationOpen,
		migration,
		chooseTarget,
		loadRecord,
		createRecord,
		saveRecord,
		deleteRecord,
		validateDraft,
		runMigration,
		toggleMigration,
		keepRemote,
	};

	function resetTargetState(nextTarget: ProviderSettingsTarget) {
		setTarget(nextTarget);
		setSchema(null);
		setRecord(null);
		setRecords([]);
		setDraft({});
		setConflict(null);
		setDiagnostics([]);
		setMigration(null);
	}

	async function loadSchema(nextTarget: ProviderSettingsTarget) {
		if (!nextTarget.schemaId) return null;
		const loaded = await getProviderSettingsSchema(
			nextTarget.accountName,
			nextTarget.schemaId,
		);
		setSchema(loaded);
		return loaded;
	}

	async function chooseInitialRecord(
		nextTarget: ProviderSettingsTarget,
		loadedSchema: ProviderSettingsSchema | null,
		summaries: ProviderSettingsRecordSummary[],
	) {
		const first = summaries[0];
		if (first) {
			await loadInitialRecord(nextTarget.accountName, first.id);
			return;
		}
		setDraft(defaultDraft(loadedSchema));
	}

	async function loadInitialRecord(accountName: string, id: string) {
		const loaded = await getProviderSettings(accountName, id);
		setRecord(loaded.record);
		if (isDraftEmpty()) setDraft({ ...loaded.record.values });
	}

	async function ensureRecord(): Promise<ProviderSettingsRecord | null> {
		const currentRecord = record();
		if (currentRecord) return currentRecord;
		const current = target();
		if (!current) return null;
		const first = await firstAvailableRecord(current.accountName);
		if (!first) return null;
		const loaded = await getProviderSettings(current.accountName, first.id);
		setRecord(loaded.record);
		return loaded.record;
	}

	async function firstAvailableRecord(accountName: string) {
		const known = records()[0];
		if (known) return known;
		const listed = await listProviderSettings(accountName);
		setRecords(listed.records);
		return listed.records[0] ?? null;
	}

	async function reloadAfterDelete(current: ProviderSettingsTarget) {
		const listed = await listProviderSettings(current.accountName);
		setRecords(listed.records);
		const first = listed.records[0];
		if (first) {
			await loadRecord(current.accountName, first.id);
			return;
		}
		setRecord(null);
		setDraft(defaultDraft(schema()));
	}

	function acceptWriteResult(
		nextRecord: ProviderSettingsRecord,
		nextDiagnostics: ProviderDiagnostic[],
	) {
		replaceRecord(nextRecord);
		setDiagnostics(nextDiagnostics);
	}

	function replaceRecord(nextRecord: ProviderSettingsRecord) {
		setRecord(nextRecord);
		setDraft({ ...nextRecord.values });
	}

	function isDraftEmpty() {
		return Object.keys(draft()).length === 0;
	}

	function defaultDraft(currentSchema: ProviderSettingsSchema | null) {
		return currentSchema
			? buildInitialSettingsValues(currentSchema.schema, undefined)
			: {};
	}

	async function resolveSaveError(
		error: unknown,
		accountName: string,
		recordId: string,
		localDraft: Record<string, any>,
	) {
		if (!isConflictError(error)) throw error;
		const remote = await getProviderSettings(accountName, recordId);
		setDraft(localDraft);
		setConflict(remote.record);
		setDiagnostics(error.diagnostics ?? []);
	}
}

function isConflictError(
	error: unknown,
): error is { category: string; diagnostics?: ProviderDiagnostic[] } {
	return typeof error === "object" && error !== null && "category" in error
		? (error as { category?: unknown }).category === "conflict"
		: false;
}
