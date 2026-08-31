import type { ChatMessage, Segment, TurnUsage } from "./types";
/**
 * Chat transcript helpers. An assistant message is an ordered `Segment[]`
 * (reasoning → tool → … → text). These helpers mutate/produce segment arrays
 * for the streaming event listener and migrate legacy persisted messages.
 *
 * 相比早期数据湖版本：删除 pushChart；mergeToolResult 的 result 参数去掉 sql/table，
 * 改为 detail/payload；normalizeMessage 删除对旧 ChatCard 的迁移逻辑。
 */

let segSeq = 0;
/** Stable-ish unique id for a new segment. */
export function newSegmentId(prefix: "r" | "t" | "tool" | "txt"): string {
  segSeq += 1;
  return `seg-${prefix}-${Date.now()}-${segSeq}`;
}

/**
 * Append a reasoning/text delta to the message's segment list.
 *
 * - If the last segment is the same type, append the delta into it.
 * - Otherwise push a new segment of that type (this implicitly "closes" the
 *   previous segment — e.g. a tool_call arriving after reasoning ends the
 *   reasoning run).
 *
 * Returns a NEW segments array (immutably, for SolidJS reactivity).
 */
export function appendDelta(
  segments: Segment[],
  type: "reasoning" | "text",
  delta: string,
): Segment[] {
  if (!delta) return segments;
  const next = [...segments];
  const last = next[next.length - 1];
  if (last && last.type === type) {
    const updated = { ...last, text: last.text + delta } as any;
    if (type === "reasoning" && (last as any).startTime) {
      updated.elapsedMs = Date.now() - (last as any).startTime;
    }
    next[next.length - 1] = updated;
  } else {
    // If the previous segment was reasoning, set its final elapsedMs
    if (last && last.type === "reasoning" && (last as any).startTime && !last.elapsedMs) {
      next[next.length - 1] = { ...last, elapsedMs: Date.now() - (last as any).startTime };
    }
    next.push({
      type,
      id: newSegmentId(type === "reasoning" ? "r" : "txt"),
      text: delta,
      ...(type === "reasoning" ? { startTime: Date.now() } : {}),
    } as any);
  }
  return next;
}

/** Push a new tool segment (status: running). Called on a `tool_call` event. */
export function pushToolCall(
  segments: Segment[],
  seg: { id: string; tool: string; args?: unknown },
): Segment[] {
  const next = [...segments];
  const last = next[next.length - 1];
  if (last && last.type === "reasoning" && last.startTime && !last.elapsedMs) {
    next[next.length - 1] = { ...last, elapsedMs: Date.now() - last.startTime };
  }
  return [
    ...next,
    {
      type: "tool",
      id: seg.id,
      tool: seg.tool,
      args: seg.args,
      status: "running",
      startTime: Date.now(),
    },
  ];
}

/**
 * Merge a `tool_result` into the matching tool segment by id (status →
 * ok|error|awaiting, attach summary/detail/payload/elapsedMs). No-op if the
 * id is unknown. `awaiting` is the intermediate state in 变更前确认 mode before
 * the user confirms or cancels.
 */
export function mergeToolResult(
  segments: Segment[],
  result: {
    id: string;
    status: "ok" | "error" | "awaiting" | "running";
    summary?: string;
    detail?: string;
    payload?: unknown;
    elapsedMs?: number;
    result?: string;
    meta?: unknown;
  },
): Segment[] {
  const idx = segments.findIndex(
    (s) => s.type === "tool" && s.id === result.id,
  );
  if (idx < 0) return segments;
  const cur = segments[idx];
  if (cur.type !== "tool") return segments;
  const next = [...segments];
  next[idx] = {
    ...cur,
    status: result.status,
    summary: result.summary ?? cur.summary,
    detail: result.detail ?? cur.detail,
    payload: result.payload ?? cur.payload,
    elapsedMs: result.elapsedMs ?? cur.elapsedMs,
    result: result.result ?? cur.result,
    meta: result.meta ?? cur.meta,
  };
  return next;
}

/**
 * Normalize a persisted/loaded message. Messages that already have `segments`
 * are returned as-is (typed as Segment[]). User messages with a bare `content`
 * (legacy or simple shape) become a single text segment.
 *
 * Tolerates partially-shaped objects (the backend load gives us raw JSON).
 * 相比早期数据湖版本：删除对旧 ChatCard 卡片数组的迁移逻辑（无历史数据湖卡片）。
 */
export function normalizeMessage(raw: any): ChatMessage {
  if (raw && Array.isArray(raw.segments)) {
    return {
      id: String(raw.id ?? `msg-${Date.now()}`),
      role: raw.role === "user" ? "user" : "assistant",
      segments: raw.segments as Segment[],
      ts: Number(raw.ts ?? Date.now()),
      ...(raw.turnUsage != null ? { turnUsage: raw.turnUsage as TurnUsage } : {}),
    };
  }

  // Simple shape: synthesize a single text segment from content.
  const segments: Segment[] = [];
  const reasoning: string | undefined = raw?.reasoning;
  if (reasoning && reasoning.length > 0) {
    segments.push({ type: "reasoning", id: newSegmentId("r"), text: reasoning });
  }
  const content: string | undefined = raw?.content;
  if (content && content.length > 0) {
    segments.push({ type: "text", id: newSegmentId("txt"), text: content });
  }

  return {
    id: String(raw?.id ?? `msg-${Date.now()}`),
    role: raw?.role === "user" ? "user" : "assistant",
    segments,
    ts: Number(raw?.ts ?? Date.now()),
  };
}

// ---------------------------------------------------------------------------
// 历史轮次过程折叠（借鉴 deepseek-harness web turn-process folding）
//
// 一条完成的 assistant 消息里，最终结论之前的 reasoning/tool/中间文本都属于
// 「过程」：默认折叠成一行摘要控件，结论与图表/错误保持可见。折叠的段落
// 不再渲染（返回 null），长对话的 DOM 与内存随历史轮次线性下降——这是借鉴
// 的主要动机之一（daw 的渲染债前科）。
// ---------------------------------------------------------------------------

/** 一条 assistant 消息的过程折叠判定结果。 */
export interface TurnProcess {
  /** 折叠控件的锚点段 id（过程组第一段的 id，控件渲染在它的槽位）。 */
  anchorId: string;
  /** 应折叠的段 id 集合（reasoning/tool/边界前的中间文本）。 */
  foldedIds: Set<string>;
  /** 推理总耗时（毫秒）。任一段缺 elapsedMs 时为 undefined（旧数据），不显示时间部分。 */
  reasoningMs?: number;
  /** 工具调用次数。 */
  toolCount: number;
}

/**
 * 判定一条 assistant 消息是否可折叠，并算出折叠集合与摘要数字。
 *
 * 折叠条件（缺一不可）：
 *  - 不是正在流式输出的消息（`streaming` 为真）；
 *  - 存在最终结论段（最后一个 text 段 = 边界）——没有结论的轮次（以错误/
 *    工具段收尾）保留全部过程证据，绝不折叠；
 *  - 边界之前至少有一段 reasoning/tool。
 *
 * 图表段是交付物、错误段是终态证据，始终可见，不进折叠集合。
 */
export function deriveTurnProcess(
  segments: Segment[],
  streaming: boolean,
): TurnProcess | null {
  if (streaming || segments.length === 0) return null;

  // 边界 = 最后一个 text 段。
  let boundaryIdx = -1;
  for (let i = segments.length - 1; i >= 0; i--) {
    if (segments[i].type === "text") {
      boundaryIdx = i;
      break;
    }
  }
  if (boundaryIdx < 0) return null;

  const foldedIds = new Set<string>();
  let anchorId: string | undefined;
  let reasoningMs: number | undefined;
  let msKnown = true;
  let toolCount = 0;
  for (let i = 0; i < boundaryIdx; i++) {
    const s = segments[i];
    if (s.type === "chart" || s.type === "error") continue;
    if (s.type === "reasoning" || s.type === "tool" || s.type === "text") {
      foldedIds.add(s.id);
      if (anchorId === undefined) anchorId = s.id;
      if (s.type === "reasoning") {
        if (typeof s.elapsedMs === "number" && reasoningMs !== undefined) {
          reasoningMs += s.elapsedMs;
        } else if (typeof s.elapsedMs === "number") {
          reasoningMs = s.elapsedMs;
        } else {
          msKnown = false;
        }
      }
      if (s.type === "tool") toolCount += 1;
    }
  }
  if (anchorId === undefined) return null;

  return {
    anchorId,
    foldedIds,
    ...(msKnown && reasoningMs !== undefined ? { reasoningMs } : {}),
    toolCount,
  };
}
