import { expect, test } from "@playwright/test";
import {
	injectMock,
	navigateAndWait,
	catalogScreenshot,
	getCalls,
} from "./fixtures/tauri-mock";
import * as S from "./fixtures/scenarios";

/**
 * E2E test suite for the setup wizard flow.
 *
 * Each test injects a Tauri mock with the appropriate scenario,
 * navigates to `/`, waits for the setup view to render, makes
 * structural assertions, and captures a catalog screenshot.
 */
test.describe("Setup Wizard", () => {
	// ----------------------------------------------------------------
	// 1. Status bar with spinner
	// ----------------------------------------------------------------
	test("fresh user sees status bar", async ({ page }) => {
		await injectMock(page, S.STATUS_DETECTING);
		await navigateAndWait(page, "/");

		// The spinner is a div with animate-spin class inside the status bar
		const spinner = page.locator(".animate-spin");
		await expect(spinner).toBeVisible();

		// Status message text
		const statusText = page.locator("span", {
			hasText: "Detecting installed CLIs...",
		});
		await expect(statusText).toBeVisible();

		// Verify the start_setup command was invoked
		const calls = await getCalls(page);
		const startCall = calls.find((c) => c.cmd === "start_setup");
		expect(startCall).toBeTruthy();

		await catalogScreenshot(page, "01-wizard", "01-status-bar");
	});

	// ----------------------------------------------------------------
	// 2. Progress bar with percentage
	// ----------------------------------------------------------------
	test("progress bar shows percentage", async ({ page }) => {
		await injectMock(page, S.PROGRESS_BAR);
		await navigateAndWait(page, "/");

		// Progress root element
		const progressRoot = page.locator("[data-scope='progress'][data-part='root']");
		await expect(progressRoot).toBeVisible();

		const labelText = page.getByText("Configuring model pools...");
		await expect(labelText).toBeVisible();

		// Progress track element exists
		const track = page.locator("[data-scope='progress'][data-part='track']");
		await expect(track).toBeVisible();

		// Detail text showing pool info
		const detail = page.getByText("Pool 2 of 3: openai-gpt4o");
		await expect(detail).toBeVisible();

		await catalogScreenshot(page, "01-wizard", "02-progress-bar");
	});

	// ----------------------------------------------------------------
	// 3. Detection summary table
	// ----------------------------------------------------------------
	test("detection summary renders CLI table", async ({ page }) => {
		await injectMock(page, S.DETECTION_SUMMARY);
		await navigateAndWait(page, "/");

		// Table heading
		const heading = page.getByText("Detected CLIs");
		await expect(heading).toBeVisible();

		// Table element exists
		const table = page.locator("table");
		await expect(table).toBeVisible();

		// 5 header columns: CLI, Installed, Version, Auth, Wrappers
		const headers = table.locator("th");
		await expect(headers).toHaveCount(5);

		// 3 data rows (claude, codex, gemini)
		const dataRows = table.locator("tbody tr");
		await expect(dataRows).toHaveCount(3);

		// Verify CLI names appear in the table
		await expect(table.getByText("claude")).toBeVisible();
		await expect(table.getByText("codex")).toBeVisible();
		await expect(table.getByText("gemini")).toBeVisible();

		// Verify installed status: claude=Yes, codex=Yes, gemini=No
		const installedCells = table.locator("tbody td:nth-child(2)");
		await expect(installedCells.nth(0)).toHaveText("Yes");
		await expect(installedCells.nth(1)).toHaveText("Yes");
		await expect(installedCells.nth(2)).toHaveText("No");

		// Verify version for claude
		const versionCells = table.locator("tbody td:nth-child(3)");
		await expect(versionCells.nth(0)).toHaveText("1.2.3");

		await catalogScreenshot(page, "01-wizard", "03-detection-summary");
	});

	// ----------------------------------------------------------------
	// 4. CLI selection checkboxes
	// ----------------------------------------------------------------
	test("CLI selection checkboxes", async ({ page }) => {
		await injectMock(page, S.CLI_SELECTION);
		await navigateAndWait(page, "/");

		// Selection message heading
		const message = page.locator("h3", {
			hasText: "Select which CLIs",
		});
		await expect(message).toBeVisible();

		// 4 checkboxes total
		const checkboxes = page.locator('input[type="checkbox"]');
		await expect(checkboxes).toHaveCount(4);

		// Installed CLIs (claude, codex) should be checked
		const claudeCheckbox = page.locator('input[type="checkbox"][value="claude"]');
		await expect(claudeCheckbox).toBeChecked();
		await expect(claudeCheckbox).toBeEnabled();

		const codexCheckbox = page.locator('input[type="checkbox"][value="codex"]');
		await expect(codexCheckbox).toBeChecked();
		await expect(codexCheckbox).toBeEnabled();

		// Not-installed CLIs (gemini, aider) should be disabled
		const geminiCheckbox = page.locator(
			'input[type="checkbox"][value="gemini"]',
		);
		await expect(geminiCheckbox).toBeDisabled();

		const aiderCheckbox = page.locator('input[type="checkbox"][value="aider"]');
		await expect(aiderCheckbox).toBeDisabled();

		// Continue button exists
		const continueBtn = page.getByText("Continue", { exact: true });
		await expect(continueBtn).toBeVisible();

		// Verify CLI descriptions render
		await expect(page.getByText("Anthropic Claude CLI")).toBeVisible();
		await expect(page.getByText("OpenAI Codex CLI")).toBeVisible();

		await catalogScreenshot(page, "01-wizard", "04-cli-selection");
	});

	// ----------------------------------------------------------------
	// 5. Form with all field types
	// ----------------------------------------------------------------
	test("form renders all field types", async ({ page }) => {
		await injectMock(page, S.FORM_FULL);
		await navigateAndWait(page, "/");

		// Form title
		const title = page.locator("h3", { hasText: "Configure Model Pool" });
		await expect(title).toBeVisible();

		// Form description
		const description = page.getByText(
			"Set up your preferred model configuration",
		);
		await expect(description).toBeVisible();

		// Text input with default value
		const poolNameInput = page.locator('input[name="pool_name"]');
		await expect(poolNameInput).toBeVisible();
		await expect(poolNameInput).toHaveAttribute("type", "text");
		await expect(poolNameInput).toHaveValue("primary");
		await expect(poolNameInput).toHaveAttribute(
			"placeholder",
			"e.g. primary, development",
		);

		// Select field
		const modelSelect = page.locator('select[name="model"]');
		await expect(modelSelect).toBeVisible();
		// Verify options exist
		const options = modelSelect.locator("option");
		// placeholder option + 3 model options = 4
		await expect(options).toHaveCount(4);

		// Password field
		const apiKeyInput = page.locator('input[name="api_key"]');
		await expect(apiKeyInput).toBeVisible();
		await expect(apiKeyInput).toHaveAttribute("type", "password");
		await expect(apiKeyInput).toHaveAttribute("placeholder", "sk-...");

		// Textarea field
		const notesTextarea = page.locator('textarea[name="notes"]');
		await expect(notesTextarea).toBeVisible();
		await expect(notesTextarea).toHaveAttribute(
			"placeholder",
			"Optional notes about this pool",
		);

		// Labels (use label selector to avoid matching helper text)
		await expect(page.locator("label").filter({ hasText: "Pool Name" }).first()).toBeVisible();
		await expect(page.locator("label").filter({ hasText: "API Key" }).first()).toBeVisible();
		await expect(page.locator("label").filter({ hasText: "Notes" }).first()).toBeVisible();

		// Submit button with custom label
		const submitBtn = page.getByText("Save Configuration", { exact: true });
		await expect(submitBtn).toBeVisible();

		// Help text renders
		await expect(
			page.getByText("A friendly name for this model pool"),
		).toBeVisible();
		await expect(
			page.getByText("Your API key will be stored securely"),
		).toBeVisible();

		await catalogScreenshot(page, "01-wizard", "05-form-full");
	});

	// ----------------------------------------------------------------
	// 6. Form with checkboxes and multi-select
	// ----------------------------------------------------------------
	test("form renders checkboxes and multi-select", async ({ page }) => {
		await injectMock(page, S.FORM_CHECKBOXES);
		await navigateAndWait(page, "/");

		// Title
		const title = page.locator("h3", { hasText: "Feature Settings" });
		await expect(title).toBeVisible();

		// Checkbox with default=true should be checked
		const autoRetry = page.locator('input[name="auto_retry"]');
		await expect(autoRetry).toBeVisible();
		await expect(autoRetry).toBeChecked();

		// Checkbox label
		await expect(
			page.getByText("Enable auto-retry on failure"),
		).toBeVisible();

		// Multi-select: 4 capability checkboxes
		const capabilityCheckboxes = page.locator(
			'input[name="capabilities"]',
		);
		await expect(capabilityCheckboxes).toHaveCount(4);

		// Multi-select option labels
		await expect(page.getByText("Code Generation")).toBeVisible();
		await expect(page.getByText("Code Review")).toBeVisible();
		await expect(page.getByText("Documentation")).toBeVisible();
		await expect(page.getByText("Testing")).toBeVisible();

		// Help text for multi_select
		await expect(
			page.getByText("Select which capabilities this pool supports"),
		).toBeVisible();

		// Submit button
		const saveBtn = page.getByText("Save", { exact: true });
		await expect(saveBtn).toBeVisible();

		await catalogScreenshot(page, "01-wizard", "06-form-checkboxes");
	});

	// ----------------------------------------------------------------
	// 7. Wizard stepper
	// ----------------------------------------------------------------
	test("wizard stepper shows steps", async ({ page }) => {
		await injectMock(page, S.WIZARD_STEPPER);
		await navigateAndWait(page, "/");

		// Step indicator bars (3 steps via Steps.Item triggers)
		const stepTriggers = page.locator("[data-scope='steps'] [data-part='trigger']");
		await expect(stepTriggers).toHaveCount(3);

		// Step labels visible (use exact match to avoid matching helper text)
		await expect(page.getByText("Authentication", { exact: true })).toBeVisible();
		await expect(page.getByText("Model", { exact: true })).toBeVisible();
		await expect(page.getByText("Confirm", { exact: true })).toBeVisible();

		// Current step form renders (step 0: "Authenticate with Claude")
		const formTitle = page.locator("h3", {
			hasText: "Authenticate with Claude",
		});
		await expect(formTitle).toBeVisible();

		// Form description for step 0
		await expect(
			page.getByText("Enter your API key or use OAuth to connect."),
		).toBeVisible();

		// Step 0 has a select and password field
		const authSelect = page.locator('select[name="auth_method"]');
		await expect(authSelect).toBeVisible();

		const keyInput = page.locator('input[name="key"]');
		await expect(keyInput).toBeVisible();
		await expect(keyInput).toHaveAttribute("type", "password");

		// Submit button shows "Next"
		const nextBtn = page.getByText("Next", { exact: true });
		await expect(nextBtn).toBeVisible();

		// Back button should NOT be visible on step 0
		const backBtn = page.getByText("Back", { exact: true });
		await expect(backBtn).not.toBeVisible();

		await catalogScreenshot(page, "01-wizard", "07-wizard-stepper");
	});

	// ----------------------------------------------------------------
	// 8. OAuth flow
	// ----------------------------------------------------------------
	test("OAuth flow renders instructions", async ({ page }) => {
		await injectMock(page, S.OAUTH_FLOW);
		await navigateAndWait(page, "/");

		// Provider name in heading
		const heading = page.locator("h3", {
			hasText: "Authentication Required: claude",
		});
		await expect(heading).toBeVisible();

		// Instructions text
		await expect(
			page.getByText("Open your terminal and run"),
		).toBeVisible();

		// Inline code element with the command
		const codeElement = page.locator("code", { hasText: "claude login" });
		await expect(codeElement).toBeVisible();

		// Action buttons
		const loggedInBtn = page.getByText("I've logged in", { exact: true });
		await expect(loggedInBtn).toBeVisible();

		const skipBtn = page.getByText("Skip", { exact: true });
		await expect(skipBtn).toBeVisible();

		await catalogScreenshot(page, "01-wizard", "08-oauth-flow");
	});

	// ----------------------------------------------------------------
	// 9. API key entry
	// ----------------------------------------------------------------
	test("API key entry renders", async ({ page }) => {
		await injectMock(page, S.API_KEY_ENTRY);
		await navigateAndWait(page, "/");

		// Provider in heading
		const heading = page.locator("h3", { hasText: "API Key: OpenAI" });
		await expect(heading).toBeVisible();

		// Env var reference
		await expect(
			page.getByText("Enter your API key for OPENAI_API_KEY"),
		).toBeVisible();

		// Password input
		const keyInput = page.locator('input[type="password"]');
		await expect(keyInput).toBeVisible();
		await expect(keyInput).toHaveAttribute("placeholder", "sk-...");

		// Help link
		const helpLink = page.getByText("Get API key");
		await expect(helpLink).toBeVisible();
		await expect(helpLink).toHaveAttribute(
			"href",
			"https://platform.openai.com/api-keys",
		);

		// Submit button
		const submitBtn = page.getByText("Submit", { exact: true });
		await expect(submitBtn).toBeVisible();

		await catalogScreenshot(page, "01-wizard", "09-api-key-entry");
	});

	// ----------------------------------------------------------------
	// 10. Confirm dialog
	// ----------------------------------------------------------------
	test("confirm dialog renders", async ({ page }) => {
		await injectMock(page, S.CONFIRM_DIALOG);
		await navigateAndWait(page, "/");

		// Title
		const title = page.locator("h3", { hasText: "Delete Model Pool?" });
		await expect(title).toBeVisible();

		// Message
		const message = page.getByText(
			"This will permanently remove the 'primary' pool",
		);
		await expect(message).toBeVisible();

		// Confirm button with custom label
		const confirmBtn = page.getByText("Delete Pool", { exact: true });
		await expect(confirmBtn).toBeVisible();

		// Cancel button with custom label
		const cancelBtn = page.getByText("Cancel", { exact: true });
		await expect(cancelBtn).toBeVisible();

		await catalogScreenshot(page, "01-wizard", "10-confirm-dialog");
	});

	// ----------------------------------------------------------------
	// 11. Setup complete
	// ----------------------------------------------------------------
	test("setup complete shows summary", async ({ page }) => {
		await injectMock(page, S.SETUP_COMPLETE);
		await navigateAndWait(page, "/");

		// Checkmark symbol (rendered as &#10003; which is the check mark)
		const checkmark = page.locator(".text-5xl");
		await expect(checkmark).toBeVisible();

		// "Setup Complete" heading
		const heading = page.getByText("Setup Complete");
		await expect(heading).toBeVisible();

		// Summary text
		const summary = page.getByText(
			"All CLIs configured successfully. 3 model pools created.",
		);
		await expect(summary).toBeVisible();

		// Configured items list
		await expect(page.getByText("claude (OAuth)")).toBeVisible();
		await expect(page.getByText("codex (API key)")).toBeVisible();
		await expect(page.getByText("claude-sonnet-4")).toBeVisible();
		await expect(page.getByText("gpt-4o")).toBeVisible();
		await expect(page.getByText("gemini-2.5-pro")).toBeVisible();

		// Items rendered as list items
		const items = page.locator("ul li");
		await expect(items).toHaveCount(5);

		// "View Pools" button
		const viewPoolsBtn = page.getByText("View Pools", { exact: true });
		await expect(viewPoolsBtn).toBeVisible();

		await catalogScreenshot(page, "01-wizard", "11-setup-complete");
	});

	// ----------------------------------------------------------------
	// 12. Error state with retry
	// ----------------------------------------------------------------
	test("error state shows retry", async ({ page }) => {
		await injectMock(page, S.ERROR_SETUP);
		await navigateAndWait(page, "/", 2000);

		// Error message in the error div (border-[#ef5350])
		const errorDiv = page.locator("div.border-\\[\\#ef5350\\]");
		await expect(errorDiv).toBeVisible();

		// Error text content
		await expect(
			page.getByText("Claude CLI crashed unexpectedly"),
		).toBeVisible();

		// Retry button
		const retryBtn = page.getByText("Retry Setup", { exact: true });
		await expect(retryBtn).toBeVisible();

		// Cancel button
		const cancelBtn = page.getByText("Cancel", { exact: true });
		await expect(cancelBtn).toBeVisible();

		await catalogScreenshot(page, "01-wizard", "12-error-retry");
	});

	// ----------------------------------------------------------------
	// 13. Stale session
	// ----------------------------------------------------------------
	test("stale session shows start fresh", async ({ page }) => {
		await injectMock(page, S.STALE_SESSION);
		await navigateAndWait(page, "/");

		// A confirm dialog should appear first (the "Yes" button from the
		// confirm action in the STALE_SESSION scenario)
		const yesBtn = page.getByText("Yes", { exact: true });
		await expect(yesBtn).toBeVisible();

		// Click "Yes" to trigger setup_respond, which will fail because
		// respondFails is true, causing the stale session state
		await yesBtn.click();
		await page.waitForTimeout(1000);

		// Stale session message
		const staleMsg = page.getByText(
			"The setup session is no longer active.",
		);
		await expect(staleMsg).toBeVisible();

		// Start Fresh button
		const freshBtn = page.getByText("Start Fresh", { exact: true });
		await expect(freshBtn).toBeVisible();

		// Cancel button in the stale session view
		const cancelBtn = page.getByText("Cancel", { exact: true });
		await expect(cancelBtn).toBeVisible();

		await catalogScreenshot(page, "01-wizard", "13-stale-session");
	});

	// ----------------------------------------------------------------
	// 14. Results display with multiple result types
	// ----------------------------------------------------------------
	test("results display renders multiple types", async ({ page }) => {
		await injectMock(page, S.RESULTS_DISPLAY);
		await navigateAndWait(page, "/", 2000);

		// Command output: the command line
		await expect(page.getByText("$ claude --version")).toBeVisible();

		// Command output: stdout content
		await expect(
			page.getByText("claude 1.2.3 (anthropic-cli)"),
		).toBeVisible();

		// Test result: success
		await expect(page.getByText("claude-sonnet-4: PASS")).toBeVisible();

		// Test result: failure
		await expect(page.getByText("gpt-4o: FAIL")).toBeVisible();

		// Config written: description
		await expect(
			page.getByText("Created model config for Claude Sonnet 4"),
		).toBeVisible();

		// Config written: path
		await expect(
			page.getByText(
				"~/.config/oulipoly-agent-runner/models/claude-sonnet.toml",
			),
		).toBeVisible();

		// There should be 4 result display containers
		const resultCards = page.locator(".rounded-lg.bg-\\[\\#16213e\\].p-4");
		await expect(resultCards).toHaveCount(4);

		await catalogScreenshot(page, "01-wizard", "14-results-display");
	});

	// ----------------------------------------------------------------
	// 15. Invocation tracking: check_setup_needed is called
	// ----------------------------------------------------------------
	test("check_setup_needed is invoked on mount", async ({ page }) => {
		await injectMock(page, S.FRESH_USER);
		await navigateAndWait(page, "/");

		const calls = await getCalls(page);
		const setupCheck = calls.find((c) => c.cmd === "check_setup_needed");
		expect(setupCheck).toBeTruthy();

		// start_setup should also be called when setup is needed
		const startSetup = calls.find((c) => c.cmd === "start_setup");
		expect(startSetup).toBeTruthy();

		// The channel argument should have been passed
		expect(startSetup?.args).toHaveProperty("onEvent");

		await catalogScreenshot(page, "01-wizard", "15-invocation-tracking");
	});
});
