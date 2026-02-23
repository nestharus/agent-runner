import { createSignal, For, Match, onMount, Show, Switch } from "solid-js";
import { Channel, cancelSetup, setupRespond } from "../lib/tauri";
import type {
	Action,
	ResultContent,
	SetupEvent,
	UserResponse,
} from "../lib/types";
import ApiKeyEntry from "./ApiKeyEntry";
import CliSelector from "./CliSelector";
import ConfirmDialog from "./ConfirmDialog";
import FormRenderer from "./FormRenderer";
import OAuthFlow from "./OAuthFlow";
import ProgressBar from "./ProgressBar";
import ResultDisplay from "./ResultDisplay";
import StatusBar from "./StatusBar";
import WizardStepper from "./WizardStepper";

interface SetupSessionProps {
	startFn: (channel: Channel<SetupEvent>) => Promise<string>;
	onComplete: (summary: string, items: string[]) => void;
	onCancel: () => void;
}

export default function SetupSession(props: SetupSessionProps) {
	const [status, setStatus] = createSignal("");
	const [statusVisible, setStatusVisible] = createSignal(false);
	const [progressMsg, setProgressMsg] = createSignal("");
	const [progressPct, setProgressPct] = createSignal<number | null>(null);
	const [progressDetail, setProgressDetail] = createSignal<string | null>(null);
	const [progressVisible, setProgressVisible] = createSignal(false);
	const [currentAction, setCurrentAction] = createSignal<Action | null>(null);
	const [results, setResults] = createSignal<ResultContent[]>([]);
	const [error, setError] = createSignal<string | null>(null);
	const [showRetry, setShowRetry] = createSignal(false);
	const [showStale, setShowStale] = createSignal(false);

	function sendResponse(response: UserResponse) {
		setCurrentAction(null);
		setStatusVisible(true);
		setStatus("Processing...");
		setupRespond(response).catch(() => {
			setStatusVisible(false);
			setShowStale(true);
		});
	}

	function handleEvent(event: SetupEvent) {
		switch (event.event) {
			case "status":
				setStatusVisible(true);
				setStatus(event.data.message);
				break;
			case "progress":
				setProgressVisible(true);
				setProgressMsg(event.data.message);
				setProgressPct(event.data.percent);
				setProgressDetail(event.data.detail);
				break;
			case "need_input":
				setStatusVisible(false);
				setCurrentAction(event.data.action);
				break;
			case "show_result":
				setResults((prev) => [...prev, event.data.content]);
				break;
			case "complete":
				setStatusVisible(false);
				setProgressVisible(false);
				setCurrentAction(null);
				props.onComplete(event.data.summary, event.data.items_configured);
				break;
			case "error":
				setError(event.data.message);
				if (!event.data.recoverable) {
					setStatusVisible(false);
					setProgressVisible(false);
					setShowRetry(true);
				}
				break;
		}
	}

	async function start() {
		setStatus("");
		setStatusVisible(false);
		setProgressVisible(false);
		setCurrentAction(null);
		setResults([]);
		setError(null);
		setShowRetry(false);
		setShowStale(false);

		const channel = new Channel<SetupEvent>();
		channel.onmessage = handleEvent;

		try {
			await props.startFn(channel);
		} catch (err) {
			setError(`Setup failed: ${String(err)}`);
			setShowRetry(true);
		}
	}

	function handleFreshStart() {
		cancelSetup().catch(() => {});
		start();
	}

	onMount(() => {
		start();
	});

	return (
		<div>
			<StatusBar message={status()} visible={statusVisible()} />
			<ProgressBar
				message={progressMsg()}
				percent={progressPct()}
				detail={progressDetail()}
				visible={progressVisible()}
			/>

			<Show when={error()}>
				<div class="mb-3 rounded-lg border-l-[3px] border-error bg-error/15 p-3 text-[13px] text-error">
					{error()}
				</div>
			</Show>

			<Switch>
				<Match when={currentAction()?.type === "form"}>
					<FormRenderer
						form={currentAction() as Extract<Action, { type: "form" }>}
						onSubmit={(values) => {
							const action = currentAction() as Extract<
								Action,
								{ type: "form" }
							>;
							sendResponse({
								type: "form_submit",
								form_id: action.form_id,
								values,
							});
						}}
					/>
				</Match>
				<Match when={currentAction()?.type === "wizard"}>
					<WizardStepper
						wizard={currentAction() as Extract<Action, { type: "wizard" }>}
						onStepSubmit={(step, values) => {
							const action = currentAction() as Extract<
								Action,
								{ type: "wizard" }
							>;
							sendResponse({
								type: "wizard_step",
								wizard_id: action.wizard_id,
								step,
								values,
							});
						}}
					/>
				</Match>
				<Match when={currentAction()?.type === "confirm"}>
					{(() => {
						const action = () =>
							currentAction() as Extract<Action, { type: "confirm" }>;
						return (
							<ConfirmDialog
								title={action().title}
								message={action().message}
								confirmLabel={action().confirm_label}
								cancelLabel={action().cancel_label}
								onConfirm={() =>
									sendResponse({
										type: "confirm",
										confirm_id: action().confirm_id,
										confirmed: true,
									})
								}
								onCancel={() =>
									sendResponse({
										type: "confirm",
										confirm_id: action().confirm_id,
										confirmed: false,
									})
								}
							/>
						);
					})()}
				</Match>
				<Match when={currentAction()?.type === "oauth_flow"}>
					{(() => {
						const action = () =>
							currentAction() as Extract<Action, { type: "oauth_flow" }>;
						return (
							<OAuthFlow
								provider={action().provider}
								instructions={action().instructions}
								onDone={() =>
									sendResponse({
										type: "oauth_complete",
										provider: action().provider,
										success: true,
									})
								}
								onSkip={() =>
									sendResponse({
										type: "oauth_complete",
										provider: action().provider,
										success: false,
									})
								}
							/>
						);
					})()}
				</Match>
				<Match when={currentAction()?.type === "api_key_entry"}>
					{(() => {
						const action = () =>
							currentAction() as Extract<Action, { type: "api_key_entry" }>;
						return (
							<ApiKeyEntry
								provider={action().provider}
								envVar={action().env_var}
								helpUrl={action().help_url}
								onSubmit={(key) =>
									sendResponse({
										type: "api_key",
										provider: action().provider,
										key,
									})
								}
							/>
						);
					})()}
				</Match>
				<Match when={currentAction()?.type === "cli_selection"}>
					{(() => {
						const action = () =>
							currentAction() as Extract<Action, { type: "cli_selection" }>;
						return (
							<CliSelector
								message={action().message}
								available={action().available}
								onSubmit={(selected) =>
									sendResponse({
										type: "cli_selection",
										selected,
									})
								}
							/>
						);
					})()}
				</Match>
			</Switch>

			<Show when={results().length > 0}>
				<div class="mt-4 flex flex-col gap-3">
					<For each={results()}>
						{(content) => <ResultDisplay content={content} />}
					</For>
				</div>
			</Show>

			<Show when={showRetry()}>
				<div class="mt-4 flex justify-center gap-2">
					<button
						type="button"
						class="rounded bg-accent px-5 py-2 text-sm font-medium text-black transition-colors hover:bg-accent-hover"
						onClick={() => start()}
					>
						Retry Setup
					</button>
					<button
						type="button"
						class="rounded bg-[#2a3a5e] px-5 py-2 text-sm font-medium text-text transition-colors hover:bg-[#0f3460]"
						onClick={() => props.onCancel()}
					>
						Cancel
					</button>
				</div>
			</Show>

			<Show when={showStale()}>
				<div class="rounded-lg bg-[#0f3460] p-6 text-center">
					<p class="mb-4 text-text-dim">
						The setup session is no longer active.
					</p>
					<div class="flex justify-center gap-2">
						<button
							type="button"
							class="rounded bg-accent px-5 py-2 text-sm font-medium text-black transition-colors hover:bg-accent-hover"
							onClick={handleFreshStart}
						>
							Start Fresh
						</button>
						<button
							type="button"
							class="rounded bg-[#2a3a5e] px-5 py-2 text-sm font-medium text-text transition-colors hover:bg-[#0f3460]"
							onClick={() => props.onCancel()}
						>
							Cancel
						</button>
					</div>
				</div>
			</Show>
		</div>
	);
}
