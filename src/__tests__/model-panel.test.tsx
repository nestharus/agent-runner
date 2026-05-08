import {
	cleanup,
	fireEvent,
	render,
	screen,
	waitFor,
} from "@solidjs/testing-library";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ModelPanel from "../components/ModelPanel";
import type { ModelConfig } from "../lib/types";

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

function modelWithCodexArgs(name: string, args: string[]): ModelConfig {
	return {
		name,
		prompt_mode: "stdin",
		providers: [{ name: "codex", args }],
		inputs: [],
	};
}

function renderCodexEditPanel(modelNames: string[], editModelName = modelNames[0]) {
	return render(() => (
		<ModelPanel
			mode="edit"
			poolCommands={["codex"]}
			modelNames={modelNames}
			editModelName={editModelName}
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
	// risk: exhaustive surfaces 63-65; level: frontend component; source: contract § 5.9, A8, A10
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

	// risk: exhaustive surface 64; level: frontend component; source: contract § 5.9, A6, A10
	it("preserves thrown Tauri invoke fallback rendering as exit -1", async () => {
		setHandler("test_model", () => Promise.reject(new Error("invoke failed")));

		renderEditPanel();
		fireEvent.click(screen.getByText("Save & Test"));

		await waitFor(() => {
			expect(screen.getByText("Test failed (exit -1)")).toBeTruthy();
		});
	});

	it("omits common bypass approvals and sandbox flags from the save payload", async () => {
		let savedModel: ModelConfig | null = null;
		const models: Record<string, ModelConfig> = {
			"gpt-high": modelWithCodexArgs("gpt-high", [
				"--dangerously-bypass-approvals-and-sandbox",
				"-m",
				"gpt-5.5",
			]),
			"gpt-low": modelWithCodexArgs("gpt-low", [
				"--dangerously-bypass-approvals-and-sandbox",
				"-m",
				"gpt-5.5",
			]),
		};
		setHandler("get_model", (args: any) => Promise.resolve(models[args.name]));
		setHandler("save_model", (args: any) => {
			savedModel = args.model;
			return Promise.resolve();
		});

		renderCodexEditPanel(["gpt-high", "gpt-low"], "gpt-high");

		await waitFor(() => {
			expect(screen.getByText("codex")).toBeTruthy();
		});
		fireEvent.click(screen.getByText("Save & Test"));

		await waitFor(() => {
			expect(savedModel).not.toBeNull();
		});
		expect(savedModel?.providers[0].args).not.toContain(
			"--dangerously-bypass-approvals-and-sandbox",
		);
	});

	it("omits variable bypass approvals and sandbox flags from the save payload", async () => {
		let savedModel: ModelConfig | null = null;
		const models: Record<string, ModelConfig> = {
			"gpt-high": modelWithCodexArgs("gpt-high", [
				"--dangerously-bypass-approvals-and-sandbox",
				"enabled",
				"-m",
				"gpt-5.5",
			]),
			"gpt-low": modelWithCodexArgs("gpt-low", ["-m", "gpt-5.4"]),
		};
		setHandler("get_model", (args: any) => Promise.resolve(models[args.name]));
		setHandler("save_model", (args: any) => {
			savedModel = args.model;
			return Promise.resolve();
		});

		renderCodexEditPanel(["gpt-high", "gpt-low"], "gpt-high");

		await waitFor(() => {
			expect(screen.getByText("--dangerously-bypass-approvals-and-sandbox")).toBeTruthy();
		});
		fireEvent.click(screen.getByText("Save & Test"));

		await waitFor(() => {
			expect(savedModel).not.toBeNull();
		});
		expect(savedModel?.providers[0].args).not.toContain(
			"--dangerously-bypass-approvals-and-sandbox",
		);
	});

	it("omits yolo flags from common and variable save payload paths", async () => {
		let savedModels: ModelConfig[] = [];
		const commonModels: Record<string, ModelConfig> = {
			"gpt-high": modelWithCodexArgs("gpt-high", ["--yolo", "-m", "gpt-5.5"]),
			"gpt-low": modelWithCodexArgs("gpt-low", ["--yolo", "-m", "gpt-5.5"]),
		};
		setHandler("get_model", (args: any) =>
			Promise.resolve(commonModels[args.name]),
		);
		setHandler("save_model", (args: any) => {
			savedModels.push(args.model);
			return Promise.resolve();
		});

		const commonRender = renderCodexEditPanel(["gpt-high", "gpt-low"], "gpt-high");
		await waitFor(() => {
			expect(screen.getByText("codex")).toBeTruthy();
		});
		fireEvent.click(screen.getByText("Save & Test"));
		await waitFor(() => {
			expect(savedModels.length).toBe(1);
		});
		expect(savedModels[0].providers[0].args).not.toContain("--yolo");
		commonRender.unmount();
		cleanup();

		savedModels = [];
		const variableModels: Record<string, ModelConfig> = {
			"gpt-high": modelWithCodexArgs("gpt-high", [
				"--yolo",
				"enabled",
				"-m",
				"gpt-5.5",
			]),
			"gpt-low": modelWithCodexArgs("gpt-low", ["-m", "gpt-5.4"]),
		};
		setHandler("get_model", (args: any) =>
			Promise.resolve(variableModels[args.name]),
		);

		renderCodexEditPanel(["gpt-high", "gpt-low"], "gpt-high");
		await waitFor(() => {
			expect(screen.getByText("--yolo")).toBeTruthy();
		});
		fireEvent.click(screen.getByText("Save & Test"));
		await waitFor(() => {
			expect(savedModels.length).toBe(1);
		});
		expect(savedModels[0].providers[0].args).not.toContain("--yolo");
	});
});
