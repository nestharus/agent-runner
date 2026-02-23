import { describe, expect, it } from "vitest";
import { groupModels, parseModelName, resolveModelName } from "../lib/grouping";

describe("parseModelName", () => {
	it("parses a name with tilde separator", () => {
		expect(parseModelName("gpt-codex~high")).toEqual({
			group: "gpt-codex",
			facet: "high",
			fullName: "gpt-codex~high",
		});
	});

	it("parses standalone name without tilde", () => {
		expect(parseModelName("glm")).toEqual({
			group: "glm",
			facet: null,
			fullName: "glm",
		});
	});

	it("splits on first tilde only", () => {
		expect(parseModelName("a~b~c")).toEqual({
			group: "a",
			facet: "b~c",
			fullName: "a~b~c",
		});
	});

	it("handles tilde at end", () => {
		expect(parseModelName("foo~")).toEqual({
			group: "foo",
			facet: "",
			fullName: "foo~",
		});
	});
});

describe("groupModels", () => {
	it("groups models with same prefix", () => {
		const groups = groupModels([
			"gpt-codex~high",
			"gpt-codex~low",
			"gpt-codex~medium",
		]);
		expect(groups).toEqual([
			{
				group: "gpt-codex",
				facets: ["high", "low", "medium"],
				modelNames: ["gpt-codex~high", "gpt-codex~low", "gpt-codex~medium"],
				standalone: false,
			},
		]);
	});

	it("keeps standalone models separate", () => {
		const groups = groupModels(["glm", "gpt~high", "gpt~low"]);
		expect(groups).toHaveLength(2);

		const glm = groups.find((g) => g.group === "glm");
		expect(glm).toEqual({
			group: "glm",
			facets: [],
			modelNames: ["glm"],
			standalone: true,
		});

		const gpt = groups.find((g) => g.group === "gpt");
		expect(gpt).toEqual({
			group: "gpt",
			facets: ["high", "low"],
			modelNames: ["gpt~high", "gpt~low"],
			standalone: false,
		});
	});

	it("returns empty array for empty input", () => {
		expect(groupModels([])).toEqual([]);
	});

	it("sorts facets alphabetically", () => {
		const groups = groupModels([
			"claude~sonnet",
			"claude~haiku",
			"claude~opus",
		]);
		expect(groups[0].facets).toEqual(["haiku", "opus", "sonnet"]);
	});

	it("preserves group order from input", () => {
		const groups = groupModels(["claude~sonnet", "gpt~high", "glm"]);
		expect(groups.map((g) => g.group)).toEqual(["claude", "gpt", "glm"]);
	});
});

describe("resolveModelName", () => {
	it("joins group and facet with tilde", () => {
		expect(resolveModelName("gpt-codex", "high")).toBe("gpt-codex~high");
	});
});
