import { createSignal, For, Show } from "solid-js";
import OllieSvg from "../components/OllieSvg";
import SetupSession from "../components/SetupSession";
import { startSetup } from "../lib/tauri";

// TODO(design): Setup flow still needs:
// - Status/detecting phase: Ollie searching animation (full-body, needs design)
// - Progress phase: Ollie walking companion (full-body, needs design)
// - Error: Ollie error scene (needs design)
// Success state now uses Ollie portrait. Full animation upgrades pending.

interface SetupViewProps {
	onComplete: () => void;
}

export default function SetupView(props: SetupViewProps) {
	const [running, setRunning] = createSignal(true);
	const [complete, setComplete] = createSignal<{
		summary: string;
		items: string[];
	} | null>(null);

	function handleComplete(summary: string, items: string[]) {
		setRunning(false);
		setComplete({ summary, items });
	}

	function handleCancel() {
		setRunning(false);
		props.onComplete();
	}

	return (
		<div class="mx-auto max-w-3xl p-5">
			<Show when={running()}>
				<SetupSession
					startFn={(ch) => startSetup(ch)}
					onComplete={handleComplete}
					onCancel={handleCancel}
				/>
			</Show>

			<Show when={complete()}>
				{(data) => (
					<div class="py-12 text-center">
						<div class="mb-4 flex justify-center">
							<OllieSvg size={80} />
						</div>
						<h2 class="mb-2 text-xl text-text">Setup Complete</h2>
						<p class="mb-6 text-text-dim">{data().summary}</p>
						<Show when={data().items.length > 0}>
							<ul class="mb-6 flex flex-wrap justify-center gap-2 list-none p-0">
								<For each={data().items}>
									{(item) => (
										<li class="rounded bg-surface px-3 py-1.5 text-[13px]">
											{item}
										</li>
									)}
								</For>
							</ul>
						</Show>
						<button
							type="button"
							class="rounded bg-accent px-5 py-2 text-sm font-medium text-black transition-colors hover:bg-accent-hover"
							onClick={() => props.onComplete()}
						>
							Continue
						</button>
					</div>
				)}
			</Show>
		</div>
	);
}
