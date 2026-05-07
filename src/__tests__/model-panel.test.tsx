import {
	cleanup,
	fireEvent,
	render,
	screen,
	waitFor,
} from "@solidjs/testing-library";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ModelPanel from "../components/ModelPanel";

const tauriMock = await vi.importMock<any>("@tauri-apps/api/core");
const setHandler = tauriMock.__setHandler as (
	cmd: string,
	handler: (args?: any) => Promise<unknown>,
) => void;
const clearHandlers = tauriMock.__clearHandlers as () => void;

function renderEditPanel() {
	return render(() => (
		<ModelPanel
			mode="edit"
			poolCommands={["sh"]}
			modelNames={[]}
			editModelName="sigterm-model"
			onSave={() => {}}
			onClose={() => {}}
		/>
	));
}

beforeEach(() => {
	cleanup();
	clearHandlers();
	vi.clearAllMocks();
});

describe("ModelPanel", () => {
	// risk: exhaustive surfaces 63-65; level: frontend component; source: proposals/nes-261-NES-261.md L303-305, A8/A10
	it("renders Rust-originated SIGTERM model test result as exit 143", async () => {
		setHandler("test_model", () =>
			Promise.resolve({
				success: false,
				stdout: "",
				stderr: "",
				exit_code: 143,
			}),
		);

		renderEditPanel();
		fireEvent.click(screen.getByText("Save & Test"));

		await waitFor(() => {
			expect(screen.getByText("Test failed (exit 143)")).toBeTruthy();
		});
	});

	// risk: exhaustive surface 64; level: frontend component; source: proposals/nes-261-NES-261.md L303-305, L247-251, A6/A10
	it("preserves thrown Tauri invoke fallback rendering as exit -1", async () => {
		setHandler("test_model", () => Promise.reject(new Error("invoke failed")));

		renderEditPanel();
		fireEvent.click(screen.getByText("Save & Test"));

		await waitFor(() => {
			expect(screen.getByText("Test failed (exit -1)")).toBeTruthy();
		});
	});
});
