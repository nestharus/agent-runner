export interface ParsedArg {
	key: string; // "--model", "-p", "" (positional)
	value: string | null; // null for standalone flags
}

export function parseArgs(args: string[]): ParsedArg[] {
	const result: ParsedArg[] = [];
	let i = 0;

	while (i < args.length) {
		const token = args[i];
		if (token.startsWith("-")) {
			// Check if next token is a value (doesn't start with -)
			if (i + 1 < args.length && !args[i + 1].startsWith("-")) {
				result.push({ key: token, value: args[i + 1] });
				i += 2;
			} else {
				result.push({ key: token, value: null });
				i += 1;
			}
		} else {
			// Positional argument
			result.push({ key: "", value: token });
			i += 1;
		}
	}

	return result;
}

export interface VariableArg {
	key: string;
	examples: Record<string, string>;
}

export interface ArgAnalysis {
	common: ParsedArg[]; // identical across all models for this provider
	variable: VariableArg[];
}

export function analyzeProviderArgs(
	entries: { modelName: string; args: string[] }[],
): ArgAnalysis {
	if (entries.length === 0) {
		return { common: [], variable: [] };
	}

	if (entries.length === 1) {
		const parsed = parseArgs(entries[0].args);
		return {
			common: parsed.filter((a) => a.value === null),
			variable: parsed
				.filter((a) => a.value !== null)
				.map((a) => ({
					key: a.key,
					examples: { [entries[0].modelName]: a.value as string },
				})),
		};
	}

	// Parse all entries
	const allParsed = entries.map((e) => ({
		modelName: e.modelName,
		parsed: parseArgs(e.args),
	}));

	// Collect all keys with their values per model
	const keyValues = new Map<string, Map<string, string | null>>();

	for (const { modelName, parsed } of allParsed) {
		for (const arg of parsed) {
			const mapKey = arg.key;
			if (!keyValues.has(mapKey)) {
				keyValues.set(mapKey, new Map());
			}
			keyValues.get(mapKey)?.set(modelName, arg.value);
		}
	}

	const common: ParsedArg[] = [];
	const variable: VariableArg[] = [];
	const modelNames = entries.map((e) => e.modelName);

	for (const [key, valuesMap] of keyValues) {
		// Check if all models have this key
		const allHave = modelNames.every((n) => valuesMap.has(n));
		if (!allHave) {
			// Not present in all — treat as variable
			const examples: Record<string, string> = {};
			for (const [name, val] of valuesMap) {
				if (val !== null) examples[name] = val;
			}
			if (Object.keys(examples).length > 0) {
				variable.push({ key, examples });
			}
			continue;
		}

		// All models have this key — check if values are identical
		const values = [...valuesMap.values()];
		const allSame = values.every((v) => v === values[0]);

		if (allSame) {
			common.push({ key, value: values[0] });
		} else {
			const examples: Record<string, string> = {};
			for (const [name, val] of valuesMap) {
				if (val !== null) examples[name] = val;
			}
			variable.push({ key, examples });
		}
	}

	return { common, variable };
}
