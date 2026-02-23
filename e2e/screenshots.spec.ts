import fs from "node:fs";
import { expect, test } from "@playwright/test";
import { injectMock, navigateAndWait, catalogScreenshot } from "./fixtures/tauri-mock";
import type { MockConfig } from "./fixtures/tauri-mock";
import * as S from "./fixtures/scenarios";

// ---------------------------------------------------------------------------
// Catalog directory setup
// ---------------------------------------------------------------------------

const CATALOG_ROOT = "/tmp/oulipoly-e2e/catalog";
const CATEGORIES = [
	"01-wizard",
	"02-dashboard",
	"03-panels",
	"04-responsive",
	"05-states",
] as const;

test.beforeAll(() => {
	for (const category of CATEGORIES) {
		fs.mkdirSync(`${CATALOG_ROOT}/${category}`, { recursive: true });
	}
});

// ---------------------------------------------------------------------------
// 01-wizard: Setup wizard flow screenshots (13 tests)
// ---------------------------------------------------------------------------

test.describe("01-wizard", () => {
	test("01-status-detecting", async ({ page }) => {
		await injectMock(page, S.STATUS_DETECTING);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "01-wizard", "01-status-detecting");
	});

	test("02-progress-bar", async ({ page }) => {
		await injectMock(page, S.PROGRESS_BAR);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "01-wizard", "02-progress-bar");
	});

	test("03-detection-summary", async ({ page }) => {
		await injectMock(page, S.DETECTION_SUMMARY);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "01-wizard", "03-detection-summary");
	});

	test("04-cli-selection", async ({ page }) => {
		await injectMock(page, S.CLI_SELECTION);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "01-wizard", "04-cli-selection");
	});

	test("05-form-text-select", async ({ page }) => {
		await injectMock(page, S.FORM_FULL);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "01-wizard", "05-form-text-select");
	});

	test("06-form-checkboxes", async ({ page }) => {
		await injectMock(page, S.FORM_CHECKBOXES);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "01-wizard", "06-form-checkboxes");
	});

	test("07-wizard-stepper", async ({ page }) => {
		await injectMock(page, S.WIZARD_STEPPER);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "01-wizard", "07-wizard-stepper");
	});

	test("08-oauth-flow", async ({ page }) => {
		await injectMock(page, S.OAUTH_FLOW);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "01-wizard", "08-oauth-flow");
	});

	test("09-api-key-entry", async ({ page }) => {
		await injectMock(page, S.API_KEY_ENTRY);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "01-wizard", "09-api-key-entry");
	});

	test("10-confirm-dialog", async ({ page }) => {
		await injectMock(page, S.CONFIRM_DIALOG);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "01-wizard", "10-confirm-dialog");
	});

	test("11-setup-complete", async ({ page }) => {
		await injectMock(page, S.SETUP_COMPLETE);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "01-wizard", "11-setup-complete");
	});

	test("12-error-retry", async ({ page }) => {
		await injectMock(page, S.ERROR_SETUP);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "01-wizard", "12-error-retry");
	});

	test("13-stale-session", async ({ page }) => {
		await injectMock(page, S.STALE_SESSION);
		await navigateAndWait(page, "/");

		// Wait for the confirm dialog to appear, then click "Yes" to trigger
		// the stale session handler (respondFails: true causes setupRespond to reject)
		const yesBtn = page.getByRole("button", { name: "Yes" });
		await yesBtn.waitFor({ state: "visible", timeout: 5000 });
		await yesBtn.click();
		await page.waitForTimeout(1000);

		await catalogScreenshot(page, "01-wizard", "13-stale-session");
	});
});

// ---------------------------------------------------------------------------
// 02-dashboard: Pools view screenshots (7 tests)
// ---------------------------------------------------------------------------

test.describe("02-dashboard", () => {
	test("01-pools-populated", async ({ page }) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "02-dashboard", "01-pools-populated");
	});

	test("02-pools-empty", async ({ page }) => {
		await injectMock(page, S.EMPTY_POOLS);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "02-dashboard", "02-pools-empty");
	});

	test("03-add-pool-input", async ({ page }) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		// Click the "+" button to open the add-pool inline input
		await page.click('[title="Add provider pool"]');
		await page.waitForTimeout(500);

		await catalogScreenshot(page, "02-dashboard", "03-add-pool-input");
	});

	test("04-models-dropdown", async ({ page }) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		// Click the "3 Models" button on the first pool to open the Ark UI Popover
		const modelsBtn = page.locator("text=3 Models").first();
		await modelsBtn.click();
		await page.waitForTimeout(500);

		await catalogScreenshot(page, "02-dashboard", "04-models-dropdown");
	});

	test("05-tag-selected", async ({ page }) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		// Click a command chip to select it (adds ring-1 ring-accent)
		const commandChip = page.locator('span[role="option"]').first();
		await commandChip.click();
		await page.waitForTimeout(300);

		await catalogScreenshot(page, "02-dashboard", "05-tag-selected");
	});

	test("06-tag-editing", async ({ page }) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		// Double-click a command chip to enter inline edit mode
		const commandChip = page.locator('span[role="option"]').first();
		await commandChip.dblclick();
		await page.waitForTimeout(300);

		await catalogScreenshot(page, "02-dashboard", "06-tag-editing");
	});

	test("07-pool-settings", async ({ page }) => {
		await injectMock(page, S.POOL_WITH_FLAGS);
		await navigateAndWait(page, "/");

		// Click the gear icon to open PoolSettingsPanel dialog
		await page.click('[title="Pool settings"]');
		await page.waitForTimeout(500);

		await catalogScreenshot(page, "02-dashboard", "07-pool-settings");
	});
});

// ---------------------------------------------------------------------------
// 03-panels: Model panel and pool settings screenshots (5 tests)
// ---------------------------------------------------------------------------

test.describe("03-panels", () => {
	test("01-model-panel-edit", async ({ page }) => {
		await injectMock(page, S.MODEL_EDIT);
		await navigateAndWait(page, "/");

		// Open models dropdown on first pool
		const modelsBtn = page.locator("text=3 Models").first();
		await modelsBtn.click();
		await page.waitForTimeout(500);

		// Click a facet chip to open edit panel
		const popover = page.locator('[data-scope="popover"][data-part="content"][data-state="open"]');
		await popover.getByText("high").first().click();
		await page.waitForTimeout(500);

		await catalogScreenshot(page, "03-panels", "01-model-panel-edit");
	});

	test("02-model-panel-add", async ({ page }) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		// Open models dropdown on first pool
		const modelsBtn = page.locator("text=3 Models").first();
		await modelsBtn.click();
		await page.waitForTimeout(500);

		// Click the "New" button to open ModelPanel in add mode (first() because each pool has one)
		const newBtn = page.locator('[title="Add standalone model"]').first();
		await newBtn.click();
		await page.waitForTimeout(500);

		await catalogScreenshot(page, "03-panels", "02-model-panel-add");
	});

	test("03-model-test-success", async ({ page }) => {
		await injectMock(page, S.MODEL_EDIT);
		await navigateAndWait(page, "/");

		// Open models dropdown -> click facet -> open edit panel
		const modelsBtn = page.locator("text=3 Models").first();
		await modelsBtn.click();
		await page.waitForTimeout(500);
		const popover3 = page.locator('[data-scope="popover"][data-part="content"][data-state="open"]');
		await popover3.getByText("high").first().click();
		await page.waitForTimeout(500);

		// Click "Save & Test" to trigger test (MODEL_EDIT has success result)
		await page.click("text=Save & Test");
		await page.waitForTimeout(2000);

		await catalogScreenshot(page, "03-panels", "03-model-test-success");
	});

	test("04-model-test-failure", async ({ page }) => {
		await injectMock(page, S.MODEL_TEST_FAILURE);
		await navigateAndWait(page, "/");

		// Open models dropdown -> click facet -> open edit panel
		const modelsBtn = page.locator("text=3 Models").first();
		await modelsBtn.click();
		await page.waitForTimeout(500);
		const popover4 = page.locator('[data-scope="popover"][data-part="content"][data-state="open"]');
		await popover4.getByText("high").first().click();
		await page.waitForTimeout(500);

		// Click "Save & Test" to trigger test (MODEL_TEST_FAILURE has failure result)
		await page.click("text=Save & Test");
		await page.waitForTimeout(2000);

		await catalogScreenshot(page, "03-panels", "04-model-test-failure");
	});

	test("05-pool-settings-flags", async ({ page }) => {
		await injectMock(page, S.POOL_WITH_FLAGS);
		await navigateAndWait(page, "/");

		// Click the gear icon to open PoolSettingsPanel with flag toggles
		await page.click('[title="Pool settings"]');
		await page.waitForTimeout(500);

		await catalogScreenshot(page, "03-panels", "05-pool-settings-flags");
	});
});

// ---------------------------------------------------------------------------
// 04-responsive: Viewport size variations (3 tests)
// ---------------------------------------------------------------------------

test.describe("04-responsive", () => {
	test("01-narrow-480", async ({ page }) => {
		await page.setViewportSize({ width: 480, height: 700 });
		await injectMock(page, S.FORM_FULL);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "04-responsive", "01-narrow-480");
	});

	test("02-default-900", async ({ page }) => {
		await page.setViewportSize({ width: 900, height: 700 });
		await injectMock(page, S.FORM_FULL);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "04-responsive", "02-default-900");
	});

	test("03-wide-1400", async ({ page }) => {
		await page.setViewportSize({ width: 1400, height: 900 });
		await injectMock(page, S.FORM_FULL);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "04-responsive", "03-wide-1400");
	});
});

// ---------------------------------------------------------------------------
// 05-states: Loading, error, and empty states (3 tests)
// ---------------------------------------------------------------------------

test.describe("05-states", () => {
	test("01-loading", async ({ page }) => {
		// Inject a mock where list_pools hangs (never resolves) to capture loading state
		const loadingConfig: MockConfig = {
			checkSetup: false,
			pools: [],
			models: [],
		};
		await page.addInitScript(`
			(() => {
				window.__TAURI_INTERNALS__ = {
					transformCallback: (cb, once) => {
						const id = Math.floor(Math.random() * 100000);
						const prop = '_' + id;
						Object.defineProperty(window, prop, {
							value: (result) => {
								if (once) { delete window[prop]; }
								return cb(result);
							},
							writable: true,
							configurable: true,
						});
						return id;
					},
					unregisterCallback: (id) => { delete window['_' + id]; },
					invoke: (cmd, args) => {
						if (cmd === 'plugin:event|listen') return Promise.resolve(0);
						if (cmd === 'plugin:event|unlisten') return Promise.resolve();
						if (cmd === 'check_setup_needed') return Promise.resolve(false);
						if (cmd === 'list_pools') return new Promise(() => {});
						if (cmd === 'list_models') return new Promise(() => {});
						return Promise.resolve();
					},
					metadata: {
						currentWindow: { label: 'main' },
						currentWebview: { label: 'main' },
					},
					convertFileSrc: (src) => src,
				};
			})();
		`);
		await page.goto("/", { waitUntil: "networkidle" });
		await page.waitForTimeout(500);

		await catalogScreenshot(page, "05-states", "01-loading");
	});

	test("02-error", async ({ page }) => {
		// Inject a mock where list_pools rejects to show the error state
		await page.addInitScript(`
			(() => {
				window.__TAURI_INTERNALS__ = {
					transformCallback: (cb, once) => {
						const id = Math.floor(Math.random() * 100000);
						const prop = '_' + id;
						Object.defineProperty(window, prop, {
							value: (result) => {
								if (once) { delete window[prop]; }
								return cb(result);
							},
							writable: true,
							configurable: true,
						});
						return id;
					},
					unregisterCallback: (id) => { delete window['_' + id]; },
					invoke: (cmd, args) => {
						if (cmd === 'plugin:event|listen') return Promise.resolve(0);
						if (cmd === 'plugin:event|unlisten') return Promise.resolve();
						if (cmd === 'check_setup_needed') return Promise.resolve(false);
						if (cmd === 'list_pools') return Promise.reject('Connection refused: backend not running');
						if (cmd === 'list_models') return Promise.reject('Connection refused');
						return Promise.resolve();
					},
					metadata: {
						currentWindow: { label: 'main' },
						currentWebview: { label: 'main' },
					},
					convertFileSrc: (src) => src,
				};
			})();
		`);
		await page.goto("/", { waitUntil: "networkidle" });
		await page.waitForTimeout(1000);

		await catalogScreenshot(page, "05-states", "02-error");
	});

	test("03-empty", async ({ page }) => {
		await injectMock(page, S.EMPTY_POOLS);
		await navigateAndWait(page, "/");
		await catalogScreenshot(page, "05-states", "03-empty");
	});
});
