import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMock = await vi.importMock<any>("@tauri-apps/api/core");
const clearHandlers = tauriMock.__clearHandlers as () => void;

beforeEach(() => {
	clearHandlers();
	vi.clearAllMocks();
});

describe("Tauri unit-test mock boundary", () => {
	it("currently resolves unregistered commands with undefined", async () => {
		await expect(
			invoke("unregistered_example_command"),
		).resolves.toBeUndefined();

		expect(invoke).toHaveBeenCalledWith("unregistered_example_command");
	});
});
