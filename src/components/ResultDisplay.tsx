import { For, Match, Show, Switch } from "solid-js";
import { badge } from "../lib/styles";
import type { CliSummaryItem, ResultContent } from "../lib/types";

// TODO(design): Success/error results use plain text + generic colors.
// Need custom animated indicator SVGs for success/error/warning.
// Still needs design from designer.

interface ResultDisplayProps {
	content: ResultContent;
}

export default function ResultDisplay(props: ResultDisplayProps) {
	return (
		<div class="rounded-lg bg-[#16213e] p-4">
			<Switch>
				<Match when={props.content.type === "command_output"}>
					<CommandOutput
						content={
							props.content as Extract<
								ResultContent,
								{ type: "command_output" }
							>
						}
					/>
				</Match>
				<Match when={props.content.type === "detection_summary"}>
					<DetectionSummary
						content={
							props.content as Extract<
								ResultContent,
								{ type: "detection_summary" }
							>
						}
					/>
				</Match>
				<Match when={props.content.type === "config_written"}>
					<ConfigWritten
						content={
							props.content as Extract<
								ResultContent,
								{ type: "config_written" }
							>
						}
					/>
				</Match>
				<Match when={props.content.type === "test_result"}>
					<TestResult
						content={
							props.content as Extract<ResultContent, { type: "test_result" }>
						}
					/>
				</Match>
			</Switch>
		</div>
	);
}

function CommandOutput(props: {
	content: {
		command: string;
		stdout: string;
		stderr: string;
		exit_code: number;
	};
}) {
	return (
		<>
			<div class="mb-2 font-mono text-xs text-text-dim">
				$ {props.content.command}
			</div>
			<pre class="max-h-[200px] overflow-auto whitespace-pre-wrap break-all rounded bg-[#1e2a4a] p-2 font-mono text-xs text-text">
				{props.content.stdout}
			</pre>
			<Show when={props.content.stderr}>
				<pre class="mt-1 max-h-[200px] overflow-auto whitespace-pre-wrap break-all rounded bg-[#2a1a1a] p-2 font-mono text-xs text-error">
					{props.content.stderr}
				</pre>
			</Show>
			<div class="mt-1 text-xs text-text-dim">
				Exit code: {props.content.exit_code}
			</div>
		</>
	);
}

function DetectionSummary(props: { content: { clis: CliSummaryItem[] } }) {
	return (
		<>
			<h3 class="mb-2 text-sm text-text">Detected CLIs</h3>
			<table class="w-full border-collapse text-[13px]">
				<thead>
					<tr>
						<th class="border-b border-[#2a3a5e] p-2 text-left font-medium text-text-dim">
							CLI
						</th>
						<th class="border-b border-[#2a3a5e] p-2 text-left font-medium text-text-dim">
							Installed
						</th>
						<th class="border-b border-[#2a3a5e] p-2 text-left font-medium text-text-dim">
							Version
						</th>
						<th class="border-b border-[#2a3a5e] p-2 text-left font-medium text-text-dim">
							Auth
						</th>
						<th class="border-b border-[#2a3a5e] p-2 text-left font-medium text-text-dim">
							Wrappers
						</th>
					</tr>
				</thead>
				<tbody>
					<For each={props.content.clis}>
						{(cli) => (
							<tr>
								<td class="border-b border-[#2a3a5e] p-2">{cli.name}</td>
								<td
									class={`border-b border-[#2a3a5e] p-2 ${badge({ status: cli.installed ? "success" : "error" })}`}
								>
									{cli.installed ? "Yes" : "No"}
								</td>
								<td class="border-b border-[#2a3a5e] p-2">
									{cli.version || "-"}
								</td>
								<td class="border-b border-[#2a3a5e] p-2">
									{cli.authenticated ? "Yes" : "No"}
								</td>
								<td class="border-b border-[#2a3a5e] p-2">
									{cli.wrapper_count}
								</td>
							</tr>
						)}
					</For>
				</tbody>
			</table>
		</>
	);
}

function ConfigWritten(props: {
	content: { path: string; description: string };
}) {
	return (
		<div class="flex items-center gap-2">
			<span class={`${badge({ status: "success" })} text-lg`}>&#10003;</span>
			<span class="text-sm text-text">{props.content.description}</span>
			<div class="mt-1 font-mono text-xs text-text-dim">
				{props.content.path}
			</div>
		</div>
	);
}

function TestResult(props: {
	content: { model: string; success: boolean; output: string };
}) {
	return (
		<div class="flex flex-col gap-1">
			<div>
				<span
					class={badge({
						status: props.content.success ? "success" : "error",
					})}
				>
					{props.content.success ? "\u2713" : "\u2717"}
				</span>{" "}
				{props.content.model}: {props.content.success ? "PASS" : "FAIL"}
			</div>
			<pre class="max-h-[100px] overflow-auto font-mono text-[11px]">
				{props.content.output}
			</pre>
		</div>
	);
}
