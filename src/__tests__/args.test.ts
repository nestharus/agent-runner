import { describe, expect, it } from "vitest";
import { analyzeProviderArgs, parseArgs } from "../lib/args";

describe("parseArgs", () => {
	it("parses flag with value", () => {
		expect(parseArgs(["--model", "sonnet"])).toEqual([
			{ key: "--model", value: "sonnet" },
		]);
	});

	it("parses standalone flag", () => {
		expect(parseArgs(["-p"])).toEqual([{ key: "-p", value: null }]);
	});

	it("parses positional argument", () => {
		expect(parseArgs(["exec"])).toEqual([{ key: "", value: "exec" }]);
	});

	it("parses mixed args", () => {
		expect(parseArgs(["-p", "--model", "sonnet"])).toEqual([
			{ key: "-p", value: null },
			{ key: "--model", value: "sonnet" },
		]);
	});

	it("parses consecutive flags", () => {
		expect(parseArgs(["-p", "--verbose"])).toEqual([
			{ key: "-p", value: null },
			{ key: "--verbose", value: null },
		]);
	});

	it("returns empty for empty input", () => {
		expect(parseArgs([])).toEqual([]);
	});
});

describe("analyzeProviderArgs", () => {
	it("returns empty for no entries", () => {
		expect(analyzeProviderArgs([])).toEqual({
			common: [],
			variable: [],
		});
	});

	it("identifies common and variable args across models", () => {
		const result = analyzeProviderArgs([
			{ modelName: "sonnet", args: ["-p", "--model", "sonnet"] },
			{ modelName: "opus", args: ["-p", "--model", "opus"] },
		]);

		expect(result.common).toEqual([{ key: "-p", value: null }]);
		expect(result.variable).toEqual([
			{
				key: "--model",
				examples: { sonnet: "sonnet", opus: "opus" },
			},
		]);
	});

	it("treats all-same values as common", () => {
		const result = analyzeProviderArgs([
			{ modelName: "a", args: ["--temp", "0.7"] },
			{ modelName: "b", args: ["--temp", "0.7"] },
		]);

		expect(result.common).toEqual([{ key: "--temp", value: "0.7" }]);
		expect(result.variable).toEqual([]);
	});

	it("handles single entry", () => {
		const result = analyzeProviderArgs([
			{ modelName: "solo", args: ["-p", "--model", "gpt-4"] },
		]);

		expect(result.common).toEqual([{ key: "-p", value: null }]);
		expect(result.variable).toEqual([
			{ key: "--model", examples: { solo: "gpt-4" } },
		]);
	});

	it("handles entries with no args", () => {
		const result = analyzeProviderArgs([
			{ modelName: "a", args: [] },
			{ modelName: "b", args: [] },
		]);

		expect(result.common).toEqual([]);
		expect(result.variable).toEqual([]);
	});
});
