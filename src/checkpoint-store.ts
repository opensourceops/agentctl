import { randomUUID } from "node:crypto";
import Database from "better-sqlite3";
import type {
	ApprovalRecord,
	AuditEventRecord,
	CheckpointRecord,
	RunRecord,
	RunStatus,
	RuntimeSnapshot,
	TaskAttemptRecord,
	TraceSpanRecord,
} from "./types.js";
import { ensureParentDir, nowIso } from "./utils.js";

interface RuntimeDbStats {
	dbPath: string;
	fileSizeBytes: number;
	runs: {
		total: number;
		running: number;
		paused: number;
		succeeded: number;
		failed: number;
		oldestCreatedAt: string | null;
		newestUpdatedAt: string | null;
	};
	records: {
		checkpoints: number;
		taskAttempts: number;
		agentTurns: number;
		auditEvents: number;
		traceSpans: number;
		approvals: number;
	};
	latestRun?: {
		id: string;
		playbookName: string;
		status: RunStatus;
		updatedAt: string;
	};
}

interface GcResult {
	deletedRunIds: string[];
	vacuumed: boolean;
}

export interface PromptCacheStats {
	dbPath: string;
	totalResponses: number;
	hitResponses: number;
	totalCachedTokens: number;
	totalInputTokens: number;
	totalUncachedInputTokens: number;
	totalOutputTokens: number;
	latestResponseAt: string | null;
	providers: Array<{
		provider: string;
		responses: number;
		hitResponses: number;
		cachedTokens: number;
		inputTokens: number;
		uncachedInputTokens: number;
		outputTokens: number;
	}>;
	agents: Array<{
		agentRef: string;
		responses: number;
		hitResponses: number;
		cachedTokens: number;
		inputTokens: number;
		uncachedInputTokens: number;
		outputTokens: number;
	}>;
	runs: Array<{
		runId: string;
		responses: number;
		hitResponses: number;
		cachedTokens: number;
		inputTokens: number;
		uncachedInputTokens: number;
		outputTokens: number;
	}>;
	responses?: Array<{
		runId: string;
		taskId: string | null;
		agentRef: string | null;
		provider: string;
		responseId: string | null;
		key: string | null;
		retention: string | null;
		cachedTokens: number;
		inputTokens: number;
		uncachedInputTokens: number;
		outputTokens: number;
		hit: boolean;
		createdAt: string;
	}>;
}

const SCHEMA_SQL = `
CREATE TABLE IF NOT EXISTS runs (
	id TEXT PRIMARY KEY,
	playbook_name TEXT NOT NULL,
	status TEXT NOT NULL,
	trace_id TEXT NOT NULL,
	snapshot_json TEXT NOT NULL,
	created_at TEXT NOT NULL,
	updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS checkpoints (
	run_id TEXT NOT NULL,
	seq INTEGER NOT NULL,
	task_id TEXT,
	status TEXT NOT NULL,
	snapshot_json TEXT NOT NULL,
	created_at TEXT NOT NULL,
	PRIMARY KEY (run_id, seq)
);

CREATE TABLE IF NOT EXISTS task_attempts (
	run_id TEXT NOT NULL,
	task_id TEXT NOT NULL,
	attempt INTEGER NOT NULL,
	status TEXT NOT NULL,
	input_json TEXT NOT NULL,
	output_json TEXT,
	error_json TEXT,
	started_at TEXT NOT NULL,
	finished_at TEXT,
	PRIMARY KEY (run_id, task_id, attempt)
);

CREATE TABLE IF NOT EXISTS agent_turns (
	run_id TEXT NOT NULL,
	task_id TEXT NOT NULL,
	attempt INTEGER NOT NULL,
	turn INTEGER NOT NULL,
	decision_json TEXT NOT NULL,
	observation_json TEXT,
	created_at TEXT NOT NULL,
	PRIMARY KEY (run_id, task_id, attempt, turn)
);

CREATE TABLE IF NOT EXISTS audit_events (
	run_id TEXT NOT NULL,
	seq INTEGER NOT NULL,
	scope TEXT NOT NULL,
	name TEXT NOT NULL,
	level TEXT NOT NULL,
	attributes_json TEXT NOT NULL,
	created_at TEXT NOT NULL,
	PRIMARY KEY (run_id, seq)
);

CREATE TABLE IF NOT EXISTS trace_spans (
	run_id TEXT NOT NULL,
	span_id TEXT NOT NULL,
	parent_span_id TEXT,
	name TEXT NOT NULL,
	kind TEXT NOT NULL,
	status TEXT NOT NULL,
	attributes_json TEXT NOT NULL,
	started_at TEXT NOT NULL,
	ended_at TEXT,
	PRIMARY KEY (run_id, span_id)
);

CREATE TABLE IF NOT EXISTS approvals (
	id TEXT PRIMARY KEY,
	run_id TEXT NOT NULL,
	task_id TEXT NOT NULL,
	origin TEXT NOT NULL,
	tool_ref TEXT NOT NULL,
	tool_provider TEXT NOT NULL,
	tool_label TEXT NOT NULL,
	capability TEXT NOT NULL,
	risk TEXT NOT NULL,
	request_input_json TEXT NOT NULL,
	reason TEXT NOT NULL,
	agent_profile TEXT,
	status TEXT NOT NULL,
	created_at TEXT NOT NULL,
	resolved_at TEXT,
	resolved_by TEXT,
	resolution_note TEXT
);
`;

function parseJson<T>(value: string): T {
	return JSON.parse(value) as T;
}

function toCheckpointRecord(row: {
	run_id: string;
	seq: number;
	task_id: string | null;
	status: RunStatus;
	snapshot_json: string;
	created_at: string;
}): CheckpointRecord {
	return {
		runId: row.run_id,
		seq: row.seq,
		status: row.status,
		snapshot: parseJson<RuntimeSnapshot>(row.snapshot_json),
		createdAt: row.created_at,
		...(row.task_id ? { taskId: row.task_id } : {}),
	};
}

export class CheckpointStore {
	private readonly db: Database.Database;

	constructor(private readonly dbPath: string) {
		ensureParentDir(dbPath);
		this.db = new Database(dbPath);
		this.db.pragma("journal_mode = WAL");
		this.db.exec(SCHEMA_SQL);
	}

	close(): void {
		this.db.close();
	}

	getDbPath(): string {
		return this.dbPath;
	}

	createRun(playbookName: string, snapshot: RuntimeSnapshot): RunRecord {
		const createdAt = nowIso();
		const run: RunRecord = {
			id: randomUUID(),
			playbookName,
			status: "running",
			snapshot,
			traceId: randomUUID(),
			createdAt,
			updatedAt: createdAt,
		};

		this.db
			.prepare(
				`INSERT INTO runs (id, playbook_name, status, trace_id, snapshot_json, created_at, updated_at)
				 VALUES (@id, @playbookName, @status, @traceId, @snapshotJson, @createdAt, @updatedAt)`,
			)
			.run({
				id: run.id,
				playbookName: run.playbookName,
				status: run.status,
				traceId: run.traceId,
				snapshotJson: JSON.stringify(run.snapshot),
				createdAt: run.createdAt,
				updatedAt: run.updatedAt,
			});

		this.saveCheckpoint(run.id, {
			seq: 1,
			runId: run.id,
			status: run.status,
			snapshot: run.snapshot,
			createdAt,
		});
		return run;
	}

	updateRun(runId: string, status: RunStatus, snapshot: RuntimeSnapshot): RunRecord {
		const updatedAt = nowIso();
		this.db
			.prepare(`UPDATE runs SET status = ?, snapshot_json = ?, updated_at = ? WHERE id = ?`)
			.run(status, JSON.stringify(snapshot), updatedAt, runId);

		return this.getRun(runId);
	}

	getRun(runId: string): RunRecord {
		const row = this.db
			.prepare(`SELECT id, playbook_name, status, trace_id, snapshot_json, created_at, updated_at FROM runs WHERE id = ?`)
			.get(runId) as
			| {
					id: string;
					playbook_name: string;
					status: RunStatus;
					trace_id: string;
					snapshot_json: string;
					created_at: string;
					updated_at: string;
			  }
			| undefined;
		if (!row) throw new Error(`Run "${runId}" not found`);
		return {
			id: row.id,
			playbookName: row.playbook_name,
			status: row.status,
			traceId: row.trace_id,
			snapshot: parseJson<RuntimeSnapshot>(row.snapshot_json),
			createdAt: row.created_at,
			updatedAt: row.updated_at,
		};
	}

	saveCheckpoint(runId: string, checkpoint: CheckpointRecord): void {
		this.db
			.prepare(
				`INSERT OR REPLACE INTO checkpoints (run_id, seq, task_id, status, snapshot_json, created_at)
				 VALUES (@runId, @seq, @taskId, @status, @snapshotJson, @createdAt)`,
			)
			.run({
				runId,
				seq: checkpoint.seq,
				taskId: checkpoint.taskId ?? null,
				status: checkpoint.status,
				snapshotJson: JSON.stringify(checkpoint.snapshot),
				createdAt: checkpoint.createdAt,
			});
	}

	getLatestCheckpoint(runId: string): CheckpointRecord {
		const row = this.db
			.prepare(
				`SELECT run_id, seq, task_id, status, snapshot_json, created_at
				 FROM checkpoints
				 WHERE run_id = ?
				 ORDER BY seq DESC
				 LIMIT 1`,
			)
			.get(runId) as
			| {
					run_id: string;
					seq: number;
					task_id: string | null;
					status: RunStatus;
					snapshot_json: string;
					created_at: string;
			  }
			| undefined;
		if (!row) throw new Error(`No checkpoints found for run "${runId}"`);
		return toCheckpointRecord(row);
	}

	getCheckpoint(runId: string, seq: number): CheckpointRecord {
		const row = this.db
			.prepare(
				`SELECT run_id, seq, task_id, status, snapshot_json, created_at
				 FROM checkpoints
				 WHERE run_id = ? AND seq = ?`,
			)
			.get(runId, seq) as
			| {
					run_id: string;
					seq: number;
					task_id: string | null;
					status: RunStatus;
					snapshot_json: string;
					created_at: string;
			  }
			| undefined;
		if (!row) throw new Error(`Checkpoint "${runId}:${seq}" not found`);
		return toCheckpointRecord(row);
	}

	listCheckpoints(runId: string): CheckpointRecord[] {
		const rows = this.db
			.prepare(
				`SELECT run_id, seq, task_id, status, snapshot_json, created_at
				 FROM checkpoints
				 WHERE run_id = ?
				 ORDER BY seq ASC`,
			)
			.all(runId) as Array<{
			run_id: string;
			seq: number;
			task_id: string | null;
			status: RunStatus;
			snapshot_json: string;
			created_at: string;
		}>;
		return rows.map((row) => toCheckpointRecord(row));
	}

	createApproval(record: Omit<ApprovalRecord, "id" | "createdAt" | "status" | "resolvedAt" | "resolvedBy" | "resolutionNote">): ApprovalRecord {
		const approval: ApprovalRecord = {
			id: randomUUID(),
			status: "pending",
			createdAt: nowIso(),
			...record,
		};
		this.db
			.prepare(
				`INSERT INTO approvals
				 (id, run_id, task_id, origin, tool_ref, tool_provider, tool_label, capability, risk, request_input_json, reason, agent_profile, status, created_at, resolved_at, resolved_by, resolution_note)
				 VALUES (@id, @runId, @taskId, @origin, @toolRef, @toolProvider, @toolLabel, @capability, @risk, @requestInputJson, @reason, @agentProfile, @status, @createdAt, NULL, NULL, NULL)`,
			)
			.run({
				id: approval.id,
				runId: approval.runId,
				taskId: approval.taskId,
				origin: approval.origin,
				toolRef: approval.toolRef,
				toolProvider: approval.toolProvider,
				toolLabel: approval.toolLabel,
				capability: approval.capability,
				risk: approval.risk,
				requestInputJson: JSON.stringify(approval.requestInput),
				reason: approval.reason,
				agentProfile: approval.agentProfile ?? null,
				status: approval.status,
				createdAt: approval.createdAt,
			});
		return approval;
	}

	getApproval(approvalId: string): ApprovalRecord {
		const row = this.db
			.prepare(
				`SELECT id, run_id, task_id, origin, tool_ref, tool_provider, tool_label, capability, risk, request_input_json, reason, agent_profile, status, created_at, resolved_at, resolved_by, resolution_note
				 FROM approvals
				 WHERE id = ?`,
			)
			.get(approvalId) as
			| {
					id: string;
					run_id: string;
					task_id: string;
					origin: ApprovalRecord["origin"];
					tool_ref: string;
					tool_provider: ApprovalRecord["toolProvider"];
					tool_label: string;
					capability: ApprovalRecord["capability"];
					risk: ApprovalRecord["risk"];
					request_input_json: string;
					reason: string;
					agent_profile: ApprovalRecord["agentProfile"] | null;
					status: ApprovalRecord["status"];
					created_at: string;
					resolved_at: string | null;
					resolved_by: string | null;
					resolution_note: string | null;
			  }
			| undefined;
		if (!row) {
			throw new Error(`Approval "${approvalId}" not found`);
		}
		return {
			id: row.id,
			runId: row.run_id,
			taskId: row.task_id,
			origin: row.origin,
			toolRef: row.tool_ref,
			toolProvider: row.tool_provider,
			toolLabel: row.tool_label,
			capability: row.capability,
			risk: row.risk,
			requestInput: parseJson(row.request_input_json),
			reason: row.reason,
			...(row.agent_profile ? { agentProfile: row.agent_profile } : {}),
			status: row.status,
			createdAt: row.created_at,
			...(row.resolved_at ? { resolvedAt: row.resolved_at } : {}),
			...(row.resolved_by ? { resolvedBy: row.resolved_by } : {}),
			...(row.resolution_note ? { resolutionNote: row.resolution_note } : {}),
		};
	}

	listApprovals(filters: { runId?: string; status?: ApprovalRecord["status"] } = {}): ApprovalRecord[] {
		const conditions: string[] = [];
		const values: string[] = [];
		if (filters.runId) {
			conditions.push("run_id = ?");
			values.push(filters.runId);
		}
		if (filters.status) {
			conditions.push("status = ?");
			values.push(filters.status);
		}
		const whereClause = conditions.length > 0 ? `WHERE ${conditions.join(" AND ")}` : "";
		const rows = this.db
			.prepare(
				`SELECT id, run_id, task_id, origin, tool_ref, tool_provider, tool_label, capability, risk, request_input_json, reason, agent_profile, status, created_at, resolved_at, resolved_by, resolution_note
				 FROM approvals
				 ${whereClause}
				 ORDER BY created_at ASC`,
			)
			.all(...values) as Array<{
			id: string;
			run_id: string;
			task_id: string;
			origin: ApprovalRecord["origin"];
			tool_ref: string;
			tool_provider: ApprovalRecord["toolProvider"];
			tool_label: string;
			capability: ApprovalRecord["capability"];
			risk: ApprovalRecord["risk"];
			request_input_json: string;
			reason: string;
			agent_profile: ApprovalRecord["agentProfile"] | null;
			status: ApprovalRecord["status"];
			created_at: string;
			resolved_at: string | null;
			resolved_by: string | null;
			resolution_note: string | null;
		}>;
		return rows.map((row) => ({
			id: row.id,
			runId: row.run_id,
			taskId: row.task_id,
			origin: row.origin,
			toolRef: row.tool_ref,
			toolProvider: row.tool_provider,
			toolLabel: row.tool_label,
			capability: row.capability,
			risk: row.risk,
			requestInput: parseJson(row.request_input_json),
			reason: row.reason,
			...(row.agent_profile ? { agentProfile: row.agent_profile } : {}),
			status: row.status,
			createdAt: row.created_at,
			...(row.resolved_at ? { resolvedAt: row.resolved_at } : {}),
			...(row.resolved_by ? { resolvedBy: row.resolved_by } : {}),
			...(row.resolution_note ? { resolutionNote: row.resolution_note } : {}),
		}));
	}

	resolveApproval(
		approvalId: string,
		status: Extract<ApprovalRecord["status"], "approved" | "rejected">,
		options: { resolvedBy?: string; resolutionNote?: string } = {},
	): ApprovalRecord {
		const current = this.getApproval(approvalId);
		if (current.status !== "pending") {
			if (current.status === status) {
				return current;
			}
			throw new Error(`Approval "${approvalId}" is already ${current.status}`);
		}
		const resolvedAt = nowIso();
		this.db
			.prepare(
				`UPDATE approvals
				 SET status = ?, resolved_at = ?, resolved_by = ?, resolution_note = ?
				 WHERE id = ?`,
			)
			.run(status, resolvedAt, options.resolvedBy ?? null, options.resolutionNote ?? null, approvalId);
		return this.getApproval(approvalId);
	}

	listAuditEvents(runId: string): AuditEventRecord[] {
		const rows = this.db
			.prepare(
				`SELECT run_id, seq, scope, name, level, attributes_json, created_at
				 FROM audit_events
				 WHERE run_id = ?
				 ORDER BY seq ASC`,
			)
			.all(runId) as Array<{
			run_id: string;
			seq: number;
			scope: string;
			name: string;
			level: AuditEventRecord["level"];
			attributes_json: string;
			created_at: string;
		}>;
		return rows.map((row) => ({
			runId: row.run_id,
			seq: row.seq,
			scope: row.scope,
			name: row.name,
			level: row.level,
			attributes: parseJson(row.attributes_json),
			createdAt: row.created_at,
		}));
	}

	recordTaskAttemptForRun(runId: string, record: TaskAttemptRecord): void {
		this.db
			.prepare(
				`INSERT OR REPLACE INTO task_attempts
				 (run_id, task_id, attempt, status, input_json, output_json, error_json, started_at, finished_at)
				 VALUES (@runId, @taskId, @attempt, @status, @inputJson, @outputJson, @errorJson, @startedAt, @finishedAt)`,
			)
			.run({
				runId,
				taskId: record.taskId,
				attempt: record.attempt,
				status: record.status,
				inputJson: JSON.stringify(record.input),
				outputJson: record.output ? JSON.stringify(record.output) : null,
				errorJson: record.error ? JSON.stringify({ message: record.error }) : null,
				startedAt: record.startedAt,
				finishedAt: record.finishedAt ?? null,
			});
	}

	recordAgentTurn(
		runId: string,
		taskId: string,
		attempt: number,
		turn: number,
		decisionJson: string,
		observationJson: string | null,
	): void {
		this.db
			.prepare(
				`INSERT OR REPLACE INTO agent_turns
				 (run_id, task_id, attempt, turn, decision_json, observation_json, created_at)
				 VALUES (?, ?, ?, ?, ?, ?, ?)`,
			)
			.run(runId, taskId, attempt, turn, decisionJson, observationJson, nowIso());
	}

	recordAuditEvent(event: AuditEventRecord): void {
		this.db
			.prepare(
				`INSERT OR REPLACE INTO audit_events
				 (run_id, seq, scope, name, level, attributes_json, created_at)
				 VALUES (?, ?, ?, ?, ?, ?, ?)`,
			)
			.run(
				event.runId,
				event.seq,
				event.scope,
				event.name,
				event.level,
				JSON.stringify(event.attributes),
				event.createdAt,
			);
	}

	recordTraceSpan(span: TraceSpanRecord): void {
		this.db
			.prepare(
				`INSERT OR REPLACE INTO trace_spans
				 (run_id, span_id, parent_span_id, name, kind, status, attributes_json, started_at, ended_at)
				 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
			)
			.run(
				span.runId,
				span.id,
				span.parentId ?? null,
				span.name,
				span.kind,
				span.status,
				JSON.stringify(span.attributes),
				span.startedAt,
				span.endedAt ?? null,
			);
	}

	listTaskAttempts(runId: string): Array<{
		taskId: string;
		attempt: number;
		status: string;
		outputJson: string | null;
		errorJson: string | null;
	}> {
		return this.db
			.prepare(
				`SELECT task_id as taskId, attempt, status, output_json as outputJson, error_json as errorJson
				 FROM task_attempts
				 WHERE run_id = ?
				 ORDER BY task_id, attempt`,
			)
			.all(runId) as Array<{
			taskId: string;
			attempt: number;
			status: string;
			outputJson: string | null;
			errorJson: string | null;
		}>;
	}

	createReplayRun(sourceRunId: string, sourceSeq: number): RunRecord {
		const sourceRun = this.getRun(sourceRunId);
		const checkpoint = this.getCheckpoint(sourceRunId, sourceSeq);
		return this.createRun(`${sourceRun.playbookName}:replay`, checkpoint.snapshot);
	}

	getStats(): RuntimeDbStats {
		const runCounts = this.db
			.prepare(
				`SELECT
				 COUNT(*) AS total,
				 SUM(CASE WHEN status = 'running' THEN 1 ELSE 0 END) AS running,
				 SUM(CASE WHEN status = 'paused' THEN 1 ELSE 0 END) AS paused,
				 SUM(CASE WHEN status = 'succeeded' THEN 1 ELSE 0 END) AS succeeded,
				 SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) AS failed,
				 MIN(created_at) AS oldest_created_at,
				 MAX(updated_at) AS newest_updated_at
				 FROM runs`,
			)
			.get() as {
			total: number;
			running: number | null;
			paused: number | null;
			succeeded: number | null;
			failed: number | null;
			oldest_created_at: string | null;
			newest_updated_at: string | null;
		};
		const counts = {
			checkpoints: this.getSingleCount("checkpoints"),
			taskAttempts: this.getSingleCount("task_attempts"),
			agentTurns: this.getSingleCount("agent_turns"),
			auditEvents: this.getSingleCount("audit_events"),
			traceSpans: this.getSingleCount("trace_spans"),
			approvals: this.getSingleCount("approvals"),
		};
		const latestRunRow = this.db
			.prepare(
				`SELECT id, playbook_name, status, updated_at
				 FROM runs
				 ORDER BY updated_at DESC
				 LIMIT 1`,
			)
			.get() as
			| {
					id: string;
					playbook_name: string;
					status: RunStatus;
					updated_at: string;
			  }
				| undefined;
		const pageCount = this.db.pragma("page_count", { simple: true }) as number;
		const pageSize = this.db.pragma("page_size", { simple: true }) as number;
		return {
			dbPath: this.dbPath,
			fileSizeBytes: pageCount * pageSize,
			runs: {
				total: runCounts.total,
				running: runCounts.running ?? 0,
				paused: runCounts.paused ?? 0,
				succeeded: runCounts.succeeded ?? 0,
				failed: runCounts.failed ?? 0,
				oldestCreatedAt: runCounts.oldest_created_at,
				newestUpdatedAt: runCounts.newest_updated_at,
			},
			records: counts,
			...(latestRunRow
				? {
						latestRun: {
							id: latestRunRow.id,
							playbookName: latestRunRow.playbook_name,
							status: latestRunRow.status,
							updatedAt: latestRunRow.updated_at,
						},
					}
				: {}),
		};
	}

	getPromptCacheStats(filters: {
		runId?: string;
		taskId?: string;
		agentRef?: string;
		verbose?: boolean;
	} = {}): PromptCacheStats {
		const where: string[] = ["scope = 'prompt_cache'", "name = 'prompt_cache.response'"];
		const values: Array<string> = [];
		if (filters.runId) {
			where.push("run_id = ?");
			values.push(filters.runId);
		}
		if (filters.taskId) {
			where.push("json_extract(attributes_json, '$.task_id') = ?");
			values.push(filters.taskId);
		}
		if (filters.agentRef) {
			where.push("json_extract(attributes_json, '$.agent_ref') = ?");
			values.push(filters.agentRef);
		}
		const whereClause = where.join(" AND ");
		const summaryRow = this.db
			.prepare(
				`SELECT
				 COUNT(*) AS total_responses,
				 SUM(CASE WHEN json_extract(attributes_json, '$.hit') = 1 THEN 1 ELSE 0 END) AS hit_responses,
				 SUM(COALESCE(json_extract(attributes_json, '$.cached_tokens'), 0)) AS total_cached_tokens,
				 SUM(COALESCE(json_extract(attributes_json, '$.input_tokens'), 0)) AS total_input_tokens,
				 SUM(COALESCE(json_extract(attributes_json, '$.uncached_input_tokens'), 0)) AS total_uncached_input_tokens,
				 SUM(COALESCE(json_extract(attributes_json, '$.output_tokens'), 0)) AS total_output_tokens,
				 MAX(created_at) AS latest_response_at
				 FROM audit_events
				 WHERE ${whereClause}`,
			)
			.get(...values) as {
			total_responses: number;
			hit_responses: number | null;
			total_cached_tokens: number | null;
			total_input_tokens: number | null;
			total_uncached_input_tokens: number | null;
			total_output_tokens: number | null;
			latest_response_at: string | null;
		};
		const providerRows = this.db
			.prepare(
				`SELECT
				 COALESCE(json_extract(attributes_json, '$.provider'), 'unknown') AS provider,
				 COUNT(*) AS responses,
				 SUM(CASE WHEN json_extract(attributes_json, '$.hit') = 1 THEN 1 ELSE 0 END) AS hit_responses,
				 SUM(COALESCE(json_extract(attributes_json, '$.cached_tokens'), 0)) AS cached_tokens,
				 SUM(COALESCE(json_extract(attributes_json, '$.input_tokens'), 0)) AS input_tokens,
				 SUM(COALESCE(json_extract(attributes_json, '$.uncached_input_tokens'), 0)) AS uncached_input_tokens,
				 SUM(COALESCE(json_extract(attributes_json, '$.output_tokens'), 0)) AS output_tokens
				 FROM audit_events
				 WHERE ${whereClause}
				 GROUP BY provider
				 ORDER BY provider ASC`,
			)
			.all(...values) as Array<{
			provider: string;
			responses: number;
			hit_responses: number | null;
			cached_tokens: number | null;
			input_tokens: number | null;
			uncached_input_tokens: number | null;
			output_tokens: number | null;
		}>;
		const agentRows = this.db
			.prepare(
				`SELECT
				 COALESCE(json_extract(attributes_json, '$.agent_ref'), 'unknown') AS agent_ref,
				 COUNT(*) AS responses,
				 SUM(CASE WHEN json_extract(attributes_json, '$.hit') = 1 THEN 1 ELSE 0 END) AS hit_responses,
				 SUM(COALESCE(json_extract(attributes_json, '$.cached_tokens'), 0)) AS cached_tokens,
				 SUM(COALESCE(json_extract(attributes_json, '$.input_tokens'), 0)) AS input_tokens,
				 SUM(COALESCE(json_extract(attributes_json, '$.uncached_input_tokens'), 0)) AS uncached_input_tokens,
				 SUM(COALESCE(json_extract(attributes_json, '$.output_tokens'), 0)) AS output_tokens
				 FROM audit_events
				 WHERE ${whereClause}
				 GROUP BY agent_ref
				 ORDER BY agent_ref ASC`,
			)
			.all(...values) as Array<{
			agent_ref: string;
			responses: number;
			hit_responses: number | null;
			cached_tokens: number | null;
			input_tokens: number | null;
			uncached_input_tokens: number | null;
			output_tokens: number | null;
		}>;
		const runRows = this.db
			.prepare(
				`SELECT
				 run_id,
				 COUNT(*) AS responses,
				 SUM(CASE WHEN json_extract(attributes_json, '$.hit') = 1 THEN 1 ELSE 0 END) AS hit_responses,
				 SUM(COALESCE(json_extract(attributes_json, '$.cached_tokens'), 0)) AS cached_tokens,
				 SUM(COALESCE(json_extract(attributes_json, '$.input_tokens'), 0)) AS input_tokens,
				 SUM(COALESCE(json_extract(attributes_json, '$.uncached_input_tokens'), 0)) AS uncached_input_tokens,
				 SUM(COALESCE(json_extract(attributes_json, '$.output_tokens'), 0)) AS output_tokens
				 FROM audit_events
				 WHERE ${whereClause}
				 GROUP BY run_id
				 ORDER BY run_id ASC`,
			)
			.all(...values) as Array<{
			run_id: string;
			responses: number;
			hit_responses: number | null;
			cached_tokens: number | null;
			input_tokens: number | null;
			uncached_input_tokens: number | null;
			output_tokens: number | null;
		}>;
		const responseRows = filters.verbose
			? (this.db
					.prepare(
						`SELECT
						 run_id,
						 json_extract(attributes_json, '$.task_id') AS task_id,
						 json_extract(attributes_json, '$.agent_ref') AS agent_ref,
						 COALESCE(json_extract(attributes_json, '$.provider'), 'unknown') AS provider,
						 json_extract(attributes_json, '$.response_id') AS response_id,
						 json_extract(attributes_json, '$.key') AS key,
						 json_extract(attributes_json, '$.retention') AS retention,
						 COALESCE(json_extract(attributes_json, '$.cached_tokens'), 0) AS cached_tokens,
						 COALESCE(json_extract(attributes_json, '$.input_tokens'), 0) AS input_tokens,
						 COALESCE(json_extract(attributes_json, '$.uncached_input_tokens'), 0) AS uncached_input_tokens,
						 COALESCE(json_extract(attributes_json, '$.output_tokens'), 0) AS output_tokens,
						 COALESCE(json_extract(attributes_json, '$.hit'), 0) AS hit,
						 created_at
						 FROM audit_events
						 WHERE ${whereClause}
						 ORDER BY created_at ASC`,
					)
					.all(...values) as Array<{
					run_id: string;
					task_id: string | null;
					agent_ref: string | null;
					provider: string;
					response_id: string | null;
					key: string | null;
					retention: string | null;
					cached_tokens: number;
					input_tokens: number;
					uncached_input_tokens: number;
					output_tokens: number;
					hit: number;
					created_at: string;
				}>)
			: [];

		return {
			dbPath: this.dbPath,
			totalResponses: summaryRow.total_responses,
			hitResponses: summaryRow.hit_responses ?? 0,
			totalCachedTokens: summaryRow.total_cached_tokens ?? 0,
			totalInputTokens: summaryRow.total_input_tokens ?? 0,
			totalUncachedInputTokens: summaryRow.total_uncached_input_tokens ?? 0,
			totalOutputTokens: summaryRow.total_output_tokens ?? 0,
			latestResponseAt: summaryRow.latest_response_at,
			providers: providerRows.map((row) => ({
				provider: row.provider,
				responses: row.responses,
				hitResponses: row.hit_responses ?? 0,
				cachedTokens: row.cached_tokens ?? 0,
				inputTokens: row.input_tokens ?? 0,
				uncachedInputTokens: row.uncached_input_tokens ?? 0,
				outputTokens: row.output_tokens ?? 0,
			})),
			agents: agentRows.map((row) => ({
				agentRef: row.agent_ref,
				responses: row.responses,
				hitResponses: row.hit_responses ?? 0,
				cachedTokens: row.cached_tokens ?? 0,
				inputTokens: row.input_tokens ?? 0,
				uncachedInputTokens: row.uncached_input_tokens ?? 0,
				outputTokens: row.output_tokens ?? 0,
			})),
			runs: runRows.map((row) => ({
				runId: row.run_id,
				responses: row.responses,
				hitResponses: row.hit_responses ?? 0,
				cachedTokens: row.cached_tokens ?? 0,
				inputTokens: row.input_tokens ?? 0,
				uncachedInputTokens: row.uncached_input_tokens ?? 0,
				outputTokens: row.output_tokens ?? 0,
			})),
			...(filters.verbose
				? {
						responses: responseRows.map((row) => ({
							runId: row.run_id,
							taskId: row.task_id,
							agentRef: row.agent_ref,
							provider: row.provider,
							responseId: row.response_id,
							key: row.key,
							retention: row.retention,
							cachedTokens: row.cached_tokens,
							inputTokens: row.input_tokens,
							uncachedInputTokens: row.uncached_input_tokens,
							outputTokens: row.output_tokens,
							hit: Boolean(row.hit),
							createdAt: row.created_at,
						})),
					}
				: {}),
		};
	}

	garbageCollect(options: { olderThanDays: number; keepRuns: number; vacuum?: boolean }): GcResult {
		const cutoff = new Date(Date.now() - options.olderThanDays * 24 * 60 * 60 * 1000).toISOString();
		const keepTerminalRunIds = this.db
			.prepare(
				`SELECT id
				 FROM runs
				 WHERE status IN ('succeeded', 'failed')
				 ORDER BY updated_at DESC
				 LIMIT ?`,
			)
			.all(options.keepRuns) as Array<{ id: string }>;
		const keepSet = new Set(keepTerminalRunIds.map((row) => row.id));
		const candidates = this.db
			.prepare(
				`SELECT id
				 FROM runs
				 WHERE status IN ('succeeded', 'failed')
				   AND updated_at < ?
				 ORDER BY updated_at ASC`,
			)
			.all(cutoff) as Array<{ id: string }>;
		const deleteIds = candidates.map((row) => row.id).filter((id) => !keepSet.has(id));
		if (deleteIds.length === 0) {
			return { deletedRunIds: [], vacuumed: false };
		}

		const deleteMany = (table: string): void => {
			const placeholders = deleteIds.map(() => "?").join(", ");
			this.db.prepare(`DELETE FROM ${table} WHERE run_id IN (${placeholders})`).run(...deleteIds);
		};

		const transaction = this.db.transaction(() => {
			deleteMany("checkpoints");
			deleteMany("task_attempts");
			deleteMany("agent_turns");
			deleteMany("audit_events");
			deleteMany("trace_spans");
			deleteMany("approvals");
			const placeholders = deleteIds.map(() => "?").join(", ");
			this.db.prepare(`DELETE FROM runs WHERE id IN (${placeholders})`).run(...deleteIds);
		});
		transaction();

		let vacuumed = false;
		if (options.vacuum ?? true) {
			this.db.exec("VACUUM");
			vacuumed = true;
		}

		return {
			deletedRunIds: deleteIds,
			vacuumed,
		};
	}

	private getSingleCount(table: string): number {
		const row = this.db.prepare(`SELECT COUNT(*) AS count FROM ${table}`).get() as { count: number };
		return row.count;
	}
}
