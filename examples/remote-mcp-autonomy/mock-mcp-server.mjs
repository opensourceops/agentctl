#!/usr/bin/env node
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const PORT = Number(process.env.AGENTCTL_REMOTE_MCP_PORT ?? "43127");
const PROTOCOL_VERSION = "2025-11-25";
const SESSION_ID = "agentctl-remote-mcp-session";
const EXAMPLE_DIR = dirname(fileURLToPath(import.meta.url));

function isRecord(value) {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

async function readJson(request) {
	const chunks = [];
	for await (const chunk of request) {
		chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
	}
	return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

function sendJson(response, statusCode, body, headers = {}) {
	response.writeHead(statusCode, {
		"content-type": "application/json",
		...headers,
	});
	response.end(JSON.stringify(body));
}

async function buildReport(serviceRootArg) {
	const serviceRoot = resolve(EXAMPLE_DIR, serviceRootArg ?? "./fixtures/service");
	const readme = await readFile(resolve(serviceRoot, "README.md"), "utf8");
	const runbook = await readFile(resolve(serviceRoot, "docs/runbook.md"), "utf8");

	const rollbackMissing = /No rollback validation drill/i.test(readme) || /No rollback validation drill/i.test(runbook);
	const commsMissing =
		/No incident communication owner is documented/i.test(readme) ||
		/No incident communication owner is documented/i.test(runbook);

	const findings = [];
	if (rollbackMissing) {
		findings.push("Missing rollback validation drill");
	}
	if (commsMissing) {
		findings.push("Missing incident communication owner");
	}

	const report = `# Remote MCP Ops Report

## Summary
- Remote MCP audit inspected ${serviceRootArg ?? "./fixtures/service"}.
- A deployment checklist is documented in the fixture.
- No rollback validation drill is documented.
- No incident communication owner is documented.

## Risks
- Without a rollback validation drill, production rollback steps may fail under incident pressure and extend downtime.
- Without an incident communication owner, customer and internal updates may be delayed or inconsistent during an outage.

## Recommended Next Steps
- Add and rehearse a rollback validation drill for every production release train.
- Assign a named incident communication owner and document the role in the runbook.
`;

	return {
		stdout: report,
		findings,
		inspectedRoot: serviceRoot,
		source: "remote-mcp",
	};
}

const server = createServer(async (request, response) => {
	try {
		if (request.method !== "POST" || request.url !== "/mcp") {
			response.statusCode = 404;
			response.end();
			return;
		}

		const payload = await readJson(request);
		if (!isRecord(payload) || payload.jsonrpc !== "2.0" || typeof payload.method !== "string") {
			sendJson(response, 400, {
				jsonrpc: "2.0",
				id: isRecord(payload) ? payload.id ?? null : null,
				error: { code: -32600, message: "Invalid JSON-RPC payload" },
			});
			return;
		}

		if (payload.method === "initialize") {
			sendJson(
				response,
				200,
				{
					jsonrpc: "2.0",
					id: payload.id ?? null,
					result: {
						protocolVersion: PROTOCOL_VERSION,
						capabilities: {},
						serverInfo: {
							name: "agentctl-remote-mcp",
							version: "0.1.0",
						},
					},
				},
				{ "MCP-Session-Id": SESSION_ID },
			);
			return;
		}

		if (request.headers["mcp-session-id"] !== SESSION_ID) {
			sendJson(response, 404, {
				jsonrpc: "2.0",
				id: payload.id ?? null,
				error: { code: -32001, message: "Unknown MCP session" },
			});
			return;
		}

		if (payload.method === "notifications/initialized") {
			response.writeHead(202);
			response.end();
			return;
		}

		if (payload.method === "tools/list") {
			sendJson(response, 200, {
				jsonrpc: "2.0",
				id: payload.id ?? null,
				result: {
					tools: [
						{
							name: "audit_service",
							description: "Inspect a service fixture and return an operations report.",
							annotations: {
								readOnlyHint: true,
								idempotentHint: true,
							},
						},
					],
				},
			});
			return;
		}

		if (payload.method === "tools/call") {
			const params = isRecord(payload.params) ? payload.params : {};
			if (params.name !== "audit_service") {
				sendJson(response, 404, {
					jsonrpc: "2.0",
					id: payload.id ?? null,
					error: { code: -32601, message: `Unknown tool "${String(params.name ?? "")}"` },
				});
				return;
			}
			const args = isRecord(params.arguments) ? params.arguments : {};
			const result = await buildReport(typeof args.serviceRoot === "string" ? args.serviceRoot : undefined);
			sendJson(response, 200, {
				jsonrpc: "2.0",
				id: payload.id ?? null,
				result: {
					structuredContent: result,
				},
			});
			return;
		}

		sendJson(response, 404, {
			jsonrpc: "2.0",
			id: payload.id ?? null,
			error: { code: -32601, message: `Unknown method "${payload.method}"` },
		});
	} catch (error) {
		sendJson(response, 500, {
			jsonrpc: "2.0",
			id: null,
			error: {
				code: -32000,
				message: error instanceof Error ? error.message : String(error),
			},
		});
	}
});

server.listen(PORT, "127.0.0.1", () => {
	console.log(`agentctl remote MCP server listening on http://127.0.0.1:${PORT}/mcp`);
});

for (const signal of ["SIGINT", "SIGTERM"]) {
	process.on(signal, () => {
		server.close(() => {
			process.exit(0);
		});
	});
}
