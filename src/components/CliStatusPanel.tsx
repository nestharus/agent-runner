import { For } from "solid-js";
import type { CliInfo } from "../lib/types";

interface CliStatusPanelProps {
	clis: CliInfo[];
	onRefresh: () => void;
}

export default function CliStatusPanel(props: CliStatusPanelProps) {
	return (
		<div class="mb-6 rounded-lg bg-[#16213e] p-4">
			<div class="mb-3 flex items-center justify-between">
				<h3 class="text-sm font-medium text-text">Detected CLIs</h3>
				<button
					type="button"
					class="rounded px-3 py-1 text-xs text-accent transition-colors hover:bg-[#0f3460]"
					onClick={() => props.onRefresh()}
				>
					Refresh
				</button>
			</div>
			<div class="flex flex-col gap-2">
				<For each={props.clis}>
					{(cli) => (
						<div class="flex items-center gap-3 text-sm">
							<span class="w-24 font-medium text-text">{cli.name}</span>
							<span
								class={`rounded px-2 py-0.5 text-xs ${
									cli.installed
										? "bg-success/15 text-success"
										: "bg-error/15 text-error"
								}`}
							>
								{cli.installed ? "Installed" : "Not found"}
							</span>
							{cli.version && (
								<span class="text-xs text-text-dim">{cli.version}</span>
							)}
							{cli.installed && (
								<span
									class={`text-xs ${cli.authenticated ? "text-success" : "text-text-dim"}`}
								>
									{cli.authenticated ? "Authenticated" : "Not authenticated"}
								</span>
							)}
						</div>
					)}
				</For>
			</div>
		</div>
	);
}
