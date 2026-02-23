import { vi } from "vitest";

// Mock @tauri-apps/api/core for all tests
vi.mock("@tauri-apps/api/core", () => {
	const handlers: Record<string, (args?: unknown) => Promise<unknown>> = {};
	let channelCallback: ((event: unknown) => void) | null = null;

	const defaultHandlers: Record<string, () => Promise<unknown>> = {
		list_models: () => Promise.resolve([]),
		check_setup_needed: () => Promise.resolve(false),
		detect_clis: () =>
			Promise.resolve({
				clis: [],
				os: { os_type: "linux", arch: "x86_64" },
				wrappers: [],
			}),
		get_model: () =>
			Promise.resolve({ name: "test", prompt_mode: "stdin", providers: [] }),
		save_model: () => Promise.resolve(),
		delete_model: () => Promise.resolve(),
		list_pools: () => Promise.resolve([]),
		update_pool: () => Promise.resolve(),
		start_cli_setup: () => Promise.resolve("session-id"),
		reload_models: () => Promise.resolve(),
		test_model: () =>
			Promise.resolve({
				success: true,
				stdout: "Hello!",
				stderr: "",
				exit_code: 0,
			}),
		chat_send: () => Promise.resolve(),
	};

	return {
		invoke: vi.fn((cmd: string, args?: unknown) => {
			if (handlers[cmd]) return handlers[cmd](args);
			if (defaultHandlers[cmd]) return defaultHandlers[cmd]();
			return Promise.resolve();
		}),
		Channel: vi.fn().mockImplementation(function (this: {
			onmessage: ((event: unknown) => void) | null;
		}) {
			this.onmessage = null;

			// Expose callback for test injection
			Object.defineProperty(this, "__send", {
				value: (event: unknown) => {
					if (this.onmessage) this.onmessage(event);
				},
				writable: true,
			});
			channelCallback = (event: unknown) => {
				if (this.onmessage) this.onmessage(event);
			};
		}),
		__setHandler: (
			cmd: string,
			handler: (args?: unknown) => Promise<unknown>,
		) => {
			handlers[cmd] = handler;
		},
		__clearHandlers: () => {
			for (const key in handlers) delete handlers[key];
		},
		__getChannelCallback: () => channelCallback,
	};
});
