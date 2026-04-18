import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";
import solidPlugin from "vite-plugin-solid";

export default defineConfig({
	plugins: [solidPlugin(), tailwindcss()],
	server: {
		port: 5173,
		strictPort: true,
	},
	build: {
		// Tauri v2 ships with WebView2 (Win), WKWebView (macOS), and
		// webkit2gtk (Linux), all of which are modern enough to handle
		// every JS feature the SolidJS compiler emits. Targeting older
		// browsers (es2021/chrome105/safari13) caused esbuild to fail
		// with "Transforming destructuring to the configured target
		// environment is not supported yet" on every release build
		// since SolidJS started emitting more aggressive destructuring
		// patterns (broken since ~March 2026). Drop to esnext.
		target: "esnext",
	},
});
