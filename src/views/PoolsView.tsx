import {
	faArrowRight,
	faPlus,
	faXmark,
} from "@fortawesome/sharp-solid-svg-icons";
import { createQuery, useQueryClient } from "@tanstack/solid-query";
import { createSignal, For, Show } from "solid-js";
import Icon from "../components/Icon";
import InlineSpinner from "../components/InlineSpinner";
import ModelPanel from "../components/ModelPanel";
import type { TagStatus } from "../components/PoolCard";
import PoolCard from "../components/PoolCard";
import PoolSettingsPanel from "../components/PoolSettingsPanel";
import SetupSession from "../components/SetupSession";
import {
	deleteModel,
	getModel,
	listPools,
	reloadModels,
	startCliSetup,
	updatePool,
} from "../lib/tauri";
import type { ProviderConfig } from "../lib/types";

// TODO(design): This view should become "Configure" sub-screen, not the home page.
// The home page should be a task-centric HomeView with Run/Status/Configure cards.
// See /tmp/oulipoly-e2e/dashboard-rethink.md for the concept.

interface PoolsViewProps {
	onRunSetup: () => void;
}

export default function PoolsView(props: PoolsViewProps) {
	const queryClient = useQueryClient();
	const [validatingPool, setValidatingPool] = createSignal<number | null>(null);
	const [validatingCommand, setValidatingCommand] = createSignal<string | null>(
		null,
	);
	const [tagStatuses, setTagStatuses] = createSignal<Record<string, TagStatus>>(
		{},
	);
	const [selectedTag, setSelectedTag] = createSignal<string | null>(null);

	// Add-pool state
	const [addingPool, setAddingPool] = createSignal(false);
	const [newCliName, setNewCliName] = createSignal("");
	const [setupRunning, setSetupRunning] = createSignal(false);

	// Model panel state
	const [modelPanel, setModelPanel] = createSignal<{
		mode: "add" | "edit";
		poolCommands: string[];
		modelNames: string[];
		editModelName?: string;
		group?: string;
	} | null>(null);

	// Pool settings panel state
	const [poolSettings, setPoolSettings] = createSignal<{
		poolCommands: string[];
		commonFlags: string[];
	} | null>(null);

	const poolsQuery = createQuery(() => ({
		queryKey: ["pools"],
		queryFn: listPools,
	}));

	function invalidate() {
		queryClient.invalidateQueries({ queryKey: ["pools"] });
		queryClient.invalidateQueries({ queryKey: ["models"] });
	}

	function setStatus(cmd: string, status: TagStatus) {
		setTagStatuses((prev) => ({ ...prev, [cmd]: status }));
	}

	function clearStatus(cmd: string) {
		setTagStatuses((prev) => {
			const next = { ...prev };
			delete next[cmd];
			return next;
		});
	}

	function handleAddCommand(
		index: number,
		poolCommands: string[],
		cmd: string,
	) {
		setValidatingPool(index);
		setValidatingCommand(cmd);
		setStatus(cmd, "validating");
		currentPoolCommands = poolCommands;
	}

	let currentPoolCommands: string[] = [];

	async function handleSetupComplete() {
		const cmd = validatingCommand();
		const poolCmds = currentPoolCommands;
		setValidatingPool(null);
		setValidatingCommand(null);

		try {
			await reloadModels();
			if (cmd && poolCmds.length > 0) {
				await updatePool(poolCmds, [...poolCmds, cmd]);
			}
			if (cmd) clearStatus(cmd);
		} catch (err) {
			console.error("Failed to update pool after setup:", err);
			if (cmd) {
				setStatus(cmd, "error");
				setTimeout(() => clearStatus(cmd), 2000);
			}
		}

		invalidate();
	}

	function handleSetupCancel() {
		const cmd = validatingCommand();
		setValidatingPool(null);
		setValidatingCommand(null);
		if (cmd) clearStatus(cmd);
	}

	async function handleRemoveCommand(
		poolCommands: string[],
		command: string,
		modelNames: string[],
	) {
		const remaining = poolCommands.filter((c) => c !== command);

		if (remaining.length === 0) {
			if (
				!confirm("Removing the only command deletes all models in this pool.")
			)
				return;

			try {
				for (const name of modelNames) {
					await deleteModel(name);
				}
			} catch (err) {
				console.error("Failed to delete models:", err);
			}
		} else {
			try {
				await updatePool(poolCommands, remaining);
			} catch (err) {
				console.error("Failed to update pool:", err);
			}
		}

		invalidate();
	}

	async function handleEditCommand(
		poolCommands: string[],
		oldCommand: string,
		newCommand: string,
	) {
		try {
			await updatePool(
				poolCommands,
				poolCommands.map((c) => (c === oldCommand ? newCommand : c)),
			);
		} catch (err) {
			console.error("Failed to edit command:", err);
		}

		invalidate();
	}

	async function handleDeleteModel(name: string, poolModelNames: string[]) {
		if (poolModelNames.length === 1) {
			if (
				!confirm(
					"This is the last model in the pool. Deleting it removes the entire pool.",
				)
			)
				return;
		}
		try {
			await deleteModel(name);
		} catch (err) {
			console.error("Failed to delete model:", err);
		}
		await reloadModels();
		invalidate();
	}

	function handleAddModelPanel(poolCommands: string[], modelNames: string[]) {
		setModelPanel({
			mode: "add",
			poolCommands,
			modelNames,
		});
	}

	function handleAddModelToGroup(
		poolCommands: string[],
		modelNames: string[],
		group: string,
	) {
		setModelPanel({
			mode: "add",
			poolCommands,
			modelNames,
			group,
		});
	}

	function handleEditModelPanel(
		poolCommands: string[],
		modelNames: string[],
		modelName: string,
	) {
		setModelPanel({
			mode: "edit",
			poolCommands,
			modelNames,
			editModelName: modelName,
		});
	}

	async function handleModelPanelSave() {
		setModelPanel(null);
		await reloadModels();
		invalidate();
	}

	async function handlePoolSettings(
		poolCommands: string[],
		modelNames: string[],
	) {
		const flags = new Set<string>();

		for (const name of modelNames) {
			try {
				const model = await getModel(name);
				for (const provider of model.providers) {
					for (const arg of provider.args) {
						if (arg.startsWith("--dangerously-") || arg === "--yolo") {
							flags.add(arg);
						}
					}
				}
			} catch {
				// skip
			}
		}

		setPoolSettings({
			poolCommands,
			commonFlags: [...flags].sort(),
		});
	}

	async function handleToggleFlag(
		_poolCommands: string[],
		modelNames: string[],
		flag: string,
		enabled: boolean,
	) {
		for (const name of modelNames) {
			try {
				const model = await getModel(name);
				let changed = false;

				const updatedProviders: ProviderConfig[] = model.providers.map(
					(provider) => {
						const hasFlag = provider.args.includes(flag);
						if (enabled && !hasFlag) {
							changed = true;
							return { ...provider, args: [...provider.args, flag] };
						}
						if (!enabled && hasFlag) {
							changed = true;
							return {
								...provider,
								args: provider.args.filter((a) => a !== flag),
							};
						}
						return provider;
					},
				);

				if (changed) {
					const { saveModel } = await import("../lib/tauri");
					await saveModel({
						...model,
						providers: updatedProviders,
					});
				}
			} catch (err) {
				console.error(`Failed to toggle flag for model ${name}:`, err);
			}
		}

		await reloadModels();
		invalidate();
	}

	async function handleAddPoolSetupComplete() {
		setSetupRunning(false);
		setAddingPool(false);
		setNewCliName("");
		await reloadModels();
		invalidate();
	}

	function handleAddPoolSetupCancel() {
		setSetupRunning(false);
	}

	function handleContainerClick(e: MouseEvent) {
		const target = e.target as HTMLElement;
		if (target.dataset.role === "pools-container") {
			setSelectedTag(null);
		}
	}

	return (
		// biome-ignore lint/a11y/noStaticElementInteractions: click deselects tags
		// biome-ignore lint/a11y/useKeyWithClickEvents: click deselects tags
		<div
			data-role="pools-container"
			class="mx-auto max-w-4xl p-6"
			onClick={handleContainerClick}
		>
			{/* Header */}
			<div class="mb-6 flex items-center justify-between">
				<h2 class="text-xl font-semibold text-text">Provider Pools</h2>
				<button
					type="button"
					class={`flex h-8 w-8 items-center justify-center rounded-lg text-sm font-bold transition-colors ${
						addingPool()
							? "bg-border text-text-dim hover:bg-surface-alt"
							: "bg-accent text-black hover:bg-accent-hover"
					}`}
					onClick={() => {
						if (addingPool()) {
							setAddingPool(false);
							setNewCliName("");
							setSetupRunning(false);
						} else {
							setAddingPool(true);
						}
					}}
					title={addingPool() ? "Cancel" : "Add provider pool"}
				>
					<Show when={addingPool()} fallback={<Icon icon={faPlus} size={16} />}>
						<Icon icon={faXmark} size={16} />
					</Show>
				</button>
			</div>

			{/* Add-pool inline row */}
			<Show when={addingPool()}>
				<div class="mb-4 rounded-lg border border-border bg-surface p-4">
					<Show when={!setupRunning()}>
						<div class="flex items-center gap-2">
							<input
								type="text"
								class="flex-1 rounded border border-border bg-surface-alt px-3 py-2 text-sm font-mono text-text outline-none placeholder:text-text-faint focus:border-accent"
								placeholder="Enter new pool name (e.g., openai)..."
								value={newCliName()}
								onInput={(e) => setNewCliName(e.currentTarget.value)}
								onKeyDown={(e) => {
									if (e.key === "Enter" && newCliName().trim()) {
										setSetupRunning(true);
									} else if (e.key === "Escape") {
										setAddingPool(false);
										setNewCliName("");
									}
								}}
								ref={(el) => requestAnimationFrame(() => el.focus())}
							/>
							<button
								type="button"
								class="flex h-9 w-9 items-center justify-center rounded bg-accent text-black transition-colors hover:bg-accent-hover disabled:opacity-50"
								disabled={!newCliName().trim()}
								onClick={() => setSetupRunning(true)}
								title="Submit"
							>
								<Icon icon={faArrowRight} size={16} />
							</button>
						</div>
					</Show>

					<Show when={setupRunning()}>
						{(() => {
							const cli = newCliName().trim();
							return cli ? (
								<SetupSession
									startFn={(ch) => startCliSetup(cli, ch)}
									onComplete={handleAddPoolSetupComplete}
									onCancel={handleAddPoolSetupCancel}
								/>
							) : null;
						})()}
					</Show>
				</div>
			</Show>

			{/* Loading / Error states */}
			<Show when={poolsQuery.isLoading}>
				<div class="flex items-center justify-center gap-3 py-12">
					<InlineSpinner size={20} />
					<span class="text-text-dim">Loading pools...</span>
				</div>
			</Show>
			<Show when={poolsQuery.isError}>
				<p class="py-12 text-center text-error">
					Failed to load pools: {String(poolsQuery.error)}
				</p>
			</Show>

			{/* Pool list */}
			<Show
				when={(poolsQuery.data?.length ?? 0) > 0}
				fallback={
					<Show when={poolsQuery.isSuccess}>
						<div class="py-12 text-center">
							<p class="mb-4 text-text-dim">No pools yet.</p>
							<button
								type="button"
								class="rounded-lg bg-accent px-5 py-2 text-sm font-medium text-black transition-colors hover:bg-accent-hover"
								onClick={() => props.onRunSetup()}
							>
								Run Setup
							</button>
						</div>
					</Show>
				}
			>
				<div class="rounded-lg border border-border bg-surface">
					<For each={poolsQuery.data}>
						{(pool, index) => (
							<div
								class="animate-fade-up"
								style={{ "animation-delay": `${index() * 60}ms` }}
							>
								<PoolCard
									pool={pool}
									onAddCommand={(cmd) =>
										handleAddCommand(index(), pool.commands, cmd)
									}
									onRemoveCommand={(cmd) =>
										handleRemoveCommand(pool.commands, cmd, pool.model_names)
									}
									onEditCommand={(oldCmd, newCmd) =>
										handleEditCommand(pool.commands, oldCmd, newCmd)
									}
									onPoolSettings={() =>
										handlePoolSettings(pool.commands, pool.model_names)
									}
									onAddModel={() =>
										handleAddModelPanel(pool.commands, pool.model_names)
									}
									onAddModelToGroup={(group) =>
										handleAddModelToGroup(pool.commands, pool.model_names, group)
									}
									onEditModel={(name) =>
										handleEditModelPanel(pool.commands, pool.model_names, name)
									}
									onDeleteModel={(name) =>
										handleDeleteModel(name, pool.model_names)
									}
									tagStatuses={tagStatuses()}
									selectedTag={selectedTag()}
									onSelectTag={setSelectedTag}
								>
									<Show when={validatingPool() === index()}>
										{(() => {
											const cli = validatingCommand();
											return cli ? (
												<SetupSession
													startFn={(ch) => startCliSetup(cli, ch)}
													onComplete={handleSetupComplete}
													onCancel={handleSetupCancel}
												/>
											) : null;
										})()}
									</Show>
								</PoolCard>
							</div>
						)}
					</For>
				</div>
			</Show>

			{/* Pool settings panel */}
			<Show when={poolSettings()}>
				{(settings) => (
					<PoolSettingsPanel
						poolCommands={settings().poolCommands}
						commonFlags={settings().commonFlags}
						onToggleFlag={(flag, enabled) => {
							const pools = poolsQuery.data ?? [];
							const pool = pools.find(
								(p) =>
									JSON.stringify(p.commands) ===
									JSON.stringify(settings().poolCommands),
							);
							if (pool) {
								handleToggleFlag(
									pool.commands,
									pool.model_names,
									flag,
									enabled,
								);
							}
						}}
						onClose={() => setPoolSettings(null)}
					/>
				)}
			</Show>

			{/* Model panel */}
			<Show when={modelPanel()}>
				{(panel) => (
					<ModelPanel
						mode={panel().mode}
						poolCommands={panel().poolCommands}
						modelNames={panel().modelNames}
						editModelName={panel().editModelName}
						group={panel().group}
						onSave={handleModelPanelSave}
						onClose={() => setModelPanel(null)}
					/>
				)}
			</Show>
		</div>
	);
}
