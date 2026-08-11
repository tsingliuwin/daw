// 通信类型定义 —— 与 src-tauri/src/agent/wire.rs 一一对应。
// 修改通信格式时请同步两侧。

export type JsonValue = string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue };

// ---------------------------------------------------------------------------
// 统一日志 —— 与 src-tauri/src/model.rs LogRecord / LogFilter 一一对应。
// ---------------------------------------------------------------------------

/** 日志级别。后端 tracing Level 映射后的小写字符串。 */
export type LogLevel = "debug" | "info" | "warn" | "error";

/** 日志分类的固定枚举。控制台多 Tab 与日志分析模块据此过滤。
 * 必须与后端 model::LOG_CATEGORIES 保持一致。
 * 相比 lakemind 去掉了 query/import/sync/duckdb，新增 oa。 */
export type LogCategory =
  | "agent"
  | "system"
  | "ui"
  | "link"
  | "oa";

/** 一条统一日志。对应 SQLite `logs` 表的一行，也是 `app-log` 事件的 payload。
 * `detail` 是按类别而异的结构化字段。 */
export interface UnifiedLog {
  id?: number;
  /** Unix 毫秒时间戳。 */
  ts: number;
  level: LogLevel;
  category: LogCategory;
  /** 单行人类可读摘要。 */
  message: string;
  /** 结构化明细（JSON 对象，字段随 category 变化）。 */
  detail?: Record<string, unknown>;
  /** 关联 workspace（全局日志为 undefined）。 */
  workspace?: string;
  /** 关联 task（agent 类日志必填）。 */
  taskId?: string;
}

/** `query_logs` 命令的过滤参数。与后端 LogFilter 对应。 */
export interface LogFilter {
  categories?: LogCategory[];
  levels?: LogLevel[];
  fromTs?: number;
  toTs?: number;
  keyword?: string;
  limit: number;
  offset?: number;
}

/**
 * Agent 执行过程中的一段产物。一条 assistant 消息是 Segment 的有序列表，
 * 按真实发生顺序排列：reasoning → tool → reasoning → tool → … → text(结论)。
 *
 * 与后端 wire.rs 的 Segment 枚举严格对应（tag = "type", camelCase）。
 * 相比 lakemind：tool 分支去掉 sql/table，新增 detail/payload；删除 chart 分支。
 */
export type Segment =
  | { type: "reasoning"; id: string; text: string; elapsedMs?: number; startTime?: number }
  | {
      type: "tool";
      id: string;
      tool: string;
      args?: unknown;
      status: "running" | "ok" | "error" | "awaiting";
      /** 人类可读摘要（折叠时显示）。 */
      summary?: string;
      /** 写操作的人类可读动作摘要。awaiting 状态下展示给用户确认。 */
      detail?: string;
      /** 结构化结果 payload（SqlResult / OA 结果等任意 JSON）。 */
      payload?: unknown;
      elapsedMs?: number;
      /** Tool call timestamp (ms) - used to show a live timer while running. */
      startTime?: number;
      /** 工具执行结果的纯文本描述（error 时通常是错误信息）。 */
      result?: string;
    }
  | { type: "text"; id: string; text: string }
  | {
      type: "chart";
      id: string;
      chartType: string;
      title?: string;
      xField?: string;
      yFields?: string[];
      rightYFields?: string[];
      yFieldLabels?: Record<string, string>;
      table: SqlResult;
    }
  | { type: "error"; id: string; text: string };

/** 一条对话消息。assistant 消息由有序 Segment 构成。 */
export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  /** 有序产物段（user 消息为单个 text 段）。 */
  segments: Segment[];
  ts: number;
}

export interface TokenUsage {
  // ── Legacy fields (kept for backward-compat with persisted data written
  //    before the metrics refactor). `derivePanelMetrics` falls back to these
  //    when the new real fields are absent; `mergeUsage` no longer writes them. ──
  inputTokens?: number;
  outputTokens?: number;
  totalTokens?: number;
  cachedInputTokens?: number;
  messagesTokens?: number;
  toolsTokens?: number;
  preambleTokens?: number;
  cacheHitRate?: number;
  _totalInputAllTurns?: number;
  _totalCachedAllTurns?: number;
  _peakInputTokens?: number;

  // ── New real fields (provider-normalized by the backend). ──
  /** True total prompt tokens this call (cache read + creation + fresh). */
  promptTokens?: number;
  /** Completion (output) tokens this call. */
  completionTokens?: number;
  /** Tokens served from the provider cache (cheap). */
  cacheReadTokens?: number;
  /** Tokens written to the provider cache this call. */
  cacheCreationTokens?: number;
  /** Full-price input tokens (neither cached nor newly-cached). */
  freshInputTokens?: number;
  /** `k = 1` (uncalibrated) token estimate of the fixed system prompt. */
  estPreambleRaw?: number;
  /** `k = 1` (uncalibrated) token estimate of the tool definitions block. */
  estToolsRaw?: number;
  /**
   * Per-model calibration factor (EMA of real/estimated prompt). Applied to
   * `estPreambleRaw` / `estToolsRaw` so the composition estimate converges
   * toward reality over turns. Defaults to 1.0 when no sample exists.
   */
  _calibK?: number;
  /** True when the current values are a pre-FinalResponse estimate (internal
   *  only — never displayed as a label; drives the "freeze real, advance
   *  output" merge behavior). */
  isEstimate?: boolean;

  // ── Cumulative across the whole conversation (real, per LLM call). ──
  _totalPrompt?: number;
  _totalCompletion?: number;
  _totalCacheRead?: number;
  _totalCacheCreation?: number;
  /** Number of LLM calls that produced a real FinalResponse. Drives the
   *  composition multiplier (preamble/tools are sent on every call). */
  _llmCallCount?: number;
  /** Number of completed user turns (one per finished agent run). Displayed. */
  _turnCount?: number;
  /** Peak `promptTokens` ever seen — the context-window bar never shrinks. */
  _peakPromptTokens?: number;
  /** Generation speed (tok/s) of the most recently completed run. */
  _lastTokPerSec?: number;
}

/**
 * 一个任务（用户通过对话发起的一次流程）。
 * kind 区分场景：task=通用对话，data_analysis=数据分析。
 */
export interface Task {
  id: string;
  name: string;
  createdAt: number;
  /** 任务的消息历史（assistant + user 交替）。 */
  messages?: ChatMessage[];
  /** 是否已保存。 */
  saved?: boolean;
  /** 使用的模型 ID（复合键 providerId:modelId）。 */
  modelId?: string;
  /** 累计 token 用量（持久化，伴随任务全生命周期）。 */
  tokenUsage?: TokenUsage;
  /** 任务场景类型，决定 agent 工具集和 preamble。默认 "task"。 */
  kind?: "task" | "data_analysis";
}

// ---------------------------------------------------------------------------
// 数据分析 —— SqlResult / DataSourceConfig（与 src-tauri/src/model.rs 对应）
// ---------------------------------------------------------------------------

/** SQL 查询结果（execute_query / describe_table / sample_data 工具的 payload）。 */
export interface SqlResult {
  columns: string[];
  columnTypes: string[];
  rows: JsonValue[][];
  rowCount: number;
  truncated: boolean;
  elapsedMs: number;
}

/** 外部数据源连接配置（存 settings.json 的 dataSources 数组）。 */
export interface DataSourceConfig {
  id: string;
  name: string;
  dbType: "postgres" | "mysql" | "sqlite";
  host: string;
  port: number;
  databaseName: string;
  username: string;
  password: string;
  sslMode: string;
}

export interface Workspace {
  name: string;
  path: string;
}

export interface FileItem {
  name: string;
  path: string;
  is_dir: boolean;
}

/** A selectable model, disambiguated by its provider.
 * Model IDs can collide across providers (e.g. two providers offering
 * "gpt-4o"), so selection is keyed on the composite `"providerId:modelId"`. */
export interface ModelOption {
  providerId: string;
  providerName: string;
  modelId: string;
  contextWindow?: number;
}

/** Build the composite selection key for a model option. */
export function modelKeyOf(opt: ModelOption): string {
  return `${opt.providerId}:${opt.modelId}`;
}

/** Extract the human-readable model id from a composite key (the part after
 * the first `:`). Falls back to the raw value when it isn't a composite key
 * (e.g. a legacy bare model id), so old persisted selections still render. */
export function modelIdOfKey(key: string): string {
  const idx = key.indexOf(":");
  return idx >= 0 ? key.slice(idx + 1) : key;
}

/** Extract the provider id from a composite key, or null for legacy keys. */
export function providerIdOfKey(key: string): string | null {
  const idx = key.indexOf(":");
  return idx >= 0 ? key.slice(0, idx) : null;
}
