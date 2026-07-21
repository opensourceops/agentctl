import type {
	AgentProfileName,
	AgentSessionState,
	JsonObject,
	RuntimeSnapshot,
	ToolPolicySpec,
} from "./types.js";

export interface ApprovalRequestDraft {
	origin: "task" | "agent_tool";
	spec: ToolPolicySpec;
	input: JsonObject;
	reason: string;
	agentProfile?: AgentProfileName;
}

export interface ApprovalRuntimeState {
	session?: AgentSessionState;
	snapshot?: RuntimeSnapshot;
}

export class ApprovalRequiredError extends Error {
	readonly approval: ApprovalRequestDraft;
	readonly runtimeState?: ApprovalRuntimeState;

	constructor(approval: ApprovalRequestDraft, runtimeState?: ApprovalRuntimeState) {
		super(`Tool call requires approval: ${approval.reason}`);
		this.name = "ApprovalRequiredError";
		this.approval = approval;
		if (runtimeState) {
			this.runtimeState = runtimeState;
		}
	}
}

export class RunPausedError extends Error {
	readonly approvalId: string;

	constructor(approvalId: string) {
		super(`Run paused for approval: ${approvalId}`);
		this.name = "RunPausedError";
		this.approvalId = approvalId;
	}
}
