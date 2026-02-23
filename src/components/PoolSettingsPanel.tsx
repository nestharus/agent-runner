import { Dialog, Switch } from "@ark-ui/solid";
import { For, Show } from "solid-js";

interface PoolSettingsPanelProps {
	poolCommands: string[];
	commonFlags: string[];
	onToggleFlag: (flag: string, enabled: boolean) => void;
	onClose: () => void;
}

const FLAG_LABELS: Record<string, string> = {
	"--dangerously-skip-permissions": "Bypass Permissions",
	"--dangerously-bypass-approvals-and-sandbox": "Bypass Approvals & Sandbox",
	"--dangerously-bypass-approvals": "Bypass Approvals",
	"--yolo": "YOLO Mode",
};

export default function PoolSettingsPanel(props: PoolSettingsPanelProps) {
	function flagLabel(flag: string): string {
		return FLAG_LABELS[flag] ?? flag;
	}

	return (
		<Dialog.Root
			open={true}
			onOpenChange={(e) => {
				if (!e.open) props.onClose();
			}}
			closeOnEscape
		>
			<Dialog.Backdrop class="fixed inset-0 z-40 bg-black/40" />
			<Dialog.Positioner class="fixed inset-0 z-50 flex justify-end">
				<Dialog.Content class="animate-slide-in relative flex h-full w-80 flex-col shadow-lg">
					{/* Docking indicator */}
					<div class="absolute left-0 top-0 h-full w-0.5 bg-accent" />

					<div class="flex h-full flex-col bg-surface pl-1">
						{/* Header */}
						<div class="border-b border-border px-4 py-3">
							<Dialog.Title class="text-sm font-medium text-text">
								Pool Settings
							</Dialog.Title>
							<Dialog.Description class="mt-0.5 text-xs text-text-dim">
								{props.poolCommands.join(", ")}
							</Dialog.Description>
						</div>

						{/* Body */}
						<div class="flex-1 overflow-y-auto px-4 py-4">
							<Show
								when={props.commonFlags.length > 0}
								fallback={
									<p class="text-xs text-text-faint">
										No pool-level flags detected.
									</p>
								}
							>
								<div class="space-y-3">
									<For each={props.commonFlags}>
										{(flag) => (
											<Switch.Root
												checked={true}
												onCheckedChange={(e) =>
													props.onToggleFlag(flag, e.checked)
												}
												class="flex cursor-pointer items-center justify-between gap-3"
											>
												<div>
													<Switch.Label class="text-xs font-medium text-text">
														{flagLabel(flag)}
													</Switch.Label>
													<div class="text-[10px] font-mono text-text-faint">
														{flag}
													</div>
												</div>
												<Switch.Control class="relative h-5 w-9 rounded-full bg-border transition-colors data-[state=checked]:bg-accent">
													<Switch.Thumb class="absolute left-0.5 top-0.5 h-4 w-4 rounded-full bg-text-dim transition-transform data-[state=checked]:translate-x-4 data-[state=checked]:bg-white" />
												</Switch.Control>
												<Switch.HiddenInput />
											</Switch.Root>
										)}
									</For>
								</div>
							</Show>
						</div>
					</div>
				</Dialog.Content>
			</Dialog.Positioner>
		</Dialog.Root>
	);
}
