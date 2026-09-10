import { For, Show } from "solid-js";
import type {
	ProviderDiagnostic,
	ProviderSettingsMigrate,
	ProviderSettingsRecord,
	ProviderSettingsRecordSummary,
	ProviderSettingsSchema,
	ProviderSettingsTarget,
} from "../../lib/types";
import JsonSchemaRenderer from "./JsonSchemaRenderer";
import { useProviderSettings } from "./useProviderSettings";

export function ProviderSettingsPanel() {
	const settings = useProviderSettings();

	return (
		<div class="space-y-4 rounded-lg border border-border bg-surface p-4">
			<TargetSelector
				targets={settings.targets()}
				onSelect={settings.chooseTarget}
			/>
			<RecordSelector
				target={settings.target()}
				records={settings.records()}
				onSelect={settings.loadRecord}
			/>
			<RecordVersion record={settings.record()} />
			<SettingsForm
				schema={settings.schema()}
				draft={settings.draft()}
				diagnostics={settings.diagnostics()}
				onChange={settings.setDraft}
			/>
			<ConflictResolution
				conflict={settings.conflict()}
				onReapply={settings.saveRecord}
				onKeepRemote={settings.keepRemote}
				onAbandonDraft={settings.keepRemote}
			/>
			<SettingsActions
				onCreate={settings.createRecord}
				onSave={settings.saveRecord}
				onValidate={settings.validateDraft}
				onDelete={settings.deleteRecord}
				onToggleMigration={settings.toggleMigration}
			/>
			<MigrationControl
				open={settings.migrationOpen()}
				result={settings.migration()}
				onDryRun={settings.runMigration}
			/>
		</div>
	);
}

interface TargetSelectorProps {
	targets: ProviderSettingsTarget[];
	onSelect: (target: ProviderSettingsTarget) => void;
}

function TargetSelector(props: TargetSelectorProps) {
	return (
		<div class="flex flex-wrap gap-2">
			<For each={props.targets}>
				{(target) => <TargetButton target={target} onSelect={props.onSelect} />}
			</For>
		</div>
	);
}

function TargetButton(props: {
	target: ProviderSettingsTarget;
	onSelect: (target: ProviderSettingsTarget) => void;
}) {
	function selectTarget() {
		props.onSelect(props.target);
	}

	return (
		<button
			type="button"
			class="rounded border border-border px-3 py-1 text-sm text-text"
			onClick={selectTarget}
		>
			{props.target.displayName}
		</button>
	);
}

interface RecordSelectorProps {
	target: ProviderSettingsTarget | null;
	records: ProviderSettingsRecordSummary[];
	onSelect: (accountName: string, id: string) => void;
}

function RecordSelector(props: RecordSelectorProps) {
	return (
		<Show when={props.records.length > 0}>
			<div class="flex gap-2">
				<For each={props.records}>
					{(record) => (
						<RecordButton
							target={props.target}
							record={record}
							onSelect={props.onSelect}
						/>
					)}
				</For>
			</div>
		</Show>
	);
}

function RecordButton(props: {
	target: ProviderSettingsTarget | null;
	record: ProviderSettingsRecordSummary;
	onSelect: (accountName: string, id: string) => void;
}) {
	function selectRecord() {
		if (!props.target) return;
		props.onSelect(props.target.accountName, props.record.id);
	}

	return (
		<button type="button" onClick={selectRecord}>
			{props.record.displayName}
		</button>
	);
}

function RecordVersion(props: { record: ProviderSettingsRecord | null }) {
	return (
		<Show when={props.record}>
			{(record) => <p class="text-xs text-text-dim">{record().version}</p>}
		</Show>
	);
}

interface SettingsFormProps {
	schema: ProviderSettingsSchema | null;
	draft: Record<string, any>;
	diagnostics: ProviderDiagnostic[];
	onChange: (draft: Record<string, any>) => void;
}

function SettingsForm(props: SettingsFormProps) {
	return (
		<Show when={props.schema}>
			{(schema) => (
				<JsonSchemaRenderer
					schema={schema().schema}
					ui={schema().ui ?? {}}
					values={props.draft}
					diagnostics={props.diagnostics}
					onChange={props.onChange}
				/>
			)}
		</Show>
	);
}

interface ConflictResolutionProps {
	conflict: ProviderSettingsRecord | null;
	onReapply: (versionOverride?: string) => void;
	onKeepRemote: () => void;
	onAbandonDraft: () => void;
}

function ConflictResolution(props: ConflictResolutionProps) {
	return (
		<Show when={props.conflict}>
			{(remote) => (
				<ConflictResolutionBody
					remote={remote()}
					onReapply={props.onReapply}
					onKeepRemote={props.onKeepRemote}
					onAbandonDraft={props.onAbandonDraft}
				/>
			)}
		</Show>
	);
}

function ConflictResolutionBody(props: {
	remote: ProviderSettingsRecord;
	onReapply: (versionOverride?: string) => void;
	onKeepRemote: () => void;
	onAbandonDraft: () => void;
}) {
	function reapplyDraft() {
		props.onReapply(props.remote.version);
	}

	return (
		<div class="space-y-2 rounded border border-warning/40 p-3">
			<p>Conflict</p>
			<p class="text-xs text-text-dim">{props.remote.version}</p>
			<div class="flex gap-2">
				<button type="button" onClick={reapplyDraft}>
					Reapply
				</button>
				<button type="button" onClick={props.onKeepRemote}>
					Keep remote
				</button>
				<button type="button" onClick={props.onAbandonDraft}>
					Abandon draft
				</button>
			</div>
		</div>
	);
}

interface SettingsActionsProps {
	onCreate: () => void;
	onSave: () => void;
	onValidate: () => void;
	onDelete: () => void;
	onToggleMigration: () => void;
}

function SettingsActions(props: SettingsActionsProps) {
	function createRecord() {
		props.onCreate();
	}

	function saveRecord() {
		props.onSave();
	}

	function validateDraft() {
		props.onValidate();
	}

	function deleteRecord() {
		props.onDelete();
	}

	function toggleMigration() {
		props.onToggleMigration();
	}

	return (
		<div class="flex gap-2">
			<button type="button" onClick={createRecord}>
				Create
			</button>
			<button type="button" onClick={saveRecord}>
				Save
			</button>
			<button type="button" onClick={validateDraft}>
				Validate
			</button>
			<button type="button" onClick={deleteRecord}>
				Delete
			</button>
			<button type="button" onClick={toggleMigration}>
				Migration
			</button>
		</div>
	);
}

interface MigrationControlProps {
	open: boolean;
	result: ProviderSettingsMigrate | null;
	onDryRun: () => void;
}

function MigrationControl(props: MigrationControlProps) {
	return (
		<Show when={props.open}>
			<div class="space-y-2">
				<button type="button" onClick={props.onDryRun}>
					Dry run
				</button>
				<MigrationResult result={props.result} />
			</div>
		</Show>
	);
}

function MigrationResult(props: { result: ProviderSettingsMigrate | null }) {
	return (
		<Show when={props.result}>
			{(result) => (
				<div>
					<For each={result().actions ?? []}>
						{(action) => <MigrationActionRow action={action} />}
					</For>
					<For each={result().warnings ?? []}>
						{(warning) => <MigrationWarningRow warning={warning} />}
					</For>
					<For each={result().diagnostics ?? []}>
						{(diagnostic) => <MigrationDiagnosticRow diagnostic={diagnostic} />}
					</For>
				</div>
			)}
		</Show>
	);
}

function MigrationActionRow(props: { action: any }) {
	return <p>{String(props.action.kind ?? props.action)}</p>;
}

function MigrationWarningRow(props: { warning: string }) {
	return <p>{props.warning}</p>;
}

function MigrationDiagnosticRow(props: { diagnostic: ProviderDiagnostic }) {
	return <p>{props.diagnostic.message}</p>;
}

export default ProviderSettingsPanel;
