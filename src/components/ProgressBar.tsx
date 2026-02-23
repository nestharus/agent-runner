import { Progress } from "@ark-ui/solid";
import { Show } from "solid-js";

// TODO(design): Progress screen is a bare bar in empty space.
// Need Ollie walking companion illustration alongside the progress bar.
// Full-body Ollie walking animation still needs design from designer.

interface ProgressBarProps {
	message: string;
	percent: number | null;
	detail: string | null;
	visible: boolean;
}

export default function ProgressBar(props: ProgressBarProps) {
	return (
		<Show when={props.visible}>
			<Progress.Root
				value={props.percent ?? 0}
				min={0}
				max={100}
				class="mb-4 rounded-lg bg-[#16213e] p-4"
			>
				<Progress.Label class="text-sm text-text">
					{props.message}
				</Progress.Label>
				<Progress.Track class="mt-2 h-1.5 overflow-hidden rounded-full bg-[#2a3a5e]">
					<Progress.Range class="h-full rounded-full bg-accent transition-[width] duration-300" />
				</Progress.Track>
				<Show when={props.detail}>
					<Progress.ValueText class="mt-1.5 text-xs text-text-dim">
						{props.detail}
					</Progress.ValueText>
				</Show>
			</Progress.Root>
		</Show>
	);
}
