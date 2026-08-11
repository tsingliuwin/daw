import { Show, For, createSignal } from "solid-js";
import { createVirtualizer } from "@tanstack/solid-virtual";
import { createSolidTable, flexRender, getCoreRowModel } from "@tanstack/solid-table";
import type { ColumnDef } from "@tanstack/table-core";
import type { SqlResult, JsonValue } from "../lib/types";

const ROW_IDX_W = 48;
const CELL_W = 160;
const ROW_H = 28;

/**
 * SQL 查询结果表格。用 @tanstack/solid-table 的 table model + @tanstack/solid-virtual
 * 做虚拟滚动。compact 模式用于内嵌聊天工具段（限高内滚）。
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

  // 从 SqlResult 构造 ColumnDef 数组。
  const columns: ColumnDef<JsonValue[], unknown>[] = props.result.columns.map((col, i) => ({
    id: col,
    header: () => (
      <div class="result-th" style={{ width: `${CELL_W}px` }}>
        <span class="result-th-name" title={col}>{col}</span>
        <Show when={props.result.columnTypes[i]}>
          <span class="result-th-type">{props.result.columnTypes[i]}</span>
        </Show>
      </div>
    ),
    cell: (info: any) => (
      <div class="result-cell" style={{ width: `${CELL_W}px` }}>
        {fmtCell(info.getValue())}
      </div>
    ),
  }));

  // 构造行数据。
  const rows = props.result.rows;

  // table model。
  const table = createSolidTable({
    get data() { return rows; },
    columns,
    getCoreRowModel: getCoreRowModel(),
  });

  // 虚拟滚动。
  const virtualizer = createVirtualizer({
    get count() { return rows.length; },
    getScrollElement: () => scrollRef ?? null,
    estimateSize: () => ROW_H,
    overscan: 8,
  });

  const items = () => virtualizer.getVirtualItems();
  const totalH = () => virtualizer.getTotalSize();

  return (
    <div
      class="result-scroll"
      ref={scrollRef}
      style={{ height: `${Math.min(viewportH(), totalH() + 36)}px` }}
    >
      {/* 固定表头 */}
      <div class="result-thead" style={{ "padding-left": `${ROW_IDX_W}px` }}>
        <For each={table.getHeaderGroups()}>
          {(hg) => (
            <For each={hg.headers}>
              {(header) => (
                <Show when={!header.isPlaceholder}>
                  {flexRender(header.column.columnDef.header, header.getContext())}
                </Show>
              )}
            </For>
          )}
        </For>
      </div>
      {/* 虚拟行 */}
      <div style={{ height: `${totalH()}px`, position: "relative" }}>
        <For each={items()}>
          {(vi) => {
            const row = table.getRowModel().rows[vi.index];
            if (!row) return null;
            return (
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
                <For each={row.getVisibleCells()}>
                  {(cell) => flexRender(cell.column.columnDef.cell, cell.getContext())}
                </For>
              </div>
            );
          }}
        </For>
      </div>
      <Show when={props.result.truncated}>
        <div class="result-truncated">结果已截断，仅显示前 {props.result.rowCount} 行</div>
      </Show>
    </div>
  );
}

function fmtCell(v: unknown): string {
  if (v == null) return "";
  if (typeof v === "string") return v;
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  try { return JSON.stringify(v); } catch { return String(v); }
}
