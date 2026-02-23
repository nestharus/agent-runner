import { createSignal, For } from "solid-js";
import { badge, button, card } from "../lib/styles";
import type { CliOption } from "../lib/types";

interface CliSelectorProps {
	message: string;
	available: CliOption[];
	onSubmit: (selected: string[]) => void;
}

export default function CliSelector(props: CliSelectorProps) {
	const [selected, setSelected] = createSignal<Set<string>>(
		new Set(props.available.filter((c) => c.installed).map((c) => c.name)),
	);

	function toggle(name: string) {
		const next = new Set(selected());
		if (next.has(name)) {
			next.delete(name);
		} else {
			next.add(name);
		}
		setSelected(next);
	}

	return (
		<div class={card()}>
			<h3 class="mb-3 text-text">{props.message}</h3>

			<For each={props.available}>
				{(cli) => (
					<label class="flex cursor-pointer items-center gap-2.5 border-b border-[#2a3a5e] py-2.5">
						<input
							type="checkbox"
							class="h-4 w-4"
							value={cli.name}
							checked={selected().has(cli.name)}
							disabled={!cli.installed}
							onChange={() => toggle(cli.name)}
						/>
						<span class="min-w-[80px] text-sm font-medium">{cli.name}</span>
						<span
							class={`min-w-[80px] text-xs ${badge({ status: cli.installed ? "success" : "error" })}`}
						>
							{cli.installed ? "Installed" : "Not found"}
						</span>
						<span class="text-xs text-text-dim">{cli.description}</span>
					</label>
				)}
			</For>

			<div class="mt-4 flex justify-end">
				<button
					type="button"
					class={button({ intent: "primary" })}
					onClick={() => props.onSubmit([...selected()])}
				>
					Continue
				</button>
			</div>
		</div>
	);
}
