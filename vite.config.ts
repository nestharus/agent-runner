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
		target: ["es2021", "chrome105", "safari13"],
	},
});
