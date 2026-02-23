import {
	cleanup,
	fireEvent,
	render,
	screen,
	waitFor,
} from "@solidjs/testing-library";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ChatBox from "../components/ChatBox";

const tauriMock = await vi.importMock<any>("@tauri-apps/api/core");
const setHandler = tauriMock.__setHandler as (
	cmd: string,
	handler: (args?: any) => Promise<unknown>,
) => void;
const clearHandlers = tauriMock.__clearHandlers as () => void;
const getChannelCallback = tauriMock.__getChannelCallback as () =>
	| ((event: any) => void)
	| null;

function sendStreamEvent(event: any) {
	const cb = getChannelCallback();
	if (cb) cb(event);
}

beforeEach(() => {
	cleanup();
	clearHandlers();
	vi.clearAllMocks();
});

describe("ChatBox", () => {
	it("renders with default placeholder text", () => {
		render(() => <ChatBox />);
		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		expect(input.placeholder).toBe("What would you like to do?");
	});

	it("renders with custom placeholder", () => {
		render(() => <ChatBox placeholder="Ask about this pool..." />);
		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		expect(input.placeholder).toBe("Ask about this pool...");
	});

	it("shows empty state message in full mode", () => {
		render(() => <ChatBox mode="full" />);
		expect(screen.getByText("Ask anything to get started")).toBeTruthy();
	});

	it("send button is disabled when input is empty", () => {
		render(() => <ChatBox />);
		const sendBtn = screen.getByTestId("chatbox-send") as HTMLButtonElement;
		expect(sendBtn.disabled).toBe(true);
	});

	it("send button is enabled when input has text", async () => {
		render(() => <ChatBox />);
		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		fireEvent.input(input, { target: { value: "hello" } });

		await waitFor(() => {
			const sendBtn = screen.getByTestId("chatbox-send") as HTMLButtonElement;
			expect(sendBtn.disabled).toBe(false);
		});
	});

	it("sends message on button click and calls chat_send", async () => {
		setHandler("chat_send", () => Promise.resolve());
		render(() => <ChatBox context="test-panel" />);

		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		fireEvent.input(input, { target: { value: "hello world" } });
		fireEvent.click(screen.getByTestId("chatbox-send"));

		await waitFor(() => {
			expect(invoke).toHaveBeenCalledWith(
				"chat_send",
				expect.objectContaining({
					message: "hello world",
					context: "test-panel",
				}),
			);
		});
	});

	it("sends message on Enter key press", async () => {
		setHandler("chat_send", () => Promise.resolve());
		render(() => <ChatBox />);

		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		fireEvent.input(input, { target: { value: "test message" } });
		fireEvent.keyDown(input, { key: "Enter", shiftKey: false });

		await waitFor(() => {
			expect(invoke).toHaveBeenCalledWith(
				"chat_send",
				expect.objectContaining({
					message: "test message",
				}),
			);
		});
	});

	it("does not send on Shift+Enter (allows newline)", async () => {
		render(() => <ChatBox />);

		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		fireEvent.input(input, { target: { value: "line 1" } });
		fireEvent.keyDown(input, { key: "Enter", shiftKey: true });

		// Should not have called chat_send
		expect(invoke).not.toHaveBeenCalledWith("chat_send", expect.anything());
	});

	it("displays user message after sending", async () => {
		setHandler("chat_send", () => Promise.resolve());
		render(() => <ChatBox mode="full" />);

		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		fireEvent.input(input, { target: { value: "my question" } });
		fireEvent.click(screen.getByTestId("chatbox-send"));

		await waitFor(() => {
			expect(screen.getByText("my question")).toBeTruthy();
		});
	});

	it("clears input after sending", async () => {
		setHandler("chat_send", () => Promise.resolve());
		render(() => <ChatBox />);

		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		fireEvent.input(input, { target: { value: "some text" } });
		fireEvent.click(screen.getByTestId("chatbox-send"));

		await waitFor(() => {
			expect(
				(screen.getByTestId("chatbox-input") as HTMLTextAreaElement).value,
			).toBe("");
		});
	});

	it("shows loading indicator while waiting for response", async () => {
		setHandler("chat_send", () => new Promise(() => {})); // never resolves
		render(() => <ChatBox mode="full" />);

		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		fireEvent.input(input, { target: { value: "hello" } });
		fireEvent.click(screen.getByTestId("chatbox-send"));

		await waitFor(() => {
			expect(screen.getByText("Thinking...")).toBeTruthy();
		});
	});

	it("streams assistant response via delta events", async () => {
		setHandler("chat_send", () => Promise.resolve());
		render(() => <ChatBox mode="full" />);

		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		fireEvent.input(input, { target: { value: "hello" } });
		fireEvent.click(screen.getByTestId("chatbox-send"));

		await waitFor(() => {
			expect(invoke).toHaveBeenCalledWith("chat_send", expect.anything());
		});

		sendStreamEvent({ event: "delta", data: { text: "Hello" } });
		sendStreamEvent({ event: "delta", data: { text: " there!" } });
		sendStreamEvent({ event: "done", data: {} });

		await waitFor(() => {
			expect(screen.getByText("Hello there!")).toBeTruthy();
		});
	});

	it("shows error message on stream error", async () => {
		setHandler("chat_send", () => Promise.resolve());
		render(() => <ChatBox mode="full" />);

		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		fireEvent.input(input, { target: { value: "hello" } });
		fireEvent.click(screen.getByTestId("chatbox-send"));

		await waitFor(() => {
			expect(invoke).toHaveBeenCalledWith("chat_send", expect.anything());
		});

		sendStreamEvent({
			event: "error",
			data: { message: "Rate limit exceeded" },
		});

		await waitFor(() => {
			expect(screen.getByText("Rate limit exceeded")).toBeTruthy();
		});
	});

	it("shows error on invoke rejection", async () => {
		setHandler("chat_send", () => Promise.reject("Connection failed"));
		render(() => <ChatBox mode="full" />);

		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		fireEvent.input(input, { target: { value: "hello" } });
		fireEvent.click(screen.getByTestId("chatbox-send"));

		await waitFor(() => {
			expect(screen.getByText("Connection failed")).toBeTruthy();
		});
	});

	it("disables input while loading", async () => {
		setHandler("chat_send", () => new Promise(() => {})); // never resolves
		render(() => <ChatBox />);

		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		fireEvent.input(input, { target: { value: "hello" } });
		fireEvent.click(screen.getByTestId("chatbox-send"));

		await waitFor(() => {
			expect(
				(screen.getByTestId("chatbox-input") as HTMLTextAreaElement).disabled,
			).toBe(true);
		});
	});

	it("compact mode shows last assistant response only", async () => {
		setHandler("chat_send", () => Promise.resolve());
		render(() => <ChatBox mode="compact" />);

		// No history section in compact mode
		expect(screen.queryByTestId("chatbox-history")).toBeNull();

		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		fireEvent.input(input, { target: { value: "hello" } });
		fireEvent.click(screen.getByTestId("chatbox-send"));

		await waitFor(() => {
			expect(invoke).toHaveBeenCalledWith("chat_send", expect.anything());
		});

		sendStreamEvent({ event: "delta", data: { text: "I can help!" } });
		sendStreamEvent({ event: "done", data: {} });

		await waitFor(() => {
			const lastResponse = screen.getByTestId("chatbox-last-response");
			expect(lastResponse).toBeTruthy();
			expect(lastResponse.textContent).toContain("I can help!");
		});
	});

	it("does not send empty or whitespace-only messages", async () => {
		render(() => <ChatBox />);

		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		fireEvent.input(input, { target: { value: "   " } });
		fireEvent.click(screen.getByTestId("chatbox-send"));

		expect(invoke).not.toHaveBeenCalledWith("chat_send", expect.anything());
	});

	it("passes empty context when none provided", async () => {
		setHandler("chat_send", () => Promise.resolve());
		render(() => <ChatBox />);

		const input = screen.getByTestId("chatbox-input") as HTMLTextAreaElement;
		fireEvent.input(input, { target: { value: "hello" } });
		fireEvent.click(screen.getByTestId("chatbox-send"));

		await waitFor(() => {
			expect(invoke).toHaveBeenCalledWith(
				"chat_send",
				expect.objectContaining({
					context: "",
				}),
			);
		});
	});
});
