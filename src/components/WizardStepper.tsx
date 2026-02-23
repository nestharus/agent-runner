import { Steps } from "@ark-ui/solid";
import { createSignal, For, Show } from "solid-js";
import { button } from "../lib/styles";
import type { WizardAction } from "../lib/types";
import FormRenderer from "./FormRenderer";

interface WizardStepperProps {
	wizard: WizardAction;
	onStepSubmit: (step: number, values: Record<string, string>) => void;
}

export default function WizardStepper(props: WizardStepperProps) {
	const [currentStep, setCurrentStep] = createSignal(props.wizard.current_step);

	function handleFormSubmit(values: Record<string, string>) {
		props.onStepSubmit(currentStep(), values);
	}

	function handleBack() {
		const step = currentStep();
		if (step > 0) {
			setCurrentStep(step - 1);
		}
	}

	return (
		<div class="mx-auto max-w-[600px]">
			<Steps.Root
				count={props.wizard.steps.length}
				step={currentStep()}
				onStepChange={(details) => setCurrentStep(details.step)}
			>
				<Steps.List class="mb-4 flex gap-1">
					<For each={props.wizard.steps}>
						{(_step, i) => (
							<Steps.Item index={i()}>
								<Steps.Trigger
									class={`h-2 flex-1 rounded-sm transition-colors duration-300 ${
										i() < currentStep()
											? "bg-success"
											: i() === currentStep()
												? "bg-accent"
												: "bg-[#2a3a5e]"
									}`}
								/>
							</Steps.Item>
						)}
					</For>
				</Steps.List>

				<div class="mb-6 flex justify-between text-xs text-text-dim">
					<For each={props.wizard.steps}>
						{(step, i) => (
							<span
								class={
									i() === currentStep()
										? "font-medium text-accent"
										: i() < currentStep()
											? "text-success"
											: undefined
								}
							>
								{step.label}
							</span>
						)}
					</For>
				</div>
			</Steps.Root>

			<FormRenderer
				form={props.wizard.steps[currentStep()].form}
				onSubmit={handleFormSubmit}
			/>

			<Show when={currentStep() > 0}>
				<button
					type="button"
					class={`mt-3 ${button({ intent: "secondary" })}`}
					onClick={handleBack}
				>
					Back
				</button>
			</Show>
		</div>
	);
}
