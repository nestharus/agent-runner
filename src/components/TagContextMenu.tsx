import { onCleanup, onMount } from "solid-js";

interface TagContextMenuProps {
	command: string;
	x: number;
	y: number;
	onDelete: () => void;
	onProperties: () => void;
	onClose: () => void;
}

export default function TagContextMenu(props: TagContextMenuProps) {
	let menuRef: HTMLDivElement | undefined;

	function handleClickOutside(e: MouseEvent) {
		if (menuRef && !menuRef.contains(e.target as Node)) {
			props.onClose();
		}
	}

	function handleKeyDown(e: KeyboardEvent) {
		if (e.key === "Escape") props.onClose();
	}

	onMount(() => {
		document.addEventListener("click", handleClickOutside, true);
		document.addEventListener("keydown", handleKeyDown);
	});

	onCleanup(() => {
		document.removeEventListener("click", handleClickOutside, true);
		document.removeEventListener("keydown", handleKeyDown);
	});

	return (
		<div
			ref={menuRef}
			class="fixed z-50 min-w-[140px] rounded border border-[#2a3a5e] bg-[#16213e] py-1 shadow-lg"
			style={{ left: `${props.x}px`, top: `${props.y}px` }}
		>
			<button
				type="button"
				class="w-full px-3 py-1.5 text-left text-xs text-error transition-colors hover:bg-[#1a2a4e]"
				onClick={() => {
					props.onDelete();
					props.onClose();
				}}
			>
				Delete
			</button>
			<button
				type="button"
				class="w-full px-3 py-1.5 text-left text-xs text-text transition-colors hover:bg-[#1a2a4e]"
				onClick={() => {
					props.onProperties();
					props.onClose();
				}}
			>
				Properties
			</button>
		</div>
	);
}
