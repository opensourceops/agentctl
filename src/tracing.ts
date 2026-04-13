import { randomUUID } from "node:crypto";
import { trace } from "@opentelemetry/api";
import type { AuditEventRecord, JsonValue, TraceSpanKind, TraceSpanRecord, TraceStatus } from "./types.js";
import { nowIso } from "./utils.js";
import type { CheckpointStore } from "./checkpoint-store.js";

export interface TraceSink {
	onSpanStarted?(span: TraceSpanRecord): void;
	onSpanEnded?(span: TraceSpanRecord): void;
	onAuditEvent?(event: AuditEventRecord): void;
}

export class OtelTraceSink implements TraceSink {
	private readonly tracer = trace.getTracer("agentctl");

	onSpanEnded(span: TraceSpanRecord): void {
		const otelSpan = this.tracer.startSpan(span.name, {
			attributes: {
				"autonomous.run_id": span.runId,
				"autonomous.span_kind": span.kind,
				...span.attributes,
			},
			startTime: new Date(span.startedAt),
		});
		otelSpan.end(span.endedAt ? new Date(span.endedAt) : undefined);
	}
}

export class TraceRecorder {
	private nextAuditSeq = 1;

	constructor(
		private readonly runId: string,
		private readonly store: CheckpointStore,
		private readonly sinks: TraceSink[] = [],
	) {}

	startSpan(name: string, kind: TraceSpanKind, attributes: Record<string, JsonValue>, parentId?: string): ActiveSpan {
		const span: TraceSpanRecord = {
			id: randomUUID(),
			runId: this.runId,
			name,
			kind,
			status: "ok",
			attributes,
			startedAt: nowIso(),
			...(parentId ? { parentId } : {}),
		};
		this.store.recordTraceSpan(span);
		for (const sink of this.sinks) sink.onSpanStarted?.(span);
		return new ActiveSpan(span, this.store, this.sinks);
	}

	recordAudit(scope: string, name: string, level: "info" | "warning" | "error", attributes: Record<string, JsonValue>): void {
		const event: AuditEventRecord = {
			seq: this.nextAuditSeq++,
			runId: this.runId,
			scope,
			name,
			level,
			attributes,
			createdAt: nowIso(),
		};
		this.store.recordAuditEvent(event);
		for (const sink of this.sinks) sink.onAuditEvent?.(event);
	}
}

export class ActiveSpan {
	constructor(
		private span: TraceSpanRecord,
		private readonly store: CheckpointStore,
		private readonly sinks: TraceSink[],
	) {}

	end(status: TraceStatus = "ok", extraAttributes: Record<string, JsonValue> = {}): void {
		this.span = {
			...this.span,
			status,
			attributes: { ...this.span.attributes, ...extraAttributes },
			endedAt: nowIso(),
		};
		this.store.recordTraceSpan(this.span);
		for (const sink of this.sinks) sink.onSpanEnded?.(this.span);
	}

	get id(): string {
		return this.span.id;
	}
}
