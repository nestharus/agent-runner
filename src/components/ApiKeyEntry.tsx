import { Field } from "@ark-ui/solid";
import { createSignal, Show } from "solid-js";
import { button, card, input } from "../lib/styles";

interface ApiKeyEntryProps {
	provider: string;
	envVar: string;
	helpUrl: string | null;
	onSubmit: (key: string) => void;
}

export default function ApiKeyEntry(props: ApiKeyEntryProps) {
	const [key, setKey] = createSignal("");

	function handleSubmit() {
		const trimmed = key().trim();
		if (trimmed) {
			props.onSubmit(trimmed);
		}
	}

	return (
		<div class={card()}>
			<h3 class="mb-3 text-text">API Key: {props.provider}</h3>
			<p class="mb-2 text-sm text-text">
				Enter your API key for {props.envVar}
			</p>
			<Show when={props.helpUrl}>
				{(url) => (
					<a
						href={url()}
						target="_blank"
						rel="noopener noreferrer"
						class="mb-3 inline-block text-[13px] text-accent no-underline"
					>
						Get API key
					</a>
				)}
			</Show>
			<Field.Root>
				<Field.Input
					type="password"
					class={input()}
					placeholder="sk-..."
					onInput={(e) => setKey(e.currentTarget.value)}
				/>
			</Field.Root>
			<div class="mt-4 flex justify-end gap-2">
				<button
					type="button"
					class={button({ intent: "primary" })}
					onClick={handleSubmit}
				>
					Submit
				</button>
			</div>
		</div>
	);
}
