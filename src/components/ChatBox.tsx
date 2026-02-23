import { faPaperPlane, faSparkle } from "@fortawesome/sharp-solid-svg-icons";
import { createSignal, For, Show } from "solid-js";
import { Channel, chatSend } from "../lib/tauri";
import type { ChatMessage, ChatStreamEvent } from "../lib/types";
import Icon from "./Icon";

interface ChatBoxProps {
	/** Compact mode shows only the input and the last response. Full mode shows scrollable history. */
	mode?: "compact" | "full";
	/** Optional context string sent alongside each message (e.g. panel name, current state). */
	context?: string;
	/** Placeholder text for the input. */
	placeholder?: string;
}

export default function ChatBox(props: ChatBoxProps) {
	const [messages, setMessages] = createSignal<ChatMessage[]>([]);
	const [input, setInput] = createSignal("");
	const [loading, setLoading] = createSignal(false);
	const [error, setError] = createSignal<string | null>(null);
	let scrollRef: HTMLDivElement | undefined;

	const isCompact = () => (props.mode ?? "full") === "compact";
	const placeholder = () => props.placeholder ?? "What would you like to do?";

	function scrollToBottom() {
		const el = scrollRef;
		if (el) {
			requestAnimationFrame(() => {
				el.scrollTop = el.scrollHeight;
			});
		}
	}

	async function handleSend() {
		const text = input().trim();
		if (!text || loading()) return;

		setError(null);
		setInput("");
		setMessages((prev) => [...prev, { role: "user", content: text }]);

		// Add a placeholder assistant message for streaming
		setMessages((prev) => [...prev, { role: "assistant", content: "" }]);
		setLoading(true);
		scrollToBottom();

		const channel = new Channel<ChatStreamEvent>();
		channel.onmessage = (event: ChatStreamEvent) => {
			if (event.event === "delta") {
				setMessages((prev) => {
					const next = [...prev];
					const last = next[next.length - 1];
					if (last && last.role === "assistant") {
						next[next.length - 1] = {
							...last,
							content: last.content + event.data.text,
						};
					}
					return next;
				});
				scrollToBottom();
			} else if (event.event === "done") {
				setLoading(false);
			} else if (event.event === "error") {
				setError(event.data.message);
				setLoading(false);
				// Remove the empty assistant message on error
				setMessages((prev) => {
					const last = prev[prev.length - 1];
					if (last && last.role === "assistant" && last.content === "") {
						return prev.slice(0, -1);
					}
					return prev;
				});
			}
		};

		try {
			await chatSend(text, props.context ?? "", channel);
		} catch (err) {
			setError(String(err));
			setLoading(false);
			// Remove the empty assistant message on error
			setMessages((prev) => {
				const last = prev[prev.length - 1];
				if (last && last.role === "assistant" && last.content === "") {
					return prev.slice(0, -1);
				}
				return prev;
			});
		}
	}

	function handleKeyDown(e: KeyboardEvent) {
		if (e.key === "Enter" && !e.shiftKey) {
			e.preventDefault();
			handleSend();
		}
	}

	/** In compact mode, show only the last assistant response. */
	const lastAssistantMessage = () => {
		const msgs = messages();
		for (let i = msgs.length - 1; i >= 0; i--) {
			if (msgs[i].role === "assistant" && msgs[i].content) {
				return msgs[i];
			}
		}
		return null;
	};

	return (
		<div
			class={`flex flex-col ${isCompact() ? "" : "h-full"}`}
			data-testid="chatbox"
		>
			{/* Message history (full mode) */}
			<Show when={!isCompact()}>
				<div
					ref={scrollRef}
					class="flex-1 overflow-y-auto px-3 py-2"
					data-testid="chatbox-history"
				>
					<Show
						when={messages().length > 0}
						fallback={
							<div class="flex h-full items-center justify-center">
								<div class="text-center">
									<Icon
										icon={faSparkle}
										size={24}
										class="mx-auto mb-2 text-text-faint"
									/>
									<p class="text-sm text-text-dim">
										Ask anything to get started
									</p>
								</div>
							</div>
						}
					>
						<For each={messages()}>
							{(msg) => (
								<div
									class={`mb-3 ${msg.role === "user" ? "flex justify-end" : ""}`}
								>
									<div
										class={`max-w-[85%] rounded-lg px-3 py-2 text-sm ${
											msg.role === "user"
												? "bg-accent/15 text-text"
												: "bg-surface-alt text-text"
										}`}
									>
										<Show
											when={
												msg.role === "assistant" && !msg.content && loading()
											}
										>
											<div class="flex items-center gap-2 text-text-dim">
												<div class="h-3 w-3 animate-spin rounded-full border border-border border-t-accent" />
												<span class="text-xs">Thinking...</span>
											</div>
										</Show>
										<Show when={msg.content}>
											<pre class="whitespace-pre-wrap font-sans">
												{msg.content}
											</pre>
										</Show>
									</div>
								</div>
							)}
						</For>
					</Show>
				</div>
			</Show>

			{/* Last response (compact mode) */}
			<Show when={isCompact()}>
				<Show when={loading() && !lastAssistantMessage()}>
					<div class="flex items-center gap-2 px-3 py-2 text-text-dim">
						<div class="h-3 w-3 animate-spin rounded-full border border-border border-t-accent" />
						<span class="text-xs">Thinking...</span>
					</div>
				</Show>
				<Show when={lastAssistantMessage()}>
					{(msg) => (
						<div class="px-3 py-2" data-testid="chatbox-last-response">
							<div class="rounded-lg bg-surface-alt px-3 py-2 text-sm text-text">
								<pre class="whitespace-pre-wrap font-sans">{msg().content}</pre>
								<Show when={loading()}>
									<div class="mt-1 flex items-center gap-2 text-text-dim">
										<div class="h-3 w-3 animate-spin rounded-full border border-border border-t-accent" />
										<span class="text-xs">Thinking...</span>
									</div>
								</Show>
							</div>
						</div>
					)}
				</Show>
			</Show>

			{/* Error */}
			<Show when={error()}>
				<div class="mx-3 mb-2 rounded border-l-[3px] border-error bg-error/15 p-2 text-xs text-error">
					{error()}
				</div>
			</Show>

			{/* Input area */}
			<div class="flex items-end gap-2 border-t border-border px-3 py-2">
				<textarea
					class="flex-1 resize-none rounded border border-border bg-surface-alt px-3 py-2 text-sm text-text outline-none transition-colors placeholder:text-text-faint focus:border-accent"
					placeholder={placeholder()}
					value={input()}
					onInput={(e) => setInput(e.currentTarget.value)}
					onKeyDown={handleKeyDown}
					rows={1}
					disabled={loading()}
					data-testid="chatbox-input"
				/>
				<button
					type="button"
					class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-accent text-black transition-colors hover:bg-accent-hover disabled:opacity-50"
					onClick={handleSend}
					disabled={loading() || !input().trim()}
					title="Send message"
					data-testid="chatbox-send"
				>
					<Icon icon={faPaperPlane} size={14} />
				</button>
			</div>
		</div>
	);
}
