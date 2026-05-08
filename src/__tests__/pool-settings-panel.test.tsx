import {
	cleanup,
	fireEvent,
	render,
	screen,
	waitFor,
} from "@solidjs/testing-library";
import { beforeEach, describe, expect, it, vi } from "vitest";
import PoolSettingsPanel from "../components/PoolSettingsPanel";
import { PROVIDER_LEVEL_FLAG_LABELS } from "../lib/providerFlags";

beforeEach(() => {
	cleanup();
	vi.clearAllMocks();
});

describe("PoolSettingsPanel", () => {
	it("renders labels for every known provider-level flag", () => {
		render(() => (
			<PoolSettingsPanel
				poolCommands={["codex"]}
				commonFlags={Object.keys(PROVIDER_LEVEL_FLAG_LABELS)}
				onToggleFlag={() => {}}
				onClose={() => {}}
			/>
		));

		for (const label of Object.values(PROVIDER_LEVEL_FLAG_LABELS)) {
			expect(screen.getByText(label)).toBeTruthy();
		}
	});

	it("renders the empty pool-level flag state", () => {
		render(() => (
			<PoolSettingsPanel
				poolCommands={["codex"]}
				commonFlags={[]}
				onToggleFlag={() => {}}
				onClose={() => {}}
			/>
		));

		expect(screen.getByText("No pool-level flags detected.")).toBeTruthy();
	});

	it("calls onClose exactly once when the dialog is dismissed with Escape", async () => {
		const onClose = vi.fn();
		render(() => (
			<PoolSettingsPanel
				poolCommands={["codex"]}
				commonFlags={[]}
				onToggleFlag={() => {}}
				onClose={onClose}
			/>
		));

		fireEvent.keyDown(document, { key: "Escape" });

		await waitFor(() => {
			expect(onClose).toHaveBeenCalledTimes(1);
		});
	});

	it("does not call onToggleFlag when a denied provider-level flag row is interacted with", async () => {
		const onToggleFlag = vi.fn();
		render(() => (
			<PoolSettingsPanel
				poolCommands={["codex"]}
				commonFlags={["--dangerously-bypass-approvals-and-sandbox"]}
				onToggleFlag={onToggleFlag}
				onClose={() => {}}
			/>
		));

		fireEvent.click(screen.getByText("Bypass Approvals & Sandbox"));
		fireEvent.keyDown(screen.getByText("Bypass Approvals & Sandbox"), {
			key: "Enter",
		});

		await waitFor(() => {
			expect(onToggleFlag).not.toHaveBeenCalled();
		});
	});
});
