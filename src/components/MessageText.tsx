import { For, Match, Show, Switch, createMemo } from "solid-js";
import type { Segment } from "../lib/types";
import { splitTextByChartRefs, findChartSegment } from "../lib/chartRef";
import MarkdownRenderer from "./MarkdownRenderer";
import ChartSegment from "./ChartSegment";

type ChartSeg = Extract<Segment, { type: "chart" }>;

/** A text chunk after splitting, with any chart reference resolved to its
 *  segment (or `undefined` if unresolvable). */
type ResolvedChunk =
  | { kind: "text"; content: string }
  | { kind: "chartRef"; ref: string; chart: ChartSeg | undefined };

/**
 * Renders a markdown text segment that may contain inline chart-reference
 * markers (`{{chart:<id>}}` — returned to the model by the `render_chart`
 * tool). Text between markers is rendered with MarkdownRenderer; each marker
 * is replaced in-place by the corresponding interactive ChartSegment.
 *
 * Streaming stability: reuses chart-ref chunk objects across re-renders so
 * SolidJS <For> (reference-keyed) doesn't dispose+recreate echarts instances
 * on every token.
 */
export default function MessageText(props: { text: string; segments?: Segment[]; charts?: Segment[] }) {
  let chartChunkCache = new Map<string, ResolvedChunk>();

  const chunks = createMemo<ResolvedChunk[]>(() => {
    const segs = props.segments ?? [];
    // Prefer the cross-conversation chart index so a marker that references a
    // chart emitted in a previous message still resolves inline.
    const searchSource = props.charts ?? segs;
    const nextCache = new Map<string, ResolvedChunk>();
    const out: ResolvedChunk[] = splitTextByChartRefs(props.text).map((p) => {
      if (p.kind === "chartRef") {
        const chart = findChartSegment(searchSource, p.ref);
        const cached = chartChunkCache.get(p.ref);
        if (cached && cached.kind === "chartRef" && cached.chart === chart) {
          nextCache.set(p.ref, cached);
          return cached;
        }
        const fresh: ResolvedChunk = { kind: "chartRef", ref: p.ref, chart };
        nextCache.set(p.ref, fresh);
        return fresh;
      }
      return { kind: "text", content: p.content };
    });
    chartChunkCache = nextCache;
    return out;
  });
  const hasRefs = createMemo(() => chunks().some((c) => c.kind === "chartRef"));

  return (
    <Show when={hasRefs()} fallback={<MarkdownRenderer content={props.text} />}>
      <For each={chunks()}>
        {(c) => (
          <Switch>
            <Match when={c.kind === "text"}>
              <MarkdownRenderer content={c.kind === "text" ? c.content : ""} />
            </Match>
            <Match when={c.kind === "chartRef" && c.chart}>
              {(chart) => <ChartSegment seg={chart()} />}
            </Match>
            <Match when={c.kind === "chartRef"}>
              <span class="chart-ref-missing">📊 图表引用未找到</span>
            </Match>
          </Switch>
        )}
      </For>
    </Show>
  );
}
