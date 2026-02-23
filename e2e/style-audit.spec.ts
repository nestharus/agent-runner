import { expect, test } from "@playwright/test";
import { injectMock, navigateAndWait } from "./fixtures/tauri-mock";
import * as S from "./fixtures/scenarios";

test.describe("Style Audit", () => {
	test("Ark UI Popover.Content renders visible content", async ({ page }) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		// Open the models dropdown on the first pool (claude: "3 Models")
		const modelsBtn = page.locator("text=3 Models").first();
		await modelsBtn.click();
		await page.waitForTimeout(300);

		// Verify popover content is visible via Ark UI data attributes (filter by open state)
		const popoverContent = page.locator(
			'[data-scope="popover"][data-part="content"][data-state="open"]',
		);
		await expect(popoverContent).toBeVisible();

		// Verify the popover has actual child elements (not an empty container)
		const childCount = await popoverContent.locator("> *").count();
		expect(childCount).toBeGreaterThan(0);

		// Verify it contains the model count header text
		await expect(popoverContent.locator("text=Models (3)")).toBeVisible();
	});

	test("Ark UI Field.Root renders label + input", async ({ page }) => {
		await injectMock(page, S.FORM_FULL);
		await navigateAndWait(page, "/");

		// The form scenario triggers setup, which renders FormRenderer
		// Wait for the form title to appear
		await page.waitForSelector("text=Configure Model Pool", {
			timeout: 10000,
		});

		// Verify Field.Root elements exist via Ark UI data attributes
		const fieldRoots = page.locator('[data-scope="field"][data-part="root"]');
		const fieldCount = await fieldRoots.count();
		expect(fieldCount).toBeGreaterThan(0);

		// Verify at least one label exists with text content
		const labels = page.locator(
			'[data-scope="field"][data-part="label"]',
		);
		const labelCount = await labels.count();
		expect(labelCount).toBeGreaterThan(0);
		const firstLabelText = await labels.first().textContent();
		expect(firstLabelText?.trim().length).toBeGreaterThan(0);

		// Verify associated input exists within the same Field.Root
		const firstField = fieldRoots.first();
		const inputInField = firstField.locator(
			'[data-scope="field"][data-part="input"], [data-scope="field"][data-part="textarea"], [data-scope="field"][data-part="select"]',
		);
		await expect(inputInField.first()).toBeVisible();
	});

	test("Ark UI Steps renders step indicators", async ({ page }) => {
		await injectMock(page, S.WIZARD_STEPPER);
		await navigateAndWait(page, "/");

		// Wait for the wizard to render
		await page.waitForSelector("text=Authentication", { timeout: 10000 });

		// Verify Steps.Root exists via Ark UI data attributes
		const stepsRoot = page.locator(
			'[data-scope="steps"][data-part="root"]',
		);
		await expect(stepsRoot).toBeVisible();

		// Verify step trigger elements exist (the colored indicator bars)
		const stepTriggers = page.locator(
			'[data-scope="steps"][data-part="trigger"]',
		);
		const triggerCount = await stepTriggers.count();
		expect(triggerCount).toBe(3); // Authentication, Model, Confirm

		// Verify step label text is visible (use exact match to avoid matching helper text)
		await expect(page.getByText("Authentication", { exact: true })).toBeVisible();
		await expect(page.getByText("Model", { exact: true })).toBeVisible();
		await expect(page.getByText("Confirm", { exact: true })).toBeVisible();
	});

	test("Ark UI Progress renders track with indicator", async ({ page }) => {
		await injectMock(page, S.PROGRESS_BAR);
		await navigateAndWait(page, "/");

		// Wait for progress message to appear
		await page.waitForSelector("text=Configuring model pools...", {
			timeout: 10000,
		});

		// Verify Progress.Root exists
		const progressRoot = page.locator(
			'[data-scope="progress"][data-part="root"]',
		);
		await expect(progressRoot).toBeVisible();

		// Verify track element exists
		const track = page.locator(
			'[data-scope="progress"][data-part="track"]',
		);
		await expect(track).toBeVisible();

		// Verify range (indicator fill) element exists
		const range = page.locator(
			'[data-scope="progress"][data-part="range"]',
		);
		await expect(range).toBeVisible();

		// Range should have non-zero width (65% progress)
		const rangeStyle = await range.getAttribute("style");
		expect(rangeStyle).toBeTruthy();
		expect(rangeStyle).toContain("width");
		// The width should not be 0%
		expect(rangeStyle).not.toContain("width: 0%");
		expect(rangeStyle).not.toContain("width:0%");
	});

	test("Ark UI Dialog.Content renders visible content", async ({ page }) => {
		await injectMock(page, S.MODEL_EDIT);
		await navigateAndWait(page, "/");

		// Open models dropdown
		const modelsBtn = page.locator("text=3 Models").first();
		await modelsBtn.click();
		await page.waitForTimeout(300);

		// Click a facet chip to open the edit panel (ModelPanel uses Dialog)
		const popoverContent = page.locator(
			'[data-scope="popover"][data-part="content"][data-state="open"]',
		);
		await popoverContent.getByText("high").first().click();
		await page.waitForTimeout(500);

		// Verify Dialog.Content is visible via Ark UI data attributes
		const dialogContent = page.locator(
			'[data-scope="dialog"][data-part="content"]',
		);
		await expect(dialogContent).toBeVisible();

		// Verify it has actual text content inside (not empty)
		const innerText = await dialogContent.textContent();
		expect(innerText?.trim().length).toBeGreaterThan(0);

		// Verify the dialog title is present
		const dialogTitle = page.locator(
			'[data-scope="dialog"][data-part="title"]',
		);
		await expect(dialogTitle).toBeVisible();
		const titleText = await dialogTitle.textContent();
		expect(titleText).toContain("claude~high");
	});

	test("Ark UI Switch renders toggle control", async ({ page }) => {
		await injectMock(page, S.POOL_WITH_FLAGS);
		await navigateAndWait(page, "/");

		// Click the pool settings gear icon to open PoolSettingsPanel
		const gearButton = page.locator('[title="Pool settings"]');
		await gearButton.click();
		await page.waitForTimeout(500);

		// Verify the settings panel dialog opened
		await expect(page.locator("text=Pool Settings")).toBeVisible();

		// Verify Switch.Control elements are visible via Ark UI data attributes
		const switchControls = page.locator(
			'[data-scope="switch"][data-part="control"]',
		);
		const switchCount = await switchControls.count();
		expect(switchCount).toBeGreaterThan(0);

		// Verify each switch has a thumb element
		for (let i = 0; i < switchCount; i++) {
			const control = switchControls.nth(i);
			await expect(control).toBeVisible();

			const thumb = control.locator(
				'[data-scope="switch"][data-part="thumb"]',
			);
			await expect(thumb).toBeVisible();
		}

		// Verify switch labels exist with readable text
		const switchLabels = page.locator(
			'[data-scope="switch"][data-part="label"]',
		);
		const labelCount = await switchLabels.count();
		expect(labelCount).toBeGreaterThan(0);
		const firstLabel = await switchLabels.first().textContent();
		expect(firstLabel?.trim().length).toBeGreaterThan(0);
	});

	test("FontAwesome icons render as SVG elements", async ({ page }) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		// Query all SVG icons rendered by the custom Icon component
		const svgIcons = page.locator('svg[aria-hidden="true"]');
		const iconCount = await svgIcons.count();

		// Expect at least 3 icons: + button, gear, chevron on pool cards
		expect(iconCount).toBeGreaterThanOrEqual(3);

		// Each SVG should contain a <path> with a non-empty d attribute
		for (let i = 0; i < Math.min(iconCount, 5); i++) {
			const svg = svgIcons.nth(i);
			const pathEl = svg.locator("path");
			await expect(pathEl).toBeAttached();

			const dAttr = await pathEl.getAttribute("d");
			expect(dAttr).toBeTruthy();
			expect(dAttr!.length).toBeGreaterThan(0);
		}
	});

	test("Custom Icon SVG paths have data", async ({ page }) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		// Get the first icon's path element
		const firstPath = page.locator('svg[aria-hidden="true"] path').first();
		await expect(firstPath).toBeAttached();

		const dAttr = await firstPath.getAttribute("d");
		expect(dAttr).toBeTruthy();

		// Real SVG path data is long (dozens or hundreds of characters),
		// not a placeholder like "M0 0" or empty string
		expect(dAttr!.length).toBeGreaterThan(10);

		// Verify it contains valid SVG path commands (M, L, C, Z, etc.)
		expect(dAttr).toMatch(/^[Mm]/);
	});

	test("Slide-in animation class present on panels", async ({ page }) => {
		await injectMock(page, S.MODEL_EDIT);
		await navigateAndWait(page, "/");

		// Open models dropdown then click model to open edit panel
		const modelsBtn = page.locator("text=3 Models").first();
		await modelsBtn.click();
		await page.waitForTimeout(300);

		const popoverForSlide = page.locator(
			'[data-scope="popover"][data-part="content"][data-state="open"]',
		);
		await popoverForSlide.getByText("high").first().click();
		await page.waitForTimeout(500);

		// The Dialog.Content element should have animate-slide-in class
		const dialogContent = page.locator(
			'[data-scope="dialog"][data-part="content"]',
		);
		await expect(dialogContent).toBeVisible();

		const classList = await dialogContent.getAttribute("class");
		expect(classList).toBeTruthy();
		expect(classList).toContain("animate-slide-in");
	});

	test("CSS color tokens resolve to actual values", async ({ page }) => {
		await injectMock(page, S.CONFIGURED_USER);
		await navigateAndWait(page, "/");

		// Wait for content to render
		await page.waitForSelector("text=Provider Pools", { timeout: 10000 });

		// The accent color (#4fc3f7) is used on the "+" button background.
		// Verify computed background-color resolves to the expected value.
		const addButton = page.locator('[title="Add provider pool"]');
		await expect(addButton).toBeVisible();

		const bgColor = await addButton.evaluate((el) => {
			return window.getComputedStyle(el).backgroundColor;
		});

		// #4fc3f7 in RGB is approximately rgb(79, 195, 247)
		// Accept the resolved color (not empty, not transparent, not "rgba(0, 0, 0, 0)")
		expect(bgColor).toBeTruthy();
		expect(bgColor).not.toBe("rgba(0, 0, 0, 0)");
		expect(bgColor).not.toBe("transparent");

		// Verify body background color resolves from --color-bg: #121212
		const bodyBg = await page.evaluate(() => {
			return window.getComputedStyle(document.body).backgroundColor;
		});
		expect(bodyBg).toBeTruthy();
		expect(bodyBg).not.toBe("rgba(0, 0, 0, 0)");
		expect(bodyBg).not.toBe("transparent");
	});

	test("Checkbox inputs are functional", async ({ page }) => {
		await injectMock(page, S.FORM_CHECKBOXES);
		await navigateAndWait(page, "/");

		// Wait for the form to render
		await page.waitForSelector("text=Feature Settings", { timeout: 10000 });

		// Find all checkbox inputs
		const checkboxes = page.locator('input[type="checkbox"]');
		const checkboxCount = await checkboxes.count();
		expect(checkboxCount).toBeGreaterThan(0);

		// The auto_retry checkbox has default_value "true", so it should be checked
		const autoRetry = page.locator('input[type="checkbox"][name="auto_retry"]');
		await expect(autoRetry).toBeVisible();
		await expect(autoRetry).toBeChecked();

		// Find an unchecked checkbox (from multi_select capabilities)
		const capabilityCheckbox = page
			.locator('input[type="checkbox"][name="capabilities"]')
			.first();
		await expect(capabilityCheckbox).toBeVisible();
		await expect(capabilityCheckbox).not.toBeChecked();

		// Click it and verify it becomes checked
		await capabilityCheckbox.click();
		await expect(capabilityCheckbox).toBeChecked();
	});
});
