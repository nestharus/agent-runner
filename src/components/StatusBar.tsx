import { Show } from "solid-js";
import InlineSpinner from "./InlineSpinner";

interface StatusBarProps {
	message: string;
	visible: boolean;
}

export default function StatusBar(props: StatusBarProps) {
	return (
		<Show when={props.visible}>
			<div class="mb-4 flex items-center gap-3 rounded-lg bg-[#16213e] p-3">
				<InlineSpinner size={18} />
				<span class="text-sm text-text">{props.message}</span>
			</div>
		</Show>
	);
}
