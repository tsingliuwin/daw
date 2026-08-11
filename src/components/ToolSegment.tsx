import { Show, For, createSignal } from "solid-js";
import type { Segment, SqlResult } from "../lib/types";
import ResultTable from "./ResultTable";

/** 工具的人类可读标签。未知工具回退到原始 tool 名。 */
const TOOL_LABELS: Record<string, string> = {
  get_current_time: "获取当前时间",
  search: "搜索",
  execute_query: "执行查询",
  list_tables: "列出数据表",
  list_connections: "列出数据源",
  list_remote_tables: "列出远程表",
  register_table: "注册表",
  describe_table: "表结构",
  sample_data: "采样数据",
  render_chart: "生成图表",
};

/** 工具常用的参数键名映射（仅用于详情展示，未知键回退原样）。 */
const ARG_KEY_LABELS: Record<string, string> = {
  sql: "SQL",
  table_name: "表名",
  query: "查询词",
  num_results: "结果数",
};

/**
 * 一个工具调用段（tool_call + tool_result 合并）。相比 lakemind：
 *  - 去掉 SqlBlock / ResultTable / OKF 详情渲染、onOpenInSqlPanel 回调。
 *  - body 改为：seg.detail 人类可读文本 + seg.payload 简单键值对/JSON 预览。
 *  - TOOL_LABELS 换成 OA 工具。
 *  - 保留：状态图标 (running/ok/error/awaiting)、折叠展开、计时器、
 *    awaiting 时的确认/取消按钮（触发 onConfirm → 调 resolve_tool_confirmation）。
 *
 * 折叠/展开由父级 ChatView 的 expandedSegmentIds 驱动。
 */
export default function ToolSegment(props: {
  seg: Segment;
  expanded: boolean;
  onToggle: (id: string) => void;
  /** awaiting 状态下用户点击「确认执行」(true) 或「取消」(false)。 */
  onConfirm?: (approved: boolean) => void;
}) {
  // ToolSegment 只在 `type === "tool"` 段渲染（父级已过滤）。窄化到本地变量。
  const t = () => (props.seg.type === "tool" ? props.seg : null);

  /** payload 解析为可遍历的 [key, value] 数组（仅当是对象时）。 */
  const payloadEntries = (): [string, unknown][] => {
    const s = t();
    const p = s?.payload;
    if (p == null) return [];
    if (typeof p === "object" && !Array.isArray(p)) {
      return Object.entries(p as Record<string, unknown>);
    }
    return [];
  };

  /** 探测 payload 是否是 SqlResult（columns 数组 + rows 二维数组）。 */
  const payloadSqlResult = (): SqlResult | null => {
    const p = t()?.payload;
    if (p == null || typeof p !== "object" || Array.isArray(p)) return null;
    const obj = p as Record<string, unknown>;
    if (Array.isArray(obj.columns) && Array.isArray(obj.rows)) {
      return obj as unknown as SqlResult;
    }
    return null;
  };

  const hasBody = () => {
    const s = t();
    if (!s) return false;
    // args 只有在非空对象时才算有内容（空 {} 不算）。
    const hasArgs = s.args && typeof s.args === "object" && Object.keys(s.args as object).length > 0;
    return !!(
      s.status === "awaiting" || // awaiting: 始终展开，展示 detail + 确认按钮
      s.status === "error" || // error: 始终可展开，展示错误详情
      s.detail ||
      s.payload != null ||
      hasArgs ||
      s.result
    );
  };

  // awaiting 状态点击确认/取消后，本地置灰，等待后端 tool_result 覆盖状态。
  const [confirmResolved, setConfirmResolved] = createSignal(false);
  const handleConfirm = (approved: boolean) => {
    if (confirmResolved()) return;
    setConfirmResolved(true);
    props.onConfirm?.(approved);
  };

  return (
    <div
      class={`tool-seg tool-seg--${t()?.status}`}
      classList={{ "tool-seg--open": props.expanded && hasBody() }}
    >
      <div
        class="tool-seg__summary"
        classList={{ "tool-seg__summary--clickable": hasBody() && t()?.status !== "running" && t()?.status !== "awaiting" }}
        onClick={() => {
          if (hasBody() && t()?.status !== "running" && t()?.status !== "awaiting") {
            props.onToggle(t()!.id);
          }
        }}
      >
        <span class="tool-seg__status">
          {t()?.status === "running" ? (
            <span class="tool-seg__spinner" />
          ) : t()?.status === "awaiting" ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px; color: var(--accent-orange, #f59e0b);">
              <circle cx="12" cy="12" r="10"></circle>
              <line x1="10" y1="9" x2="10" y2="15"></line>
              <line x1="14" y1="9" x2="14" y2="15"></line>
            </svg>
          ) : t()?.status === "error" ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px; color: var(--accent-red, #ef4444);">
              <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path>
              <line x1="12" y1="9" x2="12" y2="13"></line>
              <line x1="12" y1="17" x2="12.01" y2="17"></line>
            </svg>
          ) : (
            // ok —— 通用成功对勾
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px; color: var(--accent-green, #10b981);">
              <polyline points="20 6 9 17 4 12"></polyline>
            </svg>
          )}
        </span>
        <span class="tool-seg__name">{t() ? (TOOL_LABELS[t()!.tool] ?? t()!.tool) : ""}</span>
        <Show when={t()?.status === "running" && t()?.startTime != null}>
          <span class="tool-seg__meta" id={`tool-timer-${t()?.id}`}>· …</span>
        </Show>
        <Show when={t()?.elapsedMs != null}>
          <span class="tool-seg__meta">· {fmtMs(t()!.elapsedMs!)}</span>
        </Show>
        <Show when={t()?.summary}>
          <span class="tool-seg__summary-text">· {t()!.summary}</span>
        </Show>
        <Show when={t()?.status !== "running" && t()?.status !== "awaiting" && hasBody()}>
          <span class="tool-seg__chevron" classList={{ "tool-seg__chevron--open": props.expanded }}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="width: 10px; height: 10px; transition: transform 0.15s ease;">
              <polyline points="9 18 15 12 9 6"></polyline>
            </svg>
          </span>
        </Show>
      </div>

      <Show when={props.expanded && hasBody()}>
        <div class="tool-seg__body">
          {/* Error message (full, untruncated). */}
          <Show when={t()?.status === "error" && t()?.summary}>
            <div class="tool-seg__error-detail">{t()!.summary}</div>
          </Show>

          {/* Awaiting confirmation: 展示 detail + 确认/取消按钮。 */}
          <Show when={t()?.status === "awaiting"}>
            <div class="tool-seg__confirm">
              <div class="tool-seg__confirm-hint">即将执行以下操作，请确认：</div>
              <Show when={t()?.detail}>
                <pre class="tool-seg__detail-text">{t()!.detail}</pre>
              </Show>
              <div class="tool-seg__confirm-actions">
                <button
                  class="tool-seg__confirm-btn tool-seg__confirm-btn--ok"
                  disabled={confirmResolved()}
                  onClick={(e) => { e.stopPropagation(); handleConfirm(true); }}
                >确认执行</button>
                <button
                  class="tool-seg__confirm-btn tool-seg__confirm-btn--cancel"
                  disabled={confirmResolved()}
                  onClick={(e) => { e.stopPropagation(); handleConfirm(false); }}
                >取消</button>
              </div>
            </div>
          </Show>

          {/* 人类可读动作摘要（detail，非 awaiting 时也可展示，如已完成写操作的回执）。 */}
          <Show when={t()?.detail && t()?.status !== "awaiting"}>
            <div class="tool-seg__detail-line">{t()!.detail}</div>
          </Show>

          {/* 结构化 payload：
              - SqlResult（columns+rows）→ 虚拟滚动表格
              - 普通对象 → 键值对列表
              - 其它 → JSON 字符串预览 */}
          <Show when={t()?.payload != null}>
            <Show when={payloadSqlResult()}>
              {(sr) => <ResultTable result={sr()} compact />}
            </Show>
            <Show when={!payloadSqlResult()}>
              <Show
                when={payloadEntries().length > 0}
                fallback={
                  <pre class="tool-seg__payload-json">{JSON.stringify(t()?.payload, null, 2)}</pre>
                }
              >
                <div class="tool-seg__payload">
                  <For each={payloadEntries()}>
                    {([key, val]) => (
                      <div class="tool-seg__payload-row">
                        <span class="tool-seg__payload-key">{ARG_KEY_LABELS[key] ?? key}</span>
                        <span class="tool-seg__payload-val">{formatVal(val)}</span>
                      </div>
                    )}
                  </For>
                </div>
              </Show>
            </Show>
          </Show>

          {/* 通用调用参数（args 为对象时展示键值对）。 */}
          <Show when={t()?.args && typeof t()!.args === "object" && Object.keys(t()!.args as object).length > 0}>
            <div class="tool-seg__args">
              <div class="tool-seg__args-title">调用参数</div>
              <For each={Object.entries(t()!.args as Record<string, unknown>)}>
                {([key, val]) => (
                  <div class="tool-seg__arg-row">
                    <span class="tool-seg__arg-key">{ARG_KEY_LABELS[key] ?? key}</span>
                    <span class="tool-seg__arg-val">{formatVal(val)}</span>
                  </div>
                )}
              </For>
            </div>
          </Show>

          {/* 通用文本结果（result）。 */}
          <Show when={t()?.result}>
            <pre class="tool-seg__result-text">{t()!.result}</pre>
          </Show>
        </div>
      </Show>
    </div>
  );
}

/** 把任意值格式化成单行展示字符串。对象/数组 → 紧凑 JSON。 */
function formatVal(val: unknown): string {
  if (val == null) return "";
  if (typeof val === "string") return val;
  if (typeof val === "number" || typeof val === "boolean") return String(val);
  try {
    return JSON.stringify(val);
  } catch {
    return String(val);
  }
}

function fmtMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}
