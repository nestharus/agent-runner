import {
	cleanup,
	fireEvent,
	render,
	screen,
	waitFor,
} from "@solidjs/testing-library";
import { QueryClient, QueryClientProvider } from "@tanstack/solid-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import PoolsView from "../views/PoolsView";

const tauriMock = await vi.importMock<any>("@tauri-apps/api/core");
const setHandler = tauriMock.__setHandler as (
	cmd: string,
	handler: (args?: any) => Promise<unknown>,
) => void;
const clearHandlers = tauriMock.__clearHandlers as () => void;

const noop = () => {};

function renderWithQuery(ui: () => any) {
	const queryClient = new QueryClient({
		defaultOptions: { queries: { retry: false } },
	});
	return render(() => (
		<QueryClientProvider client={queryClient}>{ui()}</QueryClientProvider>
	));
}

beforeEach(() => {
	cleanup();
	clearHandlers();
	vi.clearAllMocks();
});

describe("PoolsView", () => {
	it("renders pool command badges from list_pools", async () => {
		setHandler("list_pools", () =>
			Promise.resolve([
				{
					commands: ["claude", "codex"],
					model_count: 2,
					model_names: ["model-a", "model-b"],
				},
				{
					commands: ["gemini"],
					model_count: 1,
					model_names: ["model-c"],
				},
			]),
		);

		renderWithQuery(() => <PoolsView onRunSetup={noop} />);

		await waitFor(() => {
			// Pool name + command chip both show "claude", so use getAllByText
			expect(screen.getAllByText("claude").length).toBeGreaterThanOrEqual(1);
			expect(screen.getByText("codex")).toBeTruthy();
			expect(screen.getAllByText("gemini").length).toBeGreaterThanOrEqual(1);
		});
	});

	it("shows model count per pool", async () => {
		setHandler("list_pools", () =>
			Promise.resolve([
				{
					commands: ["claude"],
					model_count: 3,
					model_names: ["a", "b", "c"],
				},
			]),
		);

		renderWithQuery(() => <PoolsView onRunSetup={noop} />);

		await waitFor(() => {
			expect(screen.getByText("3 Models")).toBeTruthy();
		});
	});

	it("shows empty state with Run Setup", async () => {
		setHandler("list_pools", () => Promise.resolve([]));

		renderWithQuery(() => <PoolsView onRunSetup={noop} />);

		await waitFor(() => {
			expect(screen.getByText("No pools yet.")).toBeTruthy();
			expect(screen.getByText("Run Setup")).toBeTruthy();
		});
	});

	it("renders command chips for each pool", async () => {
		setHandler("list_pools", () =>
			Promise.resolve([
				{
					commands: ["claude", "codex"],
					model_count: 1,
					model_names: ["model-a"],
				},
			]),
		);

		renderWithQuery(() => <PoolsView onRunSetup={noop} />);

		await waitFor(() => {
			expect(screen.getAllByText("claude").length).toBeGreaterThanOrEqual(1);
			expect(screen.getByText("codex")).toBeTruthy();
		});
	});

	it("shows + button in header that toggles to cancel", async () => {
		setHandler("list_pools", () =>
			Promise.resolve([
				{
					commands: ["claude"],
					model_count: 1,
					model_names: ["model-a"],
				},
			]),
		);

		renderWithQuery(() => <PoolsView onRunSetup={noop} />);

		await waitFor(() => {
			const addBtn = screen.getByTitle("Add provider pool");
			expect(addBtn).toBeTruthy();
		});

		// Click to open add-pool mode — button becomes cancel
		fireEvent.click(screen.getByTitle("Add provider pool"));

		await waitFor(() => {
			expect(screen.getByTitle("Cancel")).toBeTruthy();
		});
	});

	it("shows add-pool inline input when + is clicked", async () => {
		setHandler("list_pools", () =>
			Promise.resolve([
				{
					commands: ["claude"],
					model_count: 1,
					model_names: ["model-a"],
				},
			]),
		);

		renderWithQuery(() => <PoolsView onRunSetup={noop} />);

		await waitFor(() => {
			expect(screen.getByTitle("Add provider pool")).toBeTruthy();
		});

		fireEvent.click(screen.getByTitle("Add provider pool"));

		await waitFor(() => {
			expect(
				screen.getByPlaceholderText("Enter new pool name (e.g., openai)..."),
			).toBeTruthy();
		});
	});

	it("shows models dropdown with interactive items when clicked", async () => {
		setHandler("list_pools", () =>
			Promise.resolve([
				{
					commands: ["claude"],
					model_count: 2,
					model_names: ["sonnet", "opus"],
				},
			]),
		);

		renderWithQuery(() => <PoolsView onRunSetup={noop} />);

		await waitFor(() => {
			expect(screen.getByText("2 Models")).toBeTruthy();
		});

		// Click the models dropdown button
		fireEvent.click(screen.getByText("2 Models"));

		await waitFor(() => {
			expect(screen.getByText("Models (2)")).toBeTruthy();
			expect(screen.getByText("sonnet")).toBeTruthy();
			expect(screen.getByText("opus")).toBeTruthy();
			expect(screen.getByTitle("Add standalone model")).toBeTruthy();
		});
	});

	it("calls delete_model when delete button is clicked in dropdown", async () => {
		let deletedName = "";
		setHandler("list_pools", () =>
			Promise.resolve([
				{
					commands: ["claude"],
					model_count: 2,
					model_names: ["sonnet", "opus"],
				},
			]),
		);
		setHandler("delete_model", (args: any) => {
			deletedName = args.name;
			return Promise.resolve();
		});

		renderWithQuery(() => <PoolsView onRunSetup={noop} />);

		await waitFor(() => {
			expect(screen.getByText("2 Models")).toBeTruthy();
		});

		// Open dropdown
		fireEvent.click(screen.getByText("2 Models"));

		await waitFor(() => {
			expect(screen.getByText("sonnet")).toBeTruthy();
		});

		// Find and click the delete button for "sonnet"
		const deleteButtons = screen.getAllByTitle("Delete model");
		fireEvent.click(deleteButtons[0]);

		await waitFor(() => {
			expect(deletedName).toBe("sonnet");
		});
	});

	it("shows pool settings gear icon", async () => {
		setHandler("list_pools", () =>
			Promise.resolve([
				{
					commands: ["claude"],
					model_count: 1,
					model_names: ["model-a"],
				},
			]),
		);

		renderWithQuery(() => <PoolsView onRunSetup={noop} />);

		await waitFor(() => {
			expect(screen.getByTitle("Pool settings")).toBeTruthy();
		});
	});
});
