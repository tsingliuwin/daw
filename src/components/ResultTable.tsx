import { Show, For, createSignal } from "solid-js";
import { createVirtualizer } from "@tanstack/solid-virtual";
import type { SqlResult } from "../lib/types";

// 固定列宽（与 CSS 对应）。
const ROW_IDX_W = 48;
const CELL_W = 160;
const ROW_H = 28; // compact 行高

/**
 * SQL 查询结果表格。虚拟滚动渲染（支持大量行），固定列宽。
 * compact 模式用于内嵌聊天工具段（限高内滚）。
 *
 * 子组件 VirtualGrid 绕开 solid-virtual 的 scroll-element bootstrap 问题：
 * createVirtualizer 在组件创建时读 scrollElement，若 scroll 容器在 <Show>
 * 里初始为 false，ref 永远不赋值，virtualizer 死锁 0 行。把 virtualizer
 * 放在只在 result 非 null 时才创建的子组件里，保证 scroll 容器在
 * onMount 时已存在。
 */
export default function ResultTable(props: {
  result: SqlResult | null;
  compact?: boolean;
}) {
  const validResult = (): SqlResult | null => {
    const r = props.result;
    if (r && r.columns.length > 0) return r;
    return null;
  };
  return (
    <div class="result-wrap" classList={{ "result-wrap--compact": !!props.compact }}>
      <Show
        when={validResult()}
        fallback={<div class="result-empty">无数据</div>}
      >
        {(result) => <VirtualGrid result={result()} compact={props.compact} />}
      </Show>
    </div>
  );
}

function VirtualGrid(props: { result: SqlResult; compact?: boolean }) {
  let scrollRef: HTMLDivElement | undefined;
  const [viewportH] = createSignal(props.compact ? 220 : 400);

  const virtualizer = createVirtualizer({
    get count() {
      return props.result.rows.length;
    },
    getScrollElement: () => scrollRef ?? null,
    estimateSize: () => ROW_H,
    overscan: 8,
  });

  const items = () => virtualizer.getVirtualItems();
  const totalH = () => virtualizer.getTotalSize();

  // 单元格值格式化：null → 空，其它 → 字符串（截断长文本）。
  const fmtCell = (v: unknown): string => {
    if (v == null) return "";
    if (typeof v === "string") return v;
    if (typeof v === "number" || typeof v === "boolean") return String(v);
    try {
      return JSON.stringify(v);
    } catch {
      return String(v);
    }
  };

  return (
    <div
      class="result-scroll"
      ref={scrollRef}
      style={{ height: `${Math.min(viewportH(), totalH() + 36)}px` }}
    >
      {/* 固定表头 */}
      <div class="result-thead" style={{ "padding-left": `${ROW_IDX_W}px` }}>
        <For each={props.result.columns}>
          {(col, i) => (
            <div class="result-th" style={{ width: `${CELL_W}px` }}>
              <span class="result-th-name" title={col}>{col}</span>
              <Show when={props.result.columnTypes[i()]}>
                <span class="result-th-type">{props.result.columnTypes[i()]}</span>
              </Show>
            </div>
          )}
        </For>
      </div>
      {/* 虚拟行 */}
      <div style={{ height: `${totalH()}px`, position: "relative" }}>
        <For each={items()}>
          {(vi) => (
            <div
              class="result-row"
              style={{
                transform: `translateY(${vi.start}px)`,
                height: `${vi.size}px`,
                "padding-left": `${ROW_IDX_W}px`,
              }}
            >
              <div class="result-row-idx" style={{ width: `${ROW_IDX_W}px`, left: `0` }}>
                {vi.index + 1}
              </div>
              <For each={props.result.rows[vi.index]}>
                {(cell) => (
                  <div class="result-cell" style={{ width: `${CELL_W}px` }}>
                    {fmtCell(cell)}
                  </div>
                )}
              </For>
            </div>
          )}
        </For>
      </div>
      <Show when={props.result.truncated}>
        <div class="result-truncated">结果已截断，仅显示前 {props.result.rowCount} 行</div>
      </Show>
    </div>
  );
}
