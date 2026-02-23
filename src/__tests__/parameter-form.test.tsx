import {
	cleanup,
	fireEvent,
	render,
	screen,
	waitFor,
} from "@solidjs/testing-library";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ParameterForm from "../components/ParameterForm";
import type { Parameter } from "../lib/types";

beforeEach(() => {
	cleanup();
});

describe("ParameterForm", () => {
	it("renders enum parameter as radio chips", () => {
		const params: Parameter[] = [
			{
				name: "reasoning_effort",
				display_name: "Reasoning Effort",
				param_type: { type: "enum", options: ["low", "medium", "high"] },
				description: "Controls how much reasoning the model uses",
			},
		];

		render(() => (
			<ParameterForm
				parameters={params}
				values={{ reasoning_effort: "medium" }}
				onChange={() => {}}
			/>
		));

		expect(screen.getByText("Reasoning Effort")).toBeTruthy();
		expect(screen.getByText("low")).toBeTruthy();
		expect(screen.getByText("medium")).toBeTruthy();
		expect(screen.getByText("high")).toBeTruthy();

		// Enum options render as native radio inputs -- selected one should be checked
		const radios = screen.getAllByRole("radio") as HTMLInputElement[];
		expect(radios.length).toBe(3);

		const mediumRadio = radios.find((r) => r.value === "medium");
		expect(mediumRadio).toBeTruthy();
		expect(mediumRadio?.checked).toBe(true);

		const lowRadio = radios.find((r) => r.value === "low");
		expect(lowRadio).toBeTruthy();
		expect(lowRadio?.checked).toBe(false);

		// Description should appear
		expect(
			screen.getByText("Controls how much reasoning the model uses"),
		).toBeTruthy();
	});

	it("renders boolean parameter as toggle switch", () => {
		const params: Parameter[] = [
			{
				name: "streaming",
				display_name: "Streaming",
				param_type: { type: "boolean" },
				description: "Enable streaming responses",
			},
		];

		render(() => (
			<ParameterForm
				parameters={params}
				values={{ streaming: "true" }}
				onChange={() => {}}
			/>
		));

		expect(screen.getByText("Streaming")).toBeTruthy();
		expect(screen.getByText("Enable streaming responses")).toBeTruthy();

		// Should render a switch with a hidden checkbox input
		const checkbox = screen.getByRole("checkbox") as HTMLInputElement;
		expect(checkbox).toBeTruthy();
		expect(checkbox.checked).toBe(true);
	});

	it("renders number parameter as number input", () => {
		const params: Parameter[] = [
			{
				name: "temperature",
				display_name: "Temperature",
				param_type: { type: "number", min: 0, max: 2 },
				description: "Sampling temperature",
			},
		];

		render(() => (
			<ParameterForm
				parameters={params}
				values={{ temperature: "0.7" }}
				onChange={() => {}}
			/>
		));

		expect(screen.getByText("Temperature")).toBeTruthy();
		expect(screen.getByText("Sampling temperature")).toBeTruthy();

		const input = screen.getByDisplayValue("0.7") as HTMLInputElement;
		expect(input).toBeTruthy();
		expect(input.type).toBe("number");
		expect(input.min).toBe("0");
		expect(input.max).toBe("2");
	});

	it("renders string parameter as text input", () => {
		const params: Parameter[] = [
			{
				name: "system_prompt",
				display_name: "System Prompt",
				param_type: { type: "string" },
				description: "Custom system prompt",
			},
		];

		render(() => (
			<ParameterForm
				parameters={params}
				values={{ system_prompt: "You are helpful" }}
				onChange={() => {}}
			/>
		));

		expect(screen.getByText("System Prompt")).toBeTruthy();
		expect(screen.getByText("Custom system prompt")).toBeTruthy();

		const input = screen.getByDisplayValue(
			"You are helpful",
		) as HTMLInputElement;
		expect(input).toBeTruthy();
		expect(input.type).toBe("text");
	});

	it("onChange fires with correct values for enum selection", () => {
		const onChange = vi.fn();
		const params: Parameter[] = [
			{
				name: "reasoning_effort",
				display_name: "Reasoning Effort",
				param_type: { type: "enum", options: ["low", "medium", "high"] },
				description: "Controls reasoning",
			},
		];

		render(() => (
			<ParameterForm
				parameters={params}
				values={{ reasoning_effort: "low" }}
				onChange={onChange}
			/>
		));

		fireEvent.click(screen.getByText("high"));
		expect(onChange).toHaveBeenCalledWith({ reasoning_effort: "high" });
	});

	it("onChange fires with correct values for boolean toggle", async () => {
		const onChange = vi.fn();
		const params: Parameter[] = [
			{
				name: "streaming",
				display_name: "Streaming",
				param_type: { type: "boolean" },
				description: "Enable streaming",
			},
		];

		render(() => (
			<ParameterForm
				parameters={params}
				values={{ streaming: "false" }}
				onChange={onChange}
			/>
		));

		// Ark UI Switch uses Zag.js state machine which processes state changes
		// asynchronously via microtasks. Need to use waitFor.
		const checkbox = screen.getByRole("checkbox") as HTMLInputElement;
		checkbox.click();

		await waitFor(() => {
			expect(onChange).toHaveBeenCalledWith({ streaming: "true" });
		});
	});

	it("onChange fires with correct values for number input", () => {
		const onChange = vi.fn();
		const params: Parameter[] = [
			{
				name: "temperature",
				display_name: "Temperature",
				param_type: { type: "number", min: 0, max: 2 },
				description: "Sampling temperature",
			},
		];

		render(() => (
			<ParameterForm
				parameters={params}
				values={{ temperature: "0.7" }}
				onChange={onChange}
			/>
		));

		const input = screen.getByDisplayValue("0.7") as HTMLInputElement;
		fireEvent.input(input, { target: { value: "1.2" } });
		expect(onChange).toHaveBeenCalledWith({ temperature: "1.2" });
	});

	it("onChange fires with correct values for string input", () => {
		const onChange = vi.fn();
		const params: Parameter[] = [
			{
				name: "system_prompt",
				display_name: "System Prompt",
				param_type: { type: "string" },
				description: "Custom prompt",
			},
		];

		render(() => (
			<ParameterForm
				parameters={params}
				values={{ system_prompt: "" }}
				onChange={onChange}
			/>
		));

		const input = document.querySelector(
			"input[type='text']",
		) as HTMLInputElement;
		fireEvent.input(input, { target: { value: "Be concise" } });
		expect(onChange).toHaveBeenCalledWith({ system_prompt: "Be concise" });
	});

	it("renders multiple parameters together", () => {
		const params: Parameter[] = [
			{
				name: "reasoning_effort",
				display_name: "Reasoning Effort",
				param_type: { type: "enum", options: ["low", "high"] },
				description: "Reasoning level",
			},
			{
				name: "streaming",
				display_name: "Streaming",
				param_type: { type: "boolean" },
				description: "Stream output",
			},
			{
				name: "temperature",
				display_name: "Temperature",
				param_type: { type: "number", min: 0, max: 2 },
				description: "Temp",
			},
		];

		render(() => (
			<ParameterForm
				parameters={params}
				values={{
					reasoning_effort: "low",
					streaming: "true",
					temperature: "1.0",
				}}
				onChange={() => {}}
			/>
		));

		expect(screen.getByText("Reasoning Effort")).toBeTruthy();
		expect(screen.getByText("Streaming")).toBeTruthy();
		expect(screen.getByText("Temperature")).toBeTruthy();
	});
});
