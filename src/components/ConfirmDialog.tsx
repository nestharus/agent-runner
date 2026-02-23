import { button, card } from "../lib/styles";

interface ConfirmDialogProps {
	title: string;
	message: string;
	confirmLabel: string | null;
	cancelLabel: string | null;
	onConfirm: () => void;
	onCancel: () => void;
}

export default function ConfirmDialog(props: ConfirmDialogProps) {
	return (
		<div class={card()}>
			<h3 class="mb-3 text-text">{props.title}</h3>
			<p class="text-sm text-text">{props.message}</p>
			<div class="mt-4 flex justify-end gap-2">
				<button
					type="button"
					class={button({ intent: "secondary" })}
					onClick={props.onCancel}
				>
					{props.cancelLabel || "Cancel"}
				</button>
				<button
					type="button"
					class={button({ intent: "primary" })}
					onClick={props.onConfirm}
				>
					{props.confirmLabel || "Confirm"}
				</button>
			</div>
		</div>
	);
}
