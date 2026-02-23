import { defineConfig } from "@playwright/test";

export default defineConfig({
	testDir: "./e2e",
	outputDir: "/tmp/oulipoly-e2e/results",
	snapshotDir: "/tmp/oulipoly-e2e/snapshots",
	use: {
		baseURL: "http://localhost:5173",
		screenshot: "on",
		viewport: { width: 900, height: 700 },
		trace: "on-first-retry",
	},
	projects: [
		{ name: "desktop", use: { viewport: { width: 900, height: 700 } } },
		{ name: "wide", use: { viewport: { width: 1400, height: 900 } } },
		{ name: "narrow", use: { viewport: { width: 480, height: 700 } } },
	],
	webServer: {
		command: "bun run dev",
		url: "http://localhost:5173",
		reuseExistingServer: true,
		timeout: 30000,
	},
});
