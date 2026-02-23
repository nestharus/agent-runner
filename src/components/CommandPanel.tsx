import { createSignal, onCleanup, onMount } from "solid-js";

interface CommandPanelProps {
	command: string;
	onSave: (newCommand: string) => void;
	onClose: () => void;
}

export default function CommandPanel(props: CommandPanelProps) {
	const [value, setValue] = createSignal(props.command);
	const [visible, setVisible] = createSignal(false);

	onMount(() => {
		requestAnimationFrame(() => setVisible(true));
	});

	function handleKeyDown(e: KeyboardEvent) {
		if (e.key === "Escape") props.onClose();
	}

	onMount(() => {
		document.addEventListener("keydown", handleKeyDown);
	});

	onCleanup(() => {
		document.removeEventListener("keydown", handleKeyDown);
	});

	function handleSave() {
		const trimmed = value().trim();
		if (trimmed && trimmed !== props.command) {
			props.onSave(trimmed);
		} else {
			props.onClose();
		}
	}

	return (
		<>
			{/* Backdrop */}
			{/* biome-ignore lint/a11y/noStaticElementInteractions: backdrop dismiss */}
			{/* biome-ignore lint/a11y/useKeyWithClickEvents: backdrop dismiss */}
			<div
				class="fixed inset-0 z-40 bg-black/40 transition-opacity duration-200"
				style={{ opacity: visible() ? "1" : "0" }}
				onClick={() => props.onClose()}
			/>
			{/* Panel */}
			<div
				class="fixed right-0 top-0 z-50 flex h-full w-80 shadow-lg transition-transform duration-200"
				style={{
					transform: visible() ? "translateX(0)" : "translateX(100%)",
				}}
			>
				{/* Docking indicator */}
				<div class="absolute left-0 top-0 h-full w-0.5 bg-accent" />

				<div class="flex h-full w-full flex-col bg-surface pl-1">
					{/* Header */}
					<div class="border-b border-border px-4 py-3">
						<h3 class="text-sm font-medium text-text">Command Properties</h3>
					</div>

					{/* Body */}
					<div class="flex-1 overflow-y-auto px-4 py-4">
						<label class="block">
							<span class="mb-1 block text-xs text-text-dim">Command name</span>
							<input
								type="text"
								class="w-full rounded border border-border bg-surface-alt px-3 py-2 text-sm font-mono text-text outline-none transition-colors focus:border-accent"
								value={value()}
								onInput={(e) => setValue(e.currentTarget.value)}
								onKeyDown={(e) => {
									if (e.key === "Enter") handleSave();
								}}
								ref={(el) => requestAnimationFrame(() => el.focus())}
							/>
						</label>
					</div>

					{/* Footer */}
					<div class="border-t border-border px-4 py-3">
						<button
							type="button"
							class="w-full rounded-lg bg-accent py-2 text-sm font-medium text-black transition-colors hover:bg-accent-hover"
							onClick={handleSave}
						>
							Save
						</button>
					</div>
				</div>
			</div>
		</>
	);
}
