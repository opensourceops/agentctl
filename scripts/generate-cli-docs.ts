import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { renderReadmeCliReference } from "../src/cli-docs.js";

const README_PATH = resolve(process.cwd(), "README.md");
const START_MARKER = "<!-- CLI_REFERENCE:START -->";
const END_MARKER = "<!-- CLI_REFERENCE:END -->";

function main(): void {
	const readme = readFileSync(README_PATH, "utf8");
	const start = readme.indexOf(START_MARKER);
	const end = readme.indexOf(END_MARKER);
	if (start === -1 || end === -1 || end < start) {
		throw new Error(`README markers not found: ${START_MARKER} ... ${END_MARKER}`);
	}

	const generated = `${START_MARKER}\n${renderReadmeCliReference()}\n${END_MARKER}`;
	const nextReadme = `${readme.slice(0, start)}${generated}${readme.slice(end + END_MARKER.length)}`;
	writeFileSync(README_PATH, nextReadme, "utf8");
}

main();
