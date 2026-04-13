import { readFileSync } from "node:fs";
import { join } from "node:path";

const fixtureRoot = process.argv[2];

if (!fixtureRoot) {
	process.stderr.write("fixture root argument is required\n");
	process.exit(1);
}

const readme = readFileSync(join(fixtureRoot, "README.md"), "utf8");
const runbook = readFileSync(join(fixtureRoot, "docs", "runbook.md"), "utf8");
const nodeVersion = process.version;

const report = [
	"# Custom Pack Report",
	"",
	"## Summary",
	`- Node.js version: ${nodeVersion}`,
	"- The fixture does not define a rollback owner.",
	"- The fixture does not define a restore drill.",
	"",
	"## Evidence",
	`- README.md: ${JSON.stringify(readme.split("\\n").find((line) => line.includes("rollback owner")) ?? "missing")}`,
	`- docs/runbook.md: ${JSON.stringify(runbook.split("\\n").find((line) => line.includes("restore drill")) ?? "missing")}`,
].join("\\n");

process.stdout.write(report);
