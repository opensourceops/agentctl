import { describe, expect, test } from "vitest";
import { runInteractiveApprovalLoop, shouldUseInteractiveApprovals } from "../src/cli-approval-loop.js";
import type { ApprovalRecord, ExecutionResult } from "../src/types.js";

function createPausedResult(runId = "run-1", approvalId = "approval-1"): ExecutionResult {
	return {
		run: {
			id: runId,
			playbookName: "approval-playbook",
			status: "paused",
			traceId: "trace-1",
			snapshot: {
				inputs: {},
				vars: {},
				memory: {
					working: {},
				},
				tasks: {
					write_note: {
						status: "waiting_approval",
						attempts: 1,
						approvalId,
					},
				},
				agents: {},
			},
			createdAt: "2026-04-14T00:00:00.000Z",
			updatedAt: "2026-04-14T00:00:00.000Z",
		},
		latestCheckpoint: {
			runId,
			seq: 3,
			status: "paused",
			snapshot: {
				inputs: {},
				vars: {},
				memory: {
					working: {},
				},
				tasks: {
					write_note: {
						status: "waiting_approval",
						attempts: 1,
						approvalId,
					},
				},
				agents: {},
			},
			createdAt: "2026-04-14T00:00:00.000Z",
			taskId: "write_note",
		},
	};
}

function createApproval(id = "approval-1", runId = "run-1"): ApprovalRecord {
	return {
		id,
		runId,
		taskId: "write_note",
		origin: "task",
		toolRef: "builtin.write",
		toolProvider: "builtin",
		toolLabel: "write",
		capability: "mutate",
		risk: "medium",
		requestInput: {
			path: "./approved.txt",
			content: "approved",
		},
		reason: "write requires approval under approvalMode=on-mutate",
		status: "pending",
		createdAt: "2026-04-14T00:00:00.000Z",
	};
}

describe("cli approval loop", () => {
	test("interactive approvals are only enabled for yaml TTY sessions", () => {
		expect(shouldUseInteractiveApprovals({ outputFormat: "yaml", stdoutIsTty: true, stdinIsTty: true })).toBe(true);
		expect(shouldUseInteractiveApprovals({ outputFormat: "json", stdoutIsTty: true, stdinIsTty: true })).toBe(false);
		expect(shouldUseInteractiveApprovals({ outputFormat: "yaml", stdoutIsTty: false, stdinIsTty: true })).toBe(false);
		expect(shouldUseInteractiveApprovals({ outputFormat: "yaml", stdoutIsTty: true, stdinIsTty: false })).toBe(false);
	});

	test("approved paused runs resolve approvals and resume until success", async () => {
		const outputs: string[] = [];
		const approval = createApproval();
		const finalResult: ExecutionResult = {
			...createPausedResult(),
			run: {
				...createPausedResult().run,
				status: "succeeded",
				snapshot: {
					...createPausedResult().run.snapshot,
					tasks: {
						write_note: {
							status: "succeeded",
							attempts: 1,
							output: {
								path: "/tmp/approved.txt",
								bytesWritten: 8,
							},
						},
					},
				},
			},
			latestCheckpoint: {
				...createPausedResult().latestCheckpoint,
				status: "succeeded",
			},
		};

		const result = await runInteractiveApprovalLoop({
			initialResult: createPausedResult(),
			outputFormat: "yaml",
			stdoutIsTty: true,
			stdinIsTty: true,
			listPendingApprovals: () => [approval],
			resolveApproval: (_approvalId, status) => ({ ...approval, status, resolvedBy: "interactive-cli", resolvedAt: "2026-04-14T00:01:00.000Z" }),
			resumeRun: async () => finalResult,
			writeResult: (execution) => outputs.push(`result:${execution.run.status}`),
			writeApproval: (type, resolved) => outputs.push(`${type}:${resolved.status}`),
			prompt: {
				async ask() {
					return "approved";
				},
				async close() {},
			},
		});

		expect(result.run.status).toBe("succeeded");
		expect(outputs).toEqual(["result:paused", "approval_approve:approved", "result:succeeded"]);
	});

	test("rejected paused runs resolve approvals and resume into failure", async () => {
		const outputs: string[] = [];
		const approval = createApproval();
		const failedResult: ExecutionResult = {
			...createPausedResult(),
			run: {
				...createPausedResult().run,
				status: "failed",
				snapshot: {
					...createPausedResult().run.snapshot,
					tasks: {
						write_note: {
							status: "failed",
							attempts: 1,
							error: "Tool call rejected: write requires approval under approvalMode=on-mutate",
						},
					},
				},
			},
			latestCheckpoint: {
				...createPausedResult().latestCheckpoint,
				status: "failed",
			},
		};

		const result = await runInteractiveApprovalLoop({
			initialResult: createPausedResult(),
			outputFormat: "yaml",
			stdoutIsTty: true,
			stdinIsTty: true,
			listPendingApprovals: () => [approval],
			resolveApproval: (_approvalId, status) => ({ ...approval, status, resolvedBy: "interactive-cli", resolvedAt: "2026-04-14T00:01:00.000Z" }),
			resumeRun: async () => failedResult,
			writeResult: (execution) => outputs.push(`result:${execution.run.status}`),
			writeApproval: (type, resolved) => outputs.push(`${type}:${resolved.status}`),
			prompt: {
				async ask() {
					return "rejected";
				},
				async close() {},
			},
		});

		expect(result.run.status).toBe("failed");
		expect(outputs).toEqual(["result:paused", "approval_reject:rejected", "result:failed"]);
	});

	test("paused runs without pending approvals throw instead of looping silently", async () => {
		await expect(
			runInteractiveApprovalLoop({
				initialResult: createPausedResult(),
				outputFormat: "yaml",
				stdoutIsTty: true,
				stdinIsTty: true,
				listPendingApprovals: () => [],
				resolveApproval: () => {
					throw new Error("should not resolve");
				},
				resumeRun: async () => {
					throw new Error("should not resume");
				},
				writeResult: () => {},
				writeApproval: () => {},
				prompt: {
					async ask() {
						throw new Error("should not ask");
					},
					async close() {},
				},
			}),
		).rejects.toThrow('Run "run-1" is paused but has no pending approvals');
	});

	test("non-interactive runs still close the prompt resource and do not try to resolve approvals", async () => {
		let closed = false;
		let resolved = false;
		const result = await runInteractiveApprovalLoop({
			initialResult: createPausedResult(),
			outputFormat: "json",
			stdoutIsTty: false,
			stdinIsTty: false,
			listPendingApprovals: () => [createApproval()],
			resolveApproval: () => {
				resolved = true;
				throw new Error("should not resolve");
			},
			resumeRun: async () => {
				throw new Error("should not resume");
			},
			writeResult: () => {},
			writeApproval: () => {},
			prompt: {
				async ask() {
					throw new Error("should not ask");
				},
				async close() {
					closed = true;
				},
			},
		});

		expect(result.run.status).toBe("paused");
		expect(resolved).toBe(false);
		expect(closed).toBe(true);
	});
});
