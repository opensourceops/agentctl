export interface CliFlagDoc {
	readonly long: string;
	readonly short?: string;
	readonly valueHint?: string;
	readonly description: string;
}

export interface CliCommandDoc {
	readonly name: string;
	readonly summary: string;
	readonly usage: readonly string[];
	readonly description?: readonly string[];
	readonly examples?: readonly string[];
	readonly flags?: readonly CliFlagDoc[];
}

const GLOBAL_FLAGS: readonly CliFlagDoc[] = [
	{ short: "-h", long: "--help", description: "Show help" },
	{ short: "-v", long: "--verbose", description: "Show full structured output" },
	{ short: "-V", long: "--version", description: "Show version" },
	{ long: "--output", valueHint: "yaml|json", description: "Structured output format" },
	{ long: "--color", valueHint: "auto|always|never", description: "YAML color mode" },
] as const;

const RUNTIME_FLAGS: readonly CliFlagDoc[] = [
	{ long: "--db", valueHint: "path", description: "Runtime database path (default: ~/.agentctl/runtime/runtime.db)" },
	{ long: "--api-key", valueHint: "key", description: "Runtime API key override" },
	{ long: "--provider", valueHint: "name", description: "Provider for --api-key (default: openai)" },
] as const;

const MEMORY_FLAGS: readonly CliFlagDoc[] = [
	{ long: "--provider", valueHint: "sqlite|mongodb-atlas", description: "Long-term memory backend" },
	{ long: "--db", valueHint: "path", description: "SQLite memory DB path (default: ~/.agentctl/memory/long-term.db)" },
	{ long: "--connection-string", valueHint: "uri", description: "Remote memory backend connection string" },
	{ long: "--database", valueHint: "name", description: "Remote memory database name" },
	{ long: "--collection", valueHint: "name", description: "Remote memory collection name" },
	{ long: "--namespace", valueHint: "name", description: "Namespace filter or write target" },
	{ long: "--limit", valueHint: "N", description: "Maximum matches to return" },
	{ long: "--older-than-days", valueHint: "N", description: "Retention cutoff for memory gc" },
	{ long: "--keep-entries", valueHint: "N", description: "Newest entries to keep during memory gc" },
	{ long: "--value", valueHint: "json", description: "JSON value for memory write" },
	{ long: "--string", valueHint: "text", description: "Plain string value for memory write" },
	{ long: "--tags", valueHint: "a,b", description: "Comma-separated tags for memory write" },
] as const;

const DB_FLAGS: readonly CliFlagDoc[] = [{ long: "--db", valueHint: "path", description: "Runtime database path" }] as const;
const GC_FLAGS: readonly CliFlagDoc[] = [
	{ long: "--db", valueHint: "path", description: "Runtime database path" },
	{ long: "--older-than-days", valueHint: "N", description: "Delete terminal runs older than N days (default: 30)" },
	{ long: "--keep-runs", valueHint: "N", description: "Keep newest terminal runs regardless of age (default: 100)" },
] as const;
const AUTH_FLAGS: readonly CliFlagDoc[] = [
	{ long: "--api-key", valueHint: "key", description: "Runtime API key override" },
	{ long: "--provider", valueHint: "name", description: "Provider to inspect when no playbook is given" },
] as const;

export const ROOT_COMMAND_DOC: CliCommandDoc = {
	name: "agentctl",
	summary: "Declarative autonomous agent runtime with playbooks, packs, durable checkpoints, and replay.",
	usage: [
		"agentctl run <playbook.yaml> [flags]",
		"agentctl resume <playbook.yaml> <run-id> [flags]",
		"agentctl replay <playbook.yaml> <run-id> <checkpoint-seq> [flags]",
		"agentctl db stats [flags]",
		"agentctl memory <subcommand> [flags]",
		"agentctl gc [flags]",
		"agentctl auth check [playbook.yaml] [flags]",
		"agentctl schema",
		"agentctl update",
		"agentctl help",
		"agentctl version",
	],
	description: [
		"Use command-specific help for examples and command-specific flags.",
		'Examples: "agentctl run --help", "agentctl memory --help", "agentctl auth check --help".',
	],
	flags: GLOBAL_FLAGS,
	examples: [
		"agentctl run examples/hello.playbook.yaml",
		"agentctl db stats",
		"agentctl memory stats",
		"agentctl auth check examples/real-autonomy/mission.playbook.yaml",
	],
} as const;

export const COMMAND_DOCS: Record<string, CliCommandDoc> = {
	run: {
		name: "run",
		summary: "Start a new playbook run.",
		usage: ["agentctl run <playbook.yaml> [flags]"],
		description: ["Streams checkpoint events progressively and prints the final run result."],
		flags: [...GLOBAL_FLAGS, ...RUNTIME_FLAGS],
		examples: [
			"agentctl run examples/hello.playbook.yaml",
			"agentctl run examples/real-autonomy/mission.playbook.yaml --db .runtime/real-autonomy.db",
			"agentctl run examples/hello.playbook.yaml --output json --color never",
		],
	},
	resume: {
		name: "resume",
		summary: "Resume a non-terminal playbook run from its latest checkpoint.",
		usage: ["agentctl resume <playbook.yaml> <run-id> [flags]"],
		description: ["Fails fast for terminal runs and preserves already checkpointed side effects."],
		flags: [...GLOBAL_FLAGS, ...RUNTIME_FLAGS],
		examples: ["agentctl resume examples/hello.playbook.yaml <run-id> --db ~/.agentctl/runtime/runtime.db"],
	},
	replay: {
		name: "replay",
		summary: "Fork a fresh run from an earlier checkpoint.",
		usage: ["agentctl replay <playbook.yaml> <run-id> <checkpoint-seq> [flags]"],
		description: ["Creates a new run id and reuses the selected checkpoint snapshot as the starting state."],
		flags: [...GLOBAL_FLAGS, ...RUNTIME_FLAGS],
		examples: ["agentctl replay examples/hello.playbook.yaml <run-id> 3 --db ~/.agentctl/runtime/runtime.db"],
	},
	db: {
		name: "db",
		summary: "Inspect the runtime database.",
		usage: ["agentctl db stats [flags]"],
		description: ["Read-only runtime DB inspection. Fails on a missing DB path instead of creating one."],
		flags: [...GLOBAL_FLAGS, ...DB_FLAGS],
		examples: ["agentctl db stats", "agentctl db stats --db .runtime/real-autonomy.db --output json"],
	},
	memory: {
		name: "memory",
		summary: "Inspect and maintain long-term memory.",
		usage: [
			"agentctl memory get <key> [flags]",
			"agentctl memory search [flags]",
			"agentctl memory write <key> (--value json | --string text) [flags]",
			"agentctl memory stats [flags]",
			"agentctl memory gc [flags]",
		],
		description: [
			"Reads fail on a missing SQLite memory DB path; writes create the DB when needed.",
			'Use "--provider mongodb-atlas" to target the Atlas adapter instead of local SQLite.',
		],
		flags: [...GLOBAL_FLAGS, ...MEMORY_FLAGS],
		examples: [
			"agentctl memory get finding --namespace memory-flow",
			"agentctl memory search --query restore --limit 10",
			"agentctl memory write finding --namespace memory-flow --string restore-drill-missing --tags readiness,audit",
			"agentctl memory gc --older-than-days 30 --keep-entries 100",
		],
	},
	gc: {
		name: "gc",
		summary: "Remove old terminal runs from the runtime database.",
		usage: ["agentctl gc [flags]"],
		description: ["Running runs are never deleted."],
		flags: [...GLOBAL_FLAGS, ...GC_FLAGS],
		examples: ["agentctl gc", "agentctl gc --older-than-days 7 --keep-runs 20 --output json --verbose"],
	},
	auth: {
		name: "auth",
		summary: "Inspect provider auth configuration before a run.",
		usage: ["agentctl auth check [playbook.yaml] [flags]"],
		description: [
			"Exits nonzero when any required provider auth is missing.",
			"When a playbook is provided, only provider-backed agents in that playbook are inspected.",
		],
		flags: [...GLOBAL_FLAGS, ...AUTH_FLAGS],
		examples: [
			"agentctl auth check --provider openai",
			"agentctl auth check examples/real-autonomy/mission.playbook.yaml --output json",
		],
	},
	schema: {
		name: "schema",
		summary: "Print a short summary of the playbook DSL surface.",
		usage: ["agentctl schema"],
	},
	update: {
		name: "update",
		summary: "Print deterministic update instructions for a source checkout.",
		usage: ["agentctl update"],
	},
	version: {
		name: "version",
		summary: "Print the installed agentctl version.",
		usage: ["agentctl version", "agentctl --version", "agentctl -V"],
	},
	help: {
		name: "help",
		summary: "Show root help or command-specific help.",
		usage: ["agentctl help", "agentctl <command> --help"],
	},
} as const;

function formatFlag(flag: CliFlagDoc): string {
	const short = flag.short ? `${flag.short}, ` : "";
	const valueHint = flag.valueHint ? ` ${flag.valueHint}` : "";
	return `${short}${flag.long}${valueHint}`;
}

function renderFlags(flags: readonly CliFlagDoc[] | undefined): string[] {
	if (!flags || flags.length === 0) {
		return [];
	}
	return ["Flags:", ...flags.map((flag) => `  ${formatFlag(flag)}  ${flag.description}`)];
}

function renderExamples(examples: readonly string[] | undefined): string[] {
	if (!examples || examples.length === 0) {
		return [];
	}
	return ["Examples:", ...examples.map((example) => `  ${example}`)];
}

export function renderCommandHelp(command: CliCommandDoc): string {
	const lines = [
		"Usage:",
		...command.usage.map((usage) => `  ${usage}`),
		...(command.description && command.description.length > 0 ? ["", ...command.description] : []),
		...(command.examples && command.examples.length > 0 ? ["", ...renderExamples(command.examples)] : []),
		...(command.flags && command.flags.length > 0 ? ["", ...renderFlags(command.flags)] : []),
	];
	return `${lines.join("\n")}\n`;
}

export function renderRootHelp(): string {
	return renderCommandHelp(ROOT_COMMAND_DOC);
}

export function getCommandDoc(name: string): CliCommandDoc | undefined {
	return COMMAND_DOCS[name];
}

export function renderReadmeCliReference(): string {
	const ordered: readonly CliCommandDoc[] = [
		ROOT_COMMAND_DOC,
		COMMAND_DOCS.run!,
		COMMAND_DOCS.resume!,
		COMMAND_DOCS.replay!,
		COMMAND_DOCS.db!,
		COMMAND_DOCS.memory!,
		COMMAND_DOCS.gc!,
		COMMAND_DOCS.auth!,
		COMMAND_DOCS.schema!,
		COMMAND_DOCS.update!,
	];
	const sections = ordered.map((command) => {
		const title = command === ROOT_COMMAND_DOC ? "### Top-level help" : `### \`${command.name}\``;
		const usageBlock = `\`\`\`text\n${renderCommandHelp(command).trimEnd()}\n\`\`\``;
		return [title, usageBlock].join("\n\n");
	});
	return ["## CLI reference", ...sections].join("\n\n");
}
