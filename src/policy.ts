import { resolve } from "node:path";
import { agentProfileAllowsCapability } from "./builtin-tools.js";
import type {
	AgentProfileName,
	ApprovalMode,
	AuthorizationDecision,
	GuardrailsPolicy,
	JsonValue,
	ToolPolicySpec,
} from "./types.js";

export interface AuthorizationRequest {
	origin: "task" | "agent_tool";
	spec: ToolPolicySpec;
	input: Record<string, JsonValue>;
	agentProfile?: AgentProfileName;
}

function isPathWithinRoot(path: string, root: string): boolean {
	return path === root || path.startsWith(`${root}/`);
}

function resolveUnderWorkspace(workspaceRoot: string, value: string): string {
	return resolve(workspaceRoot, value);
}

function requiresApproval(mode: ApprovalMode, capability: AuthorizationDecision["spec"]["capability"]): boolean {
	if (capability === "internal") return false;
	if (mode === "always") return true;
	if (mode === "on-mutate") return capability === "mutate" || capability === "act";
	if (mode === "on-act") return capability === "act";
	return false;
}

export class PolicyEngine {
	private readonly workspaceRoot: string;
	private readonly writableRoots: string[];
	private readonly approvalMode: ApprovalMode;

	constructor(policy: Required<GuardrailsPolicy>) {
		this.workspaceRoot = resolve(policy.workspaceRoot);
		this.writableRoots = policy.writableRoots.map((root) => resolve(root));
		this.approvalMode = policy.approvalMode;
	}

	authorize(request: AuthorizationRequest): AuthorizationDecision {
		if (request.origin === "agent_tool" && request.agentProfile) {
			if (!agentProfileAllowsCapability(request.agentProfile, request.spec.capability)) {
				return {
					decision: "deny",
					reason: `Agent profile "${request.agentProfile}" does not allow ${request.spec.label}`,
					spec: request.spec,
				};
			}
		}

		const pathDecision = this.authorizePaths(request, request.spec);
		if (pathDecision) return pathDecision;

		if (requiresApproval(this.approvalMode, request.spec.capability)) {
			return {
				decision: "require_approval",
				reason: `${request.spec.label} requires approval under approvalMode=${this.approvalMode}`,
				spec: request.spec,
			};
		}

		return {
			decision: "allow",
			reason: `${request.spec.label} permitted`,
			spec: request.spec,
		};
	}

	private authorizePaths(request: AuthorizationRequest, decisionSpec: AuthorizationDecision["spec"]): AuthorizationDecision | undefined {
		if (decisionSpec.label === "bash") {
			const cwdValue = typeof request.input.cwd === "string" ? request.input.cwd : this.workspaceRoot;
			const cwd = resolveUnderWorkspace(this.workspaceRoot, cwdValue);
			if (!isPathWithinRoot(cwd, this.workspaceRoot)) {
				return {
					decision: "deny",
					reason: `bash cwd "${cwd}" escapes workspaceRoot`,
					spec: decisionSpec,
				};
			}
			return undefined;
		}

		if (!("path" in request.input) || typeof request.input.path !== "string") {
			return undefined;
		}

		const targetPath = resolveUnderWorkspace(this.workspaceRoot, request.input.path);
		if (decisionSpec.capability === "observe") {
			if (!isPathWithinRoot(targetPath, this.workspaceRoot)) {
				return {
					decision: "deny",
					reason: `path "${targetPath}" escapes workspaceRoot`,
					spec: decisionSpec,
				};
			}
		}

		if (decisionSpec.capability === "mutate") {
			const writable = this.writableRoots.some((root) => isPathWithinRoot(targetPath, root));
			if (!writable) {
				return {
					decision: "deny",
					reason: `path "${targetPath}" is not inside writableRoots`,
					spec: decisionSpec,
				};
			}
		}

		return undefined;
	}
}
