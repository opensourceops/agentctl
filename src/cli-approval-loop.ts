import type { ApprovalRecord, ExecutionResult, OutputFormat } from "./types.js";

export interface InteractiveApprovalPrompt {
	ask(approval: ApprovalRecord): Promise<Extract<ApprovalRecord["status"], "approved" | "rejected">>;
	close(): Promise<void>;
}

export interface InteractiveApprovalLoopOptions {
	readonly initialResult: ExecutionResult;
	readonly outputFormat: OutputFormat;
	readonly stdoutIsTty: boolean;
	readonly stdinIsTty: boolean;
	readonly listPendingApprovals: (runId: string) => ApprovalRecord[];
	readonly resolveApproval: (
		approvalId: string,
		status: Extract<ApprovalRecord["status"], "approved" | "rejected">,
	) => ApprovalRecord;
	readonly resumeRun: (runId: string) => Promise<ExecutionResult>;
	readonly writeResult: (result: ExecutionResult) => void;
	readonly writeApproval: (
		type: "approval_approve" | "approval_reject",
		approval: ApprovalRecord,
	) => void;
	readonly prompt: InteractiveApprovalPrompt;
}

export function shouldUseInteractiveApprovals(input: {
	readonly outputFormat: OutputFormat;
	readonly stdoutIsTty: boolean;
	readonly stdinIsTty: boolean;
}): boolean {
	return input.outputFormat === "yaml" && input.stdoutIsTty && input.stdinIsTty;
}

export async function runInteractiveApprovalLoop(
	options: InteractiveApprovalLoopOptions,
): Promise<ExecutionResult> {
	let result = options.initialResult;
	try {
		options.writeResult(result);
		if (
			!shouldUseInteractiveApprovals({
				outputFormat: options.outputFormat,
				stdoutIsTty: options.stdoutIsTty,
				stdinIsTty: options.stdinIsTty,
			})
		) {
			return result;
		}
		while (result.run.status === "paused") {
			const pendingApprovals = options.listPendingApprovals(result.run.id);
			if (pendingApprovals.length === 0) {
				throw new Error(`Run "${result.run.id}" is paused but has no pending approvals`);
			}
			for (const approval of pendingApprovals) {
				const nextStatus = await options.prompt.ask(approval);
				const resolved = options.resolveApproval(approval.id, nextStatus);
				options.writeApproval(nextStatus === "approved" ? "approval_approve" : "approval_reject", resolved);
			}
			result = await options.resumeRun(result.run.id);
			options.writeResult(result);
		}
		return result;
	} finally {
		await options.prompt.close();
	}
}
