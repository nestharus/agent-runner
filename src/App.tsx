import { createSignal, onMount, Show } from "solid-js";
import { checkSetupNeeded } from "./lib/tauri";
import PoolsView from "./views/PoolsView";
import SetupView from "./views/SetupView";

export default function App() {
	const [showSetup, setShowSetup] = createSignal(false);

	onMount(async () => {
		try {
			const needed = await checkSetupNeeded();
			if (needed) setShowSetup(true);
		} catch (err) {
			console.error("Failed to check setup:", err);
		}
	});

	// TODO(design): No branding anywhere in the app.
	// Need wordmark/logo for "Oulipoly Plane" in the header area.
	// Still needs design from designer.

	return (
		<div class="min-h-screen bg-bg font-sans text-text">
			<main>
				<Show
					when={showSetup()}
					fallback={<PoolsView onRunSetup={() => setShowSetup(true)} />}
				>
					<SetupView onComplete={() => setShowSetup(false)} />
				</Show>
			</main>
		</div>
	);
}
