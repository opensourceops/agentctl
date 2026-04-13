import { randomUUID } from "node:crypto";
import Database from "better-sqlite3";
import type {
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
				 SUM(CASE WHEN status = 'succeeded' THEN 1 ELSE 0 END) AS succeeded,
				 SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) AS failed,
				 MIN(created_at) AS oldest_created_at,
				 MAX(updated_at) AS newest_updated_at
				 FROM runs`,
			)
			.get() as {
			total: number;
			running: number | null;
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
