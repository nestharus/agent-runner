/**
 * Visual QA tests for the setup agent UI.
 *
 * Serves the UI files, injects Tauri API mocks, and uses Playwright
 * to interact with every component, take screenshots, and verify behavior.
 *
 * Run: PLAYWRIGHT_BROWSERS_PATH=/tmp/pw-browsers node e2e/qa-visual.mjs
 */

import { chromium } from "@playwright/test";
import { createServer } from "http";
import { readFileSync, existsSync, mkdirSync } from "fs";
import { join, dirname, extname } from "path";
import { fileURLToPath } from "url";

var __dirname = dirname(fileURLToPath(import.meta.url));
var projectRoot = join(__dirname, "..");
var uiDir = join(projectRoot, "ui");
var screenshotDir = join(__dirname, "screenshots");

if (!existsSync(screenshotDir)) mkdirSync(screenshotDir, { recursive: true });

// ---------------------------------------------------------------------------
// Static file server for the UI
// ---------------------------------------------------------------------------

var mimeTypes = {
    ".html": "text/html",
    ".js": "application/javascript",
    ".css": "text/css",
    ".png": "image/png",
};

function startServer() {
    return new Promise(function (resolve) {
        var server = createServer(function (req, res) {
            var path = req.url === "/" ? "/index.html" : req.url;
            var filePath = join(uiDir, path);
            if (!existsSync(filePath)) {
                res.writeHead(404);
                res.end("Not found");
                return;
            }
            var ext = extname(filePath);
            res.writeHead(200, { "Content-Type": mimeTypes[ext] || "text/plain" });
            res.end(readFileSync(filePath));
        });
        server.listen(0, "127.0.0.1", function () {
            var port = server.address().port;
            resolve({ server: server, url: "http://127.0.0.1:" + port });
        });
    });
}

// ---------------------------------------------------------------------------
// Tauri mock injection script
// ---------------------------------------------------------------------------

function getTauriMockScript(scenario) {
    return `
        window.__scenario = ${JSON.stringify(scenario)};
        window.__invokeCalls = [];
        window.__channelCallback = null;
        window.__respondHandler = null;

        window.__TAURI__ = {
            core: {
                invoke: function(cmd, args) {
                    window.__invokeCalls.push({ cmd: cmd, args: args, time: Date.now() });
                    var scenario = window.__scenario;

                    if (cmd === "list_models") {
                        return Promise.resolve(scenario.models || []);
                    }
                    if (cmd === "check_setup_needed") {
                        return Promise.resolve(scenario.setupNeeded !== false);
                    }
                    if (cmd === "start_setup") {
                        // Capture the channel callback
                        if (args && args.onEvent) {
                            window.__channelCallback = args.onEvent.onmessage;
                        }
                        return Promise.resolve("session-qa-" + Date.now());
                    }
                    if (cmd === "setup_respond") {
                        if (window.__respondHandler) {
                            window.__respondHandler(args.response);
                        }
                        return Promise.resolve();
                    }
                    if (cmd === "cancel_setup") {
                        return Promise.resolve();
                    }
                    if (cmd === "detect_clis") {
                        return Promise.resolve(scenario.detection || {});
                    }
                    return Promise.resolve();
                },
                Channel: function() {
                    var self = this;
                    self.onmessage = null;
                    // Store reference so inject script can send events
                    window.__latestChannel = self;
                }
            }
        };

        // Helper to send events from test code
        window.__sendEvent = function(event) {
            if (window.__latestChannel && window.__latestChannel.onmessage) {
                window.__latestChannel.onmessage(event);
            }
        };

        // Helper to set respond handler
        window.__onRespond = function(handler) {
            window.__respondHandler = handler;
        };
    `;
}

// ---------------------------------------------------------------------------
// Test runner
// ---------------------------------------------------------------------------

var passed = 0;
var failed = 0;
var screenshots = [];

async function screenshot(page, name) {
    var path = join(screenshotDir, name + ".png");
    await page.screenshot({ path: path, fullPage: true });
    screenshots.push(name);
    return path;
}

async function run() {
    var { server, url } = await startServer();
    console.log("Server running at " + url);

    var browser = await chromium.launch({ headless: true });

    try {
        // =====================================================================
        // SCENARIO 1: Fresh setup — no models configured
        // =====================================================================
        console.log("\n=== Scenario 1: Fresh Setup (auto-redirect to setup) ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 900, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({
                setupNeeded: true,
                models: [],
            }));

            await page.goto(url);
            await page.waitForTimeout(500);

            // Should auto-switch to setup view
            var setupView = await page.locator("#setup-view");
            var isVisible = await setupView.isVisible();
            check("auto-switches to setup view when needed", isVisible);
            await screenshot(page, "01-fresh-setup-auto-switch");

            // Nav should show Setup as active
            var activeNav = await page.locator(".nav-btn.active").textContent();
            check("setup nav button is active", activeNav.trim() === "Setup");

            await context.close();
        })();

        // =====================================================================
        // SCENARIO 2: Detection summary display
        // =====================================================================
        console.log("\n=== Scenario 2: Detection Summary ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 900, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({
                setupNeeded: true,
                models: [],
            }));

            await page.goto(url);
            await page.waitForTimeout(300);

            // Send status then detection summary
            await page.evaluate(function () {
                window.__sendEvent({ event: "status", data: { message: "Detecting installed CLIs..." } });
            });
            await page.waitForTimeout(200);
            await screenshot(page, "02a-detecting-status");

            await page.evaluate(function () {
                window.__sendEvent({
                    event: "show_result",
                    data: {
                        content: {
                            type: "detection_summary",
                            clis: [
                                { name: "claude", installed: true, version: "1.0.33", authenticated: true, wrapper_count: 2 },
                                { name: "codex", installed: true, version: "0.9.1", authenticated: true, wrapper_count: 0 },
                                { name: "opencode", installed: false, version: null, authenticated: false, wrapper_count: 0 },
                                { name: "gemini", installed: false, version: null, authenticated: false, wrapper_count: 0 },
                            ],
                        },
                    },
                });
            });
            await page.waitForTimeout(200);
            await screenshot(page, "02b-detection-summary");

            // Check table exists and has correct data
            var tableRows = await page.locator(".detection-table tbody tr").count();
            check("detection table has 4 rows", tableRows === 4);

            var installedRows = await page.locator(".detection-table tbody tr.installed").count();
            check("2 CLIs marked as installed", installedRows === 2);

            await context.close();
        })();

        // =====================================================================
        // SCENARIO 3: Form rendering
        // =====================================================================
        console.log("\n=== Scenario 3: Form Rendering ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 900, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({ setupNeeded: true, models: [] }));
            await page.goto(url);
            await page.waitForTimeout(300);

            // Send a form action
            await page.evaluate(function () {
                window.__sendEvent({
                    event: "need_input",
                    data: {
                        action: {
                            type: "form",
                            title: "Configure OpenAI Provider",
                            description: "Enter your OpenAI API credentials to enable GPT models.",
                            form_id: "openai-config",
                            submit_label: "Save Configuration",
                            fields: [
                                { name: "api_key", label: "API Key", field_type: "password", required: true, default_value: null, options: null, placeholder: "sk-...", help_text: "Your OpenAI API key from platform.openai.com" },
                                { name: "org_id", label: "Organization ID", field_type: "text", required: false, default_value: null, options: null, placeholder: "org-...", help_text: "Optional. Only needed for organization accounts." },
                                { name: "model", label: "Default Model", field_type: "select", required: true, default_value: "gpt-4o", options: [{ label: "GPT-4o", value: "gpt-4o" }, { label: "GPT-4o Mini", value: "gpt-4o-mini" }, { label: "o1", value: "o1" }, { label: "o3", value: "o3" }], placeholder: "Select a model...", help_text: null },
                                { name: "stream", label: "Enable Streaming", field_type: "checkbox", required: false, default_value: "true", options: null, placeholder: null, help_text: null },
                                { name: "capabilities", label: "Capabilities", field_type: "multi_select", required: false, default_value: null, options: [{ label: "Code Generation", value: "code" }, { label: "File Reading", value: "files" }, { label: "Web Search", value: "web" }], placeholder: null, help_text: "Select which capabilities to enable" },
                            ],
                        },
                    },
                });
            });
            await page.waitForTimeout(300);
            await screenshot(page, "03a-form-all-field-types");

            // Check all fields rendered
            var fields = await page.locator(".form-field").count();
            check("form has 5 fields", fields === 5);

            // Check password field has correct type
            var apiKeyType = await page.locator('input[name="api_key"]').getAttribute("type");
            check("API key is password field", apiKeyType === "password");

            // Check select has options
            var selectOptions = await page.locator('select[name="model"] option').count();
            check("model select has options", selectOptions >= 4);

            // Check checkbox is pre-checked (default_value: "true")
            var checked = await page.locator('input[name="stream"]').isChecked();
            check("streaming checkbox pre-checked", checked);

            // Fill in form
            await page.locator('input[name="api_key"]').fill("sk-test-key-12345");
            await page.locator('input[name="org_id"]').fill("org-myorg");
            await screenshot(page, "03b-form-filled");

            // Try to submit without required field cleared
            // (model already has default so it should submit)

            // Set up respond handler to capture submission
            var submitted = false;
            await page.evaluate(function () {
                window.__onRespond(function (response) {
                    window.__lastResponse = response;
                });
            });

            await page.locator(".submit-btn").click();
            await page.waitForTimeout(200);

            var response = await page.evaluate(function () { return window.__lastResponse; });
            check("form submitted with correct values", response && response.type === "form_submit");
            check("form_id matches", response && response.form_id === "openai-config");

            await screenshot(page, "03c-after-form-submit");

            await context.close();
        })();

        // =====================================================================
        // SCENARIO 4: Confirm dialog
        // =====================================================================
        console.log("\n=== Scenario 4: Confirm Dialog ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 900, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({ setupNeeded: true, models: [] }));
            await page.goto(url);
            await page.waitForTimeout(300);

            await page.evaluate(function () {
                window.__sendEvent({
                    event: "need_input",
                    data: {
                        action: {
                            type: "confirm",
                            title: "Overwrite Existing Configuration?",
                            message: "A model configuration for 'claude-sonnet' already exists. Do you want to replace it with the new settings?",
                            confirm_id: "overwrite-model",
                            confirm_label: "Replace",
                            cancel_label: "Keep Existing",
                        },
                    },
                });
            });
            await page.waitForTimeout(200);
            await screenshot(page, "04-confirm-dialog");

            // Check dialog structure
            var title = await page.locator(".confirm-dialog h3").textContent();
            check("confirm title rendered", title.includes("Overwrite"));

            var confirmBtn = await page.locator(".confirm-btn").textContent();
            check("confirm button has custom label", confirmBtn.includes("Replace"));

            var cancelBtn = await page.locator(".cancel-btn").textContent();
            check("cancel button has custom label", cancelBtn.includes("Keep"));

            await context.close();
        })();

        // =====================================================================
        // SCENARIO 5: OAuth flow
        // =====================================================================
        console.log("\n=== Scenario 5: OAuth Flow ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 900, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({ setupNeeded: true, models: [] }));
            await page.goto(url);
            await page.waitForTimeout(300);

            await page.evaluate(function () {
                window.__sendEvent({
                    event: "need_input",
                    data: {
                        action: {
                            type: "oauth_flow",
                            provider: "claude",
                            login_command: "claude login",
                            instructions: "To authenticate with Claude:\n\n1. Run: claude login\n2. A browser window will open\n3. Sign in with your Anthropic account\n4. Return here and click 'I've logged in'\n\nNote: You need an active Anthropic account. Visit anthropic.com to create one.",
                        },
                    },
                });
            });
            await page.waitForTimeout(200);
            await screenshot(page, "05-oauth-flow");

            // Check instructions visible
            var instructions = await page.locator(".instructions").textContent();
            check("OAuth instructions rendered", instructions.includes("claude login"));

            // Check buttons
            var doneBtn = await page.locator(".done-btn");
            check("'I've logged in' button exists", await doneBtn.isVisible());

            var skipBtn = await page.locator(".skip-btn");
            check("Skip button exists", await skipBtn.isVisible());

            await context.close();
        })();

        // =====================================================================
        // SCENARIO 6: API Key Entry
        // =====================================================================
        console.log("\n=== Scenario 6: API Key Entry ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 900, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({ setupNeeded: true, models: [] }));
            await page.goto(url);
            await page.waitForTimeout(300);

            await page.evaluate(function () {
                window.__sendEvent({
                    event: "need_input",
                    data: {
                        action: {
                            type: "api_key_entry",
                            provider: "Z AI",
                            env_var: "ZAI_API_KEY",
                            help_url: "https://zai.example.com/api-keys",
                        },
                    },
                });
            });
            await page.waitForTimeout(200);
            await screenshot(page, "06-api-key-entry");

            // Check structure
            var title = await page.locator(".api-key-entry h3").textContent();
            check("API key title shows provider", title.includes("Z AI"));

            var helpLink = await page.locator(".help-link");
            check("Help link exists", await helpLink.isVisible());

            var inputType = await page.locator("input[type='password']").getAttribute("type");
            check("Key input is password type", inputType === "password");

            await context.close();
        })();

        // =====================================================================
        // SCENARIO 7: CLI Selection
        // =====================================================================
        console.log("\n=== Scenario 7: CLI Selection ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 900, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({ setupNeeded: true, models: [] }));
            await page.goto(url);
            await page.waitForTimeout(300);

            await page.evaluate(function () {
                window.__sendEvent({
                    event: "need_input",
                    data: {
                        action: {
                            type: "cli_selection",
                            message: "Select which CLIs to configure",
                            available: [
                                { name: "claude", installed: true, description: "Anthropic Claude Code CLI" },
                                { name: "codex", installed: true, description: "OpenAI Codex CLI" },
                                { name: "opencode", installed: false, description: "Open-source coding assistant" },
                                { name: "gemini", installed: false, description: "Google Gemini CLI" },
                                { name: "puppy", installed: true, description: "Puppy AI CLI (community)" },
                            ],
                        },
                    },
                });
            });
            await page.waitForTimeout(200);
            await screenshot(page, "07-cli-selection");

            var checkboxes = await page.locator('.cli-selection input[type="checkbox"]').count();
            check("shows 5 CLI checkboxes", checkboxes === 5);

            // Disabled checkboxes for uninstalled
            var disabledCount = await page.locator('.cli-selection input[type="checkbox"]:disabled').count();
            check("uninstalled CLIs are disabled", disabledCount === 2);

            await context.close();
        })();

        // =====================================================================
        // SCENARIO 8: Wizard (multi-step)
        // =====================================================================
        console.log("\n=== Scenario 8: Wizard Multi-Step ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 900, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({ setupNeeded: true, models: [] }));
            await page.goto(url);
            await page.waitForTimeout(300);

            await page.evaluate(function () {
                window.__sendEvent({
                    event: "need_input",
                    data: {
                        action: {
                            type: "wizard",
                            title: "New Provider Setup",
                            wizard_id: "new-provider",
                            current_step: 0,
                            steps: [
                                {
                                    label: "Provider",
                                    description: null,
                                    form: {
                                        title: "Choose Provider",
                                        description: "Select the AI provider you want to configure.",
                                        fields: [
                                            { name: "provider", label: "Provider", field_type: "select", required: true, default_value: null, options: [{ label: "OpenAI", value: "openai" }, { label: "Anthropic", value: "anthropic" }, { label: "Google", value: "google" }, { label: "Z AI", value: "zai" }], placeholder: "Choose a provider...", help_text: null },
                                        ],
                                        form_id: "wiz-step-0",
                                        submit_label: "Next",
                                    },
                                },
                                {
                                    label: "Credentials",
                                    description: null,
                                    form: {
                                        title: "Enter Credentials",
                                        description: "Provide your API key for the selected provider.",
                                        fields: [
                                            { name: "api_key", label: "API Key", field_type: "password", required: true, default_value: null, options: null, placeholder: "Enter your API key...", help_text: "This will be stored securely in your local config." },
                                        ],
                                        form_id: "wiz-step-1",
                                        submit_label: "Next",
                                    },
                                },
                                {
                                    label: "Test",
                                    description: null,
                                    form: {
                                        title: "Verify Connection",
                                        description: "We'll send a test request to verify your credentials.",
                                        fields: [
                                            { name: "test_prompt", label: "Test Prompt", field_type: "text", required: false, default_value: "Say hello", options: null, placeholder: null, help_text: "A simple prompt to verify the connection works." },
                                        ],
                                        form_id: "wiz-step-2",
                                        submit_label: "Finish",
                                    },
                                },
                            ],
                        },
                    },
                });
            });
            await page.waitForTimeout(300);
            await screenshot(page, "08a-wizard-step-1");

            // Check step indicators
            var indicators = await page.locator(".wizard-step-indicator").count();
            check("wizard has 3 step indicators", indicators === 3);

            var activeIndicator = await page.locator(".wizard-step-indicator.active").count();
            check("first step indicator is active", activeIndicator === 1);

            // Select provider and advance
            await page.locator('select[name="provider"]').selectOption("anthropic");
            await page.locator(".submit-btn").click();
            await page.waitForTimeout(300);
            await screenshot(page, "08b-wizard-step-2");

            // Check step 1 is now completed
            var completedIndicators = await page.locator(".wizard-step-indicator.completed").count();
            check("first step is completed", completedIndicators === 1);

            // Fill API key and advance
            await page.locator('input[name="api_key"]').fill("sk-ant-test-key");
            await page.locator(".submit-btn").click();
            await page.waitForTimeout(300);
            await screenshot(page, "08c-wizard-step-3");

            await context.close();
        })();

        // =====================================================================
        // SCENARIO 9: Progress bar
        // =====================================================================
        console.log("\n=== Scenario 9: Progress Bar ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 900, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({ setupNeeded: true, models: [] }));
            await page.goto(url);
            await page.waitForTimeout(300);

            // Show status + progress together
            await page.evaluate(function () {
                window.__sendEvent({ event: "status", data: { message: "Configuring models..." } });
                window.__sendEvent({ event: "progress", data: { message: "Installing providers", percent: 35, detail: "2 of 6 providers configured" } });
            });
            await page.waitForTimeout(200);
            await screenshot(page, "09a-progress-35-percent");

            // Update progress
            await page.evaluate(function () {
                window.__sendEvent({ event: "progress", data: { message: "Installing providers", percent: 70, detail: "5 of 6 providers configured" } });
            });
            await page.waitForTimeout(200);
            await screenshot(page, "09b-progress-70-percent");

            await context.close();
        })();

        // =====================================================================
        // SCENARIO 10: Command output and test results
        // =====================================================================
        console.log("\n=== Scenario 10: Command Output + Test Results ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 900, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({ setupNeeded: true, models: [] }));
            await page.goto(url);
            await page.waitForTimeout(300);

            // Send multiple results
            await page.evaluate(function () {
                window.__sendEvent({ event: "status", data: { message: "Testing integrations..." } });
                window.__sendEvent({
                    event: "show_result",
                    data: {
                        content: {
                            type: "command_output",
                            command: "claude -p 'Say hello' --output-format json --model claude-sonnet-4-6",
                            stdout: '{"result":"Hello! I\'m Claude, an AI assistant made by Anthropic. How can I help you today?","session_id":"abc123"}',
                            stderr: "",
                            exit_code: 0,
                        },
                    },
                });
                window.__sendEvent({
                    event: "show_result",
                    data: {
                        content: {
                            type: "test_result",
                            model: "claude-sonnet-4-6",
                            success: true,
                            output: "Hello! I'm Claude, an AI assistant made by Anthropic.",
                        },
                    },
                });
                window.__sendEvent({
                    event: "show_result",
                    data: {
                        content: {
                            type: "test_result",
                            model: "gpt-4o",
                            success: false,
                            output: "Error: 401 Unauthorized - Invalid API key",
                        },
                    },
                });
                window.__sendEvent({
                    event: "show_result",
                    data: {
                        content: {
                            type: "config_written",
                            path: "~/.config/oulipoly-agent-runner/models/claude-sonnet.toml",
                            description: "Created model configuration for Claude Sonnet",
                        },
                    },
                });
            });
            await page.waitForTimeout(300);
            await screenshot(page, "10-results-mixed");

            // Check results accumulate
            var results = await page.locator(".result-display").count();
            check("4 results accumulated", results === 4);

            await context.close();
        })();

        // =====================================================================
        // SCENARIO 11: Complete screen
        // =====================================================================
        console.log("\n=== Scenario 11: Setup Complete ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 900, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({ setupNeeded: true, models: [] }));
            await page.goto(url);
            await page.waitForTimeout(300);

            await page.evaluate(function () {
                window.__sendEvent({
                    event: "complete",
                    data: {
                        summary: "Setup completed successfully. 3 CLIs configured with 5 model providers.",
                        items_configured: [
                            "Claude Code CLI (authenticated)",
                            "Codex CLI (authenticated)",
                            "Puppy CLI (API key)",
                            "claude-sonnet-4-6 model",
                            "gpt-4o model",
                            "gemini-2.0-flash model",
                            "3 skills synced across CLIs",
                            "2 MCP servers installed",
                        ],
                    },
                });
            });
            await page.waitForTimeout(300);
            await screenshot(page, "11-setup-complete");

            var items = await page.locator(".configured-items li").count();
            check("shows all configured items", items === 8);

            var viewPoolsBtn = await page.locator(".setup-complete .btn-primary");
            check("View Pools button exists", await viewPoolsBtn.isVisible());

            await context.close();
        })();

        // =====================================================================
        // SCENARIO 12: Error states
        // =====================================================================
        console.log("\n=== Scenario 12: Error States ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 900, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({ setupNeeded: true, models: [] }));
            await page.goto(url);
            await page.waitForTimeout(300);

            // Recoverable error
            await page.evaluate(function () {
                window.__sendEvent({ event: "status", data: { message: "Configuring model..." } });
                window.__sendEvent({
                    event: "error",
                    data: { message: "Command 'codex setup' timed out after 30 seconds. The CLI may be unresponsive.", recoverable: true },
                });
            });
            await page.waitForTimeout(200);
            await screenshot(page, "12a-recoverable-error");

            // Non-recoverable error
            await page.evaluate(function () {
                window.__sendEvent({
                    event: "error",
                    data: { message: "Claude CLI crashed unexpectedly (SIGSEGV). The setup agent cannot continue.", recoverable: false },
                });
            });
            await page.waitForTimeout(200);
            await screenshot(page, "12b-non-recoverable-error");

            var retryBtn = await page.locator(".retry-btn");
            check("retry button appears for non-recoverable error", await retryBtn.isVisible());

            await context.close();
        })();

        // =====================================================================
        // SCENARIO 13: Pools view with models
        // =====================================================================
        console.log("\n=== Scenario 13: Pools View ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 900, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({
                setupNeeded: false,
                models: ["claude-sonnet-4-6", "claude-opus-4-6", "gpt-4o", "gpt-4o-mini", "gemini-2.0-flash", "o3"],
            }));

            await page.goto(url);
            await page.waitForTimeout(500);
            await screenshot(page, "13-pools-with-models");

            var cards = await page.locator(".model-card").count();
            check("pools view shows 6 model cards", cards === 6);

            await context.close();
        })();

        // =====================================================================
        // SCENARIO 14: Empty pools
        // =====================================================================
        console.log("\n=== Scenario 14: Empty Pools ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 900, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({
                setupNeeded: false,
                models: [],
            }));

            await page.goto(url);
            await page.waitForTimeout(500);
            await screenshot(page, "14-pools-empty");

            var emptyState = await page.locator(".empty-state");
            check("empty state message shown", await emptyState.isVisible());

            await context.close();
        })();

        // =====================================================================
        // SCENARIO 15: Full flow simulation
        // =====================================================================
        console.log("\n=== Scenario 15: Full Flow Simulation ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 900, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({ setupNeeded: true, models: [] }));
            await page.goto(url);
            await page.waitForTimeout(300);

            // Step 1: Detection
            await page.evaluate(function () {
                window.__sendEvent({ event: "status", data: { message: "Detecting installed CLIs..." } });
            });
            await page.waitForTimeout(500);

            await page.evaluate(function () {
                window.__sendEvent({
                    event: "show_result",
                    data: {
                        content: {
                            type: "detection_summary",
                            clis: [
                                { name: "claude", installed: true, version: "1.0.33", authenticated: false, wrapper_count: 0 },
                                { name: "codex", installed: false, version: null, authenticated: false, wrapper_count: 0 },
                            ],
                        },
                    },
                });
            });
            await page.waitForTimeout(300);
            await screenshot(page, "15a-flow-detection");

            // Step 2: Ask for OAuth
            await page.evaluate(function () {
                window.__sendEvent({
                    event: "need_input",
                    data: {
                        action: {
                            type: "oauth_flow",
                            provider: "claude",
                            login_command: "claude login",
                            instructions: "Claude is installed but not authenticated.\n\n1. Run: claude login\n2. Complete the browser flow\n3. Click below when done",
                        },
                    },
                });
            });
            await page.waitForTimeout(300);
            await screenshot(page, "15b-flow-oauth");

            // User clicks "I've logged in"
            await page.locator(".done-btn").click();
            await page.waitForTimeout(200);
            await screenshot(page, "15c-flow-after-oauth");

            // Step 3: Status updates
            await page.evaluate(function () {
                window.__sendEvent({ event: "status", data: { message: "Verifying authentication..." } });
            });
            await page.waitForTimeout(300);

            await page.evaluate(function () {
                window.__sendEvent({ event: "status", data: { message: "Configuring model providers..." } });
                window.__sendEvent({ event: "progress", data: { message: "Setting up models", percent: 50, detail: "1 of 2 configured" } });
            });
            await page.waitForTimeout(300);
            await screenshot(page, "15d-flow-configuring");

            // Step 4: Test results
            await page.evaluate(function () {
                window.__sendEvent({
                    event: "show_result",
                    data: {
                        content: { type: "test_result", model: "claude-sonnet-4-6", success: true, output: "Hello!" },
                    },
                });
                window.__sendEvent({ event: "progress", data: { message: "Setting up models", percent: 100, detail: "2 of 2 configured" } });
            });
            await page.waitForTimeout(300);
            await screenshot(page, "15e-flow-test-results");

            // Step 5: Complete
            await page.evaluate(function () {
                window.__sendEvent({
                    event: "complete",
                    data: {
                        summary: "Setup complete! Claude is configured and ready.",
                        items_configured: ["Claude CLI (authenticated)", "claude-sonnet-4-6 model"],
                    },
                });
            });
            await page.waitForTimeout(300);
            await screenshot(page, "15f-flow-complete");

            await context.close();
        })();

        // =====================================================================
        // SCENARIO 16: Narrow viewport (mobile-ish)
        // =====================================================================
        console.log("\n=== Scenario 16: Narrow Viewport ===\n");

        await (async function () {
            var context = await browser.newContext({ viewport: { width: 400, height: 700 } });
            var page = await context.newPage();

            await page.addInitScript(getTauriMockScript({ setupNeeded: true, models: [] }));
            await page.goto(url);
            await page.waitForTimeout(300);

            await page.evaluate(function () {
                window.__sendEvent({
                    event: "need_input",
                    data: {
                        action: {
                            type: "form",
                            title: "Configure Provider",
                            description: "This is a test at narrow width to check for overflow issues.",
                            form_id: "narrow-form",
                            submit_label: "Save",
                            fields: [
                                { name: "key", label: "API Key", field_type: "password", required: true, default_value: null, options: null, placeholder: "Enter a very long API key to test overflow behavior...", help_text: "This help text should wrap properly at narrow widths without getting cut off" },
                                { name: "model", label: "Model", field_type: "select", required: true, default_value: null, options: [{ label: "A very long model name that might overflow", value: "long" }], placeholder: "Choose...", help_text: null },
                            ],
                        },
                    },
                });
            });
            await page.waitForTimeout(300);
            await screenshot(page, "16a-narrow-form");

            // Detection table at narrow width
            await page.evaluate(function () {
                window.__sendEvent({
                    event: "show_result",
                    data: {
                        content: {
                            type: "detection_summary",
                            clis: [
                                { name: "claude", installed: true, version: "1.0.33", authenticated: true, wrapper_count: 2 },
                                { name: "codex", installed: false, version: null, authenticated: false, wrapper_count: 0 },
                            ],
                        },
                    },
                });
            });
            await page.waitForTimeout(300);
            await screenshot(page, "16b-narrow-detection-table");

            await context.close();
        })();

    } finally {
        await browser.close();
        server.close();
    }

    // Print summary
    console.log("\n" + "=".repeat(60));
    console.log("QA Visual Test Results: " + passed + " passed, " + failed + " failed");
    console.log("Screenshots saved to: " + screenshotDir);
    console.log("Screenshots:");
    screenshots.forEach(function (name) {
        console.log("  - " + name + ".png");
    });
    console.log("=".repeat(60) + "\n");

    if (failed > 0) process.exit(1);
}

function check(name, condition) {
    if (condition) {
        passed++;
        console.log("  PASS: " + name);
    } else {
        failed++;
        console.log("  FAIL: " + name);
    }
}

run().catch(function (e) {
    console.error("Fatal error:", e);
    process.exit(1);
});
