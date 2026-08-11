import { Show, For, createSignal } from "solid-js";
import { createVirtualizer } from "@tanstack/solid-virtual";
import { createSolidTable, flexRender, getCoreRowModel } from "@tanstack/solid-table";
import type { ColumnDef } from "@tanstack/table-core";
import type { SqlResult, JsonValue } from "../lib/types";

const ROW_H = 32;

/**
 * SQL 查询结果表格。用 @tanstack/solid-table table model + @tanstack/solid-virtual
 * 虚拟滚动 + 原生 <table> 元素。compact 模式用于内嵌聊天工具段。
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
        {(result) => <VirtualTable result={result()} compact={props.compact} />}
      </Show>
    </div>
  );
}

function VirtualTable(props: { result: SqlResult; compact?: boolean }) {
  let scrollRef: HTMLDivElement | undefined;
  const [viewportH] = createSignal(props.compact ? 220 : 400);

  const columns: ColumnDef<JsonValue[], unknown>[] = props.result.columns.map((col, i) => ({
    id: col,
    accessorFn: (row: JsonValue[]) => row[i],
    header: col,
    cell: (info: any) => fmtCell(info.getValue()),
  }));

  const rows = props.result.rows;

  const table = createSolidTable({
    get data() { return rows; },
    columns,
    getCoreRowModel: getCoreRowModel(),
  });

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
      style={{ height: `${Math.min(viewportH(), totalH() + 40)}px` }}
    >
      <table class="result-table">
        <thead>
          <For each={table.getHeaderGroups()}>
            {(hg) => (
              <tr>
                <th class="result-idx-col">#</th>
                <For each={hg.headers}>
                  {(header) => (
                    <Show when={!header.isPlaceholder}>
                      <th>
                        {flexRender(header.column.columnDef.header, header.getContext())}
                        <Show when={props.result.columnTypes[header.column.getIndex()]}>
                          <span class="result-th-type">{props.result.columnTypes[header.column.getIndex()]}</span>
                        </Show>
                      </th>
                    </Show>
                  )}
                </For>
              </tr>
            )}
          </For>
        </thead>
        <tbody style={{ display: "block", height: `${totalH()}px`, position: "relative" }}>
          <For each={items()}>
            {(vi) => {
              const row = table.getRowModel().rows[vi.index];
              if (!row) return null;
              return (
                <tr
                  class="result-virtual-row"
                  style={{ transform: `translateY(${vi.start}px)`, height: `${vi.size}px`, position: "absolute", top: 0, left: 0, width: "100%" }}
                >
                  <td class="result-idx-col">{vi.index + 1}</td>
                  <For each={row.getVisibleCells()}>
                    {(cell) => (
                      <td>{flexRender(cell.column.columnDef.cell, cell.getContext())}</td>
                    )}
                  </For>
                </tr>
              );
            }}
          </For>
        </tbody>
      </table>
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
