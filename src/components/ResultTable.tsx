import { Show, For, createSignal, createMemo, createEffect, onCleanup } from "solid-js";
import { createVirtualizer } from "@tanstack/solid-virtual";
import { createSolidTable, flexRender, getCoreRowModel } from "@tanstack/solid-table";
import type { ColumnDef } from "@tanstack/table-core";
import type { SqlResult, JsonValue } from "../lib/types";

const ROW_IDX_W = 54;
const CELL_W = 160;

type Row = JsonValue[];

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

function renderCell(v: unknown): string {
  if (v == null) return "";
  if (typeof v === "string") return v;
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  try { return JSON.stringify(v); } catch { return String(v); }
}

function VirtualGrid(props: { result: SqlResult; compact?: boolean }) {
  let scrollRef: HTMLDivElement | undefined;
  const [columnSizing, setColumnSizing] = createSignal<Record<string, number>>({});

  const columns = createMemo<ColumnDef<Row, unknown>[]>(() => {
    return props.result.columns.map(
      (name, i) =>
        ({
          id: name,
          accessorFn: (row: Row) => row[i],
          header: () => name,
          cell: (info: any) => renderCell(info.getValue()),
          size: CELL_W,
          minSize: 60,
          maxSize: 600,
        }) as ColumnDef<Row, unknown>,
    );
  });

  const data = createMemo<Row[]>(() => props.result.rows ?? []);

  const table = createSolidTable({
    get data() { return data(); },
    get columns() { return columns(); },
    state: {
      get columnSizing() { return columnSizing(); },
    },
    onColumnSizingChange: setColumnSizing,
    columnResizeMode: "onChange",
    getCoreRowModel: getCoreRowModel(),
  });

  const tableWidth = createMemo(() => ROW_IDX_W + table.getCenterTotalSize());

  const rowVirtualizer = createVirtualizer({
    get count() { return table.getRowModel().rows.length; },
    getScrollElement: () => scrollRef ?? null,
    estimateSize: () => (props.compact ? 24 : 28),
    overscan: props.compact ? 8 : 12,
  });

  createEffect(() => {
    props.result;
    scrollRef?.scrollTo({ top: 0, left: 0 });
  });

  const isResizing = createMemo(() => !!table.getState().columnSizingInfo.isResizingColumn);

  createEffect(() => {
    if (isResizing()) {
      document.body.classList.add("is-resizing-column");
    } else {
      document.body.classList.remove("is-resizing-column");
    }
  });

  onCleanup(() => {
    document.body.classList.remove("is-resizing-column");
  });

  return (
    <div
      class="result-scroll"
      classList={{
        "result-scroll--compact": !!props.compact,
        "is-resizing": isResizing(),
      }}
      ref={scrollRef}
    >
      {/* Sticky header */}
      <For each={table.getHeaderGroups()}>
        {(headerGroup) => (
          <div class="result-head" role="row" style={{ width: `${tableWidth()}px` }}>
            <div class="result-cell row-idx">#</div>
            <For each={headerGroup.headers}>
              {(header) => (
                <div
                  class="result-cell head-cell"
                  style={{
                    flex: `0 0 ${header.column.getSize()}px`,
                    width: `${header.column.getSize()}px`,
                    position: "relative",
                  }}
                >
                  {flexRender(header.column.columnDef.header, header.getContext())}
                  <Show when={header.column.getCanResize()}>
                    <div
                      onMouseDown={header.getResizeHandler()}
                      onTouchStart={header.getResizeHandler()}
                      class="resizer"
                      classList={{ isResizing: header.column.getIsResizing() }}
                    />
                  </Show>
                </div>
              )}
            </For>
          </div>
        )}
      </For>
      {/* Virtualized body */}
      <div
        style={{
          height: `${rowVirtualizer.getTotalSize()}px`,
          position: "relative",
          width: `${tableWidth()}px`,
        }}
      >
        <For each={rowVirtualizer.getVirtualItems()}>
          {(vRow) => {
            const row = table.getRowModel().rows[vRow.index];
            if (!row) return null;
            return (
              <div
                class="result-row"
                role="row"
                style={{
                  position: "absolute",
                  top: "0",
                  left: "0",
                  width: `${tableWidth()}px`,
                  height: `${vRow.size}px`,
                  transform: `translateY(${vRow.start}px)`,
                }}
              >
                <div class="result-cell row-idx">{vRow.index + 1}</div>
                <For each={row.getVisibleCells()}>
                  {(cell) => (
                    <div
                      class="result-cell"
                      style={{
                        flex: `0 0 ${cell.column.getSize()}px`,
                        width: `${cell.column.getSize()}px`,
                      }}
                    >
                      {flexRender(cell.column.columnDef.cell, cell.getContext())}
                    </div>
                  )}
                </For>
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
}
