import type { ModelGroup } from "./types";

export interface ParsedModelName {
	group: string;
	facet: string | null;
	fullName: string;
}

export function parseModelName(name: string): ParsedModelName {
	const idx = name.indexOf("~");
	if (idx === -1) {
		return { group: name, facet: null, fullName: name };
	}
	return {
		group: name.slice(0, idx),
		facet: name.slice(idx + 1),
		fullName: name,
	};
}

export function groupModels(names: string[]): ModelGroup[] {
	const map = new Map<string, { facets: string[]; modelNames: string[] }>();

	for (const name of names) {
		const parsed = parseModelName(name);
		let entry = map.get(parsed.group);
		if (!entry) {
			entry = { facets: [], modelNames: [] };
			map.set(parsed.group, entry);
		}
		if (parsed.facet !== null) {
			entry.facets.push(parsed.facet);
		}
		entry.modelNames.push(parsed.fullName);
	}

	const groups: ModelGroup[] = [];
	for (const [group, entry] of map) {
		entry.facets.sort();
		entry.modelNames.sort();
		groups.push({
			group,
			facets: entry.facets,
			modelNames: entry.modelNames,
			standalone: entry.facets.length === 0,
		});
	}

	return groups;
}

export function resolveModelName(group: string, facet: string): string {
	return `${group}~${facet}`;
}
