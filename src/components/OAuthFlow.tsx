import { button, card } from "../lib/styles";

// TODO(design): OAuth flow is a wall of text. Need:
// 1. Visual step indicators (terminal -> browser -> authorize -> done)
// 2. Custom lock/key SVG icon (designed, see design deliverables)
// 3. Ollie confused pose next to the form (designed, see design deliverables)

interface OAuthFlowProps {
	provider: string;
	instructions: string;
	onDone: () => void;
	onSkip: () => void;
}

export default function OAuthFlow(props: OAuthFlowProps) {
	return (
		<div class={card()}>
			<h3 class="mb-3 text-text">Authentication Required: {props.provider}</h3>
			<div class="mb-4 text-[13px] leading-relaxed text-text-dim">
				{renderInstructions(props.instructions)}
			</div>
			<div class="flex justify-end gap-2">
				<button
					type="button"
					class={button({ intent: "primary" })}
					onClick={props.onDone}
				>
					I've logged in
				</button>
				<button
					type="button"
					class={button({ intent: "secondary" })}
					onClick={props.onSkip}
				>
					Skip
				</button>
			</div>
		</div>
	);
}

function renderInstructions(text: string) {
	// Highlight single-quoted commands as inline code
	const parts = text.split(/('([^']+)')/g);
	return parts.map((part, i) => {
		if (i % 3 === 2) {
			return (
				<code class="rounded bg-[#1e2a4a] px-1.5 py-0.5 font-mono text-accent">
					{part}
				</code>
			);
		}
		if (i % 3 === 1) return null; // Skip the full match group
		return part;
	});
}
