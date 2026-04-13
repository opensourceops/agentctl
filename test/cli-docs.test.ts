import { readFileSync } from "node:fs";
import { describe, expect, test } from "vitest";
import { getCommandDoc, renderCommandHelp, renderReadmeCliReference, renderRootHelp } from "../src/cli-docs.js";

const README_PATH = new URL("../README.md", import.meta.url);
const START_MARKER = "<!-- CLI_REFERENCE:START -->";
const END_MARKER = "<!-- CLI_REFERENCE:END -->";

function extractGeneratedReadmeSection(): string {
	const readme = readFileSync(README_PATH, "utf8");
	const start = readme.indexOf(START_MARKER);
	const end = readme.indexOf(END_MARKER);
	if (start === -1 || end === -1 || end < start) {
		throw new Error("README CLI reference markers are missing");
	}
	return readme.slice(start + START_MARKER.length, end).trim();
}

describe("cli docs", () => {
	test("root help is rendered from command metadata", () => {
		const help = renderRootHelp();
		expect(help).toContain("agentctl run <playbook.yaml> [flags]");
		expect(help).toContain("Use command-specific help for examples and command-specific flags.");
		expect(help).toContain("--output yaml|json");
	});

	test("memory help preserves command-specific provider semantics", () => {
		const memory = getCommandDoc("memory");
		expect(memory).toBeDefined();
		const help = renderCommandHelp(memory!);
		expect(help).toContain("--provider sqlite|mongodb-atlas");
		expect(help).toContain("Long-term memory backend");
		expect(help).not.toContain("Provider for --api-key");
	});

	test("README CLI reference stays generated from command metadata", () => {
		expect(extractGeneratedReadmeSection()).toBe(renderReadmeCliReference());
	});
});
