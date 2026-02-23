import { Popover } from "@ark-ui/solid";
import {
	faChevronDown,
	faGear,
	faPlus,
	faXmark,
} from "@fortawesome/sharp-solid-svg-icons";
import type { JSX } from "solid-js";
import { createSignal, For, Show } from "solid-js";
import { groupModels } from "../lib/grouping";
import type { ModelGroup, PoolSummary } from "../lib/types";
import Icon from "./Icon";
import InlineSpinner from "./InlineSpinner";

// TODO(design): All pool rows look identical -- no visual distinction between tools.
// Could use brand colors per-tool or subtle tool-specific accents.
// Lower priority now that PoolsView moves to Configure sub-screen.

export type TagStatus = "idle" | "validating" | "error";

interface PoolCardProps {
	pool: PoolSummary;
	onAddCommand: (command: string) => void;
	onRemoveCommand: (command: string) => void;
	onEditCommand: (oldCommand: string, newCommand: string) => void;
	onPoolSettings: () => void;
	onAddModel: (poolCommands: string[]) => void;
	onAddModelToGroup: (group: string) => void;
	onEditModel: (modelName: string) => void;
	onDeleteModel: (modelName: string) => void;
	tagStatuses: Record<string, TagStatus>;
	selectedTag: string | null;
	onSelectTag: (cmd: string | null) => void;
	children?: JSX.Element;
}

export default function PoolCard(props: PoolCardProps) {
	const [editingTag, setEditingTag] = createSignal<string | null>(null);
	const [editValue, setEditValue] = createSignal("");

	const poolName = () => props.pool.commands[0] ?? "pool";
	const modelGroups = () => groupModels(props.pool.model_names);

	function handleTagClick(cmd: string, e: MouseEvent) {
		e.stopPropagation();
		props.onSelectTag(props.selectedTag === cmd ? null : cmd);
	}

	function handleTagDblClick(cmd: string, e: MouseEvent) {
		e.stopPropagation();
		setEditingTag(cmd);
		setEditValue(cmd);
	}

	function handleEditKeyDown(e: KeyboardEvent, oldCmd: string) {
		if (e.key === "Enter") {
			const newVal = editValue().trim();
			if (newVal && newVal !== oldCmd) {
				props.onEditCommand(oldCmd, newVal);
			}
			setEditingTag(null);
			setEditValue("");
		} else if (e.key === "Escape") {
			setEditingTag(null);
			setEditValue("");
		}
	}

	function handleRowKeyDown(e: KeyboardEvent) {
		const sel = props.selectedTag;
		if (sel && (e.key === "Delete" || e.key === "Backspace")) {
			e.preventDefault();
			props.onRemoveCommand(sel);
			props.onSelectTag(null);
		}
	}

	function tagStatusClass(cmd: string): string {
		const status = props.tagStatuses[cmd];
		if (status === "error") return "border-error animate-pulse";
		return "border-transparent";
	}

	function handleDeleteFacetModel(group: ModelGroup, modelName: string) {
		if (group.modelNames.length === 1) {
			if (
				!confirm(
					`This is the last variant in "${group.group}". Deleting it removes the model entirely.`,
				)
			)
				return;
		}
		props.onDeleteModel(modelName);
	}

	return (
		<div>
			{/* Pool row */}
			{/* biome-ignore lint/a11y/noStaticElementInteractions: row handles DEL key for selected tags */}
			<div
				class="flex items-center gap-4 border-b border-border px-4 py-3"
				onKeyDown={handleRowKeyDown}
				tabIndex={-1}
			>
				{/* Pool name */}
				<span class="min-w-[80px] text-sm font-bold text-text">
					{poolName()}
				</span>

				{/* Command chips */}
				<div class="flex flex-1 flex-wrap items-center gap-1.5">
					<For each={props.pool.commands}>
						{(cmd) => (
							<Show
								when={editingTag() === cmd}
								fallback={
									<span
										role="option"
										tabIndex={0}
										class={`inline-flex cursor-pointer select-none items-center gap-1 rounded-full border bg-surface-alt px-2.5 py-0.5 text-xs font-mono text-accent transition-colors hover:bg-border ${
											props.selectedTag === cmd ? "ring-1 ring-accent" : ""
										} ${tagStatusClass(cmd)}`}
										onClick={(e) => handleTagClick(cmd, e)}
										onKeyDown={(e) => {
											if (e.key === "Enter")
												handleTagDblClick(cmd, e as unknown as MouseEvent);
										}}
										onDblClick={(e) => handleTagDblClick(cmd, e)}
									>
										{cmd}
										<Show when={props.tagStatuses[cmd] === "validating"}>
											<InlineSpinner size={12} />
										</Show>
									</span>
								}
							>
								<input
									type="text"
									class="w-20 rounded-full border border-accent bg-surface px-2.5 py-0.5 text-xs font-mono text-accent outline-none"
									value={editValue()}
									onInput={(e) => setEditValue(e.currentTarget.value)}
									onKeyDown={(e) => handleEditKeyDown(e, cmd)}
									onBlur={() => {
										setEditingTag(null);
										setEditValue("");
									}}
									ref={(el) => {
										requestAnimationFrame(() => el.focus());
									}}
								/>
							</Show>
						)}
					</For>
				</div>

				{/* Settings gear */}
				<button
					type="button"
					class="flex h-7 w-7 items-center justify-center text-text-dim transition-colors hover:text-text"
					onClick={() => props.onPoolSettings()}
					title="Pool settings"
				>
					<Icon icon={faGear} size={16} />
				</button>

				{/* Models dropdown via Ark UI Popover */}
				<Popover.Root>
					<Popover.Trigger class="flex items-center gap-1.5 rounded-full border border-border bg-surface-alt px-3 py-1 text-xs text-text-dim transition-colors hover:border-border-focus hover:text-text">
						<span>
							{props.pool.model_count} Model
							{props.pool.model_count !== 1 ? "s" : ""}
						</span>
						<Icon icon={faChevronDown} size={12} />
					</Popover.Trigger>
					<Popover.Positioner>
						<Popover.Content class="z-40 min-w-[320px] rounded-lg border border-border bg-surface py-1 shadow-lg">
							<div class="flex items-center justify-between px-3 py-1.5">
								<span class="text-xs font-medium text-text-dim">
									Models ({props.pool.model_count})
								</span>
								<button
									type="button"
									class="flex items-center gap-1 rounded px-1.5 py-0.5 text-xs text-accent transition-colors hover:bg-surface-alt"
									onClick={(e) => {
										e.stopPropagation();
										props.onAddModel(props.pool.commands);
									}}
									title="Add standalone model"
								>
									<Icon icon={faPlus} size={10} />
									<span>New</span>
								</button>
							</div>

							{/* Grouped model list */}
							<For each={modelGroups()}>
								{(group) => (
									<div class="px-3 py-1.5">
										<Show
											when={!group.standalone}
											fallback={
												/* Standalone model — single clickable row */
												<div class="group flex items-center justify-between">
													<button
														type="button"
														class="flex-1 text-left text-xs font-mono text-accent hover:underline"
														onClick={(e) => {
															e.stopPropagation();
															props.onEditModel(group.modelNames[0]);
														}}
													>
														{group.group}
													</button>
													<button
														type="button"
														class="ml-2 text-xs text-text-faint opacity-0 transition-opacity hover:text-error group-hover:opacity-100"
														onClick={(e) => {
															e.stopPropagation();
															props.onDeleteModel(group.modelNames[0]);
														}}
														title="Delete model"
													>
														<Icon icon={faXmark} size={12} />
													</button>
												</div>
											}
										>
											{/* Group with facets */}
											<div class="flex flex-wrap items-center gap-1.5">
												<span class="text-xs font-mono text-text-dim">
													{group.group}
												</span>
												<For each={group.facets}>
													{(facet, fi) => (
														<span class="group/chip inline-flex items-center gap-0.5 rounded-full border border-border bg-surface-alt px-2 py-0.5 text-xs font-mono text-accent transition-colors hover:border-accent">
															<button
																type="button"
																class="hover:underline"
																onClick={(e) => {
																	e.stopPropagation();
																	props.onEditModel(group.modelNames[fi()]);
																}}
															>
																{facet}
															</button>
															<button
																type="button"
																class="ml-0.5 text-text-faint opacity-0 transition-opacity hover:text-error group-hover/chip:opacity-100"
																onClick={(e) => {
																	e.stopPropagation();
																	handleDeleteFacetModel(
																		group,
																		group.modelNames[fi()],
																	);
																}}
																title={`Delete ${group.group}~${facet}`}
															>
																<Icon icon={faXmark} size={10} />
															</button>
														</span>
													)}
												</For>
												<button
													type="button"
													class="inline-flex h-5 w-5 items-center justify-center rounded-full border border-dashed border-border text-text-faint transition-colors hover:border-accent hover:text-accent"
													onClick={(e) => {
														e.stopPropagation();
														props.onAddModelToGroup(group.group);
													}}
													title={`Add variant to ${group.group}`}
												>
													<Icon icon={faPlus} size={8} />
												</button>
											</div>
										</Show>
									</div>
								)}
							</For>
						</Popover.Content>
					</Popover.Positioner>
				</Popover.Root>
			</div>

			{/* Optional children slot (SetupSession) */}
			<Show when={props.children}>
				<div class="border-b border-border px-4 py-3">{props.children}</div>
			</Show>
		</div>
	);
}
