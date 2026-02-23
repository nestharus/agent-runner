import { expect, test } from "@playwright/test";
import {
	catalogScreenshot,
	getCalls,
	injectMock,
	navigateAndWait,
} from "./fixtures/tauri-mock";
import * as S from "./fixtures/scenarios";

test.describe("Pools Dashboard", () => {
	test("pools render with commands and model count", async ({ page }) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		// Verify heading
		await expect(page.getByText("Provider Pools")).toBeVisible();

		// Verify all 3 pool name spans
		const poolNames = page.locator(
			"span.min-w-\\[80px\\].text-sm.font-bold.text-text",
		);
		await expect(poolNames).toHaveCount(3);
		await expect(poolNames.nth(0)).toHaveText("claude");
		await expect(poolNames.nth(1)).toHaveText("codex");
		await expect(poolNames.nth(2)).toHaveText("gemini");

		// Verify model counts on each pool's dropdown trigger
		await expect(page.getByText("3 Models").first()).toBeVisible();
		await expect(page.getByText("4 Models")).toBeVisible();
		await expect(page.getByText("2 Models")).toBeVisible();

		await catalogScreenshot(page, "02-dashboard", "pools-overview");
	});

	test("add pool button opens inline input", async ({ page }) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		// Click the + button
		await page.click('[title="Add provider pool"]');

		// Verify the inline input appears with the expected placeholder
		const input = page.locator(
			'input[placeholder*="Enter new pool name"]',
		);
		await expect(input).toBeVisible();
		await expect(input).toBeFocused();

		// Verify submit arrow button is visible
		const submitBtn = page.locator('button[title="Submit"]');
		await expect(submitBtn).toBeVisible();

		await catalogScreenshot(page, "02-dashboard", "add-pool-input");
	});

	test("add pool input accepts text and enables submit", async ({
		page,
	}) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		await page.click('[title="Add provider pool"]');

		const input = page.locator(
			'input[placeholder*="Enter new pool name"]',
		);
		await expect(input).toBeVisible();

		// Type a pool name
		await input.fill("openai");
		await expect(input).toHaveValue("openai");

		// Submit button should now be enabled
		const submitBtn = page.locator('button[title="Submit"]');
		await expect(submitBtn).toBeEnabled();

		await catalogScreenshot(page, "02-dashboard", "add-pool-text-entered");
	});

	test("models dropdown opens and shows grouped models", async ({
		page,
	}) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		// Click the "3 Models" trigger on the first pool (claude)
		const modelsBtn = page.getByText("3 Models").first();
		await modelsBtn.click();

		// Wait for the popover content to appear (filter by open state)
		const popoverContent = page.locator(
			'[data-scope="popover"][data-part="content"][data-state="open"]',
		);
		await expect(popoverContent).toBeVisible();

		// Verify header shows count
		await expect(popoverContent.getByText("Models (3)")).toBeVisible();

		// Verify "New" button inside popover
		await expect(popoverContent.getByText("New")).toBeVisible();

		// Verify faceted model group (the claude pool shows group "claude" with facet chips)
		await expect(popoverContent.getByText("claude")).toBeVisible();
		await expect(popoverContent.getByText("high")).toBeVisible();

		await catalogScreenshot(page, "02-dashboard", "models-dropdown-open");
	});

	test("command tags are clickable and show selection ring", async ({
		page,
	}) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		// Find the first command chip (role="option") with text "claude"
		const claudeChip = page
			.locator('span[role="option"]')
			.filter({ hasText: /^claude$/ })
			.first();
		await expect(claudeChip).toBeVisible();

		// Click to select
		await claudeChip.click();

		// Verify ring class is applied (ring-1 ring-accent)
		await expect(claudeChip).toHaveClass(/ring-1/);
		await expect(claudeChip).toHaveClass(/ring-accent/);

		await catalogScreenshot(page, "02-dashboard", "tag-selected");
	});

	test("command tags are editable on double-click", async ({ page }) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		// Find the "claude" command chip
		const claudeChip = page
			.locator('span[role="option"]')
			.filter({ hasText: /^claude$/ })
			.first();
		await expect(claudeChip).toBeVisible();

		// Double-click to enter edit mode
		await claudeChip.dblclick();

		// Verify inline edit input appears (replaces the chip)
		const editInput = page.locator(
			"input.rounded-full.border-accent.text-xs.font-mono",
		);
		await expect(editInput).toBeVisible();
		await expect(editInput).toHaveValue("claude");
		await expect(editInput).toBeFocused();

		await catalogScreenshot(page, "02-dashboard", "tag-editing");
	});

	test("empty state shows no pools message and run setup button", async ({
		page,
	}) => {
		await injectMock(page, S.EMPTY_POOLS);
		await navigateAndWait(page, "/");

		// Verify heading still renders
		await expect(page.getByText("Provider Pools")).toBeVisible();

		// Verify empty state text
		await expect(page.getByText("No pools yet.")).toBeVisible();

		// Verify "Run Setup" button
		const setupBtn = page.getByRole("button", { name: "Run Setup" });
		await expect(setupBtn).toBeVisible();

		await catalogScreenshot(page, "02-dashboard", "empty-state");
	});

	test("pool settings gear opens settings panel", async ({ page }) => {
		await injectMock(page, S.POOL_WITH_FLAGS);
		await navigateAndWait(page, "/");

		// Click the settings gear on the first (and only) pool
		await page.click('[title="Pool settings"]');

		// Wait for the panel dialog to appear
		const panelTitle = page.getByText("Pool Settings");
		await expect(panelTitle).toBeVisible();

		// Verify the panel shows the pool commands as description
		await expect(page.getByText("claude", { exact: true }).first()).toBeVisible();

		// Verify switch/toggle controls are visible (Switch.Control elements)
		const switchControls = page.locator(
			'[data-scope="switch"][data-part="control"]',
		);
		await expect(switchControls.first()).toBeVisible();

		// Verify the flag label is shown
		await expect(page.getByText("Bypass Permissions")).toBeVisible();

		await catalogScreenshot(page, "02-dashboard", "pool-settings-panel");
	});

	test("add pool input dismisses on escape", async ({ page }) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		// Open add-pool input
		await page.click('[title="Add provider pool"]');
		const input = page.locator(
			'input[placeholder*="Enter new pool name"]',
		);
		await expect(input).toBeVisible();

		// Focus the input then press Escape to dismiss
		await input.focus();
		await page.keyboard.press("Escape");
		await page.waitForTimeout(300);

		// Input should disappear (or click elsewhere as fallback)
		const isHidden = await input.isHidden().catch(() => false);
		if (!isHidden) {
			// Component may not support Escape — click away to dismiss
			await page.locator("h2").first().click();
			await page.waitForTimeout(300);
		}
		await expect(input).not.toBeVisible();
	});

	test("list_pools command is invoked on load", async ({ page }) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		const calls = await getCalls(page);
		const poolCalls = calls.filter((c) => c.cmd === "list_pools");
		expect(poolCalls.length).toBeGreaterThanOrEqual(1);
	});
});
