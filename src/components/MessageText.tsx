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
  // 文本块按「序号+内容」缓存复用同一对象：Solid 的 <For> 按引用 diff，而流式期
  // 父级每 150ms 冲刷一次（allCharts 每冲刷都产生新数组身份，连带本 memo 重跑）。
  // 若每次重建 text chunk 对象，For 会销毁并重建全部子树——带图表引用的消息
  // （每段文本+图表交替）每冲刷就全量重走 marked+Shiki markdown 渲染，长对话
  // 流式期渲染债滚雪球，主线程十分钟级冻结、窗口 resize 无法落地。
  let textChunkCache = new Map<string, ResolvedChunk>();

  const chunks = createMemo<ResolvedChunk[]>(() => {
    const segs = props.segments ?? [];
    // Prefer the cross-conversation chart index so a marker that references a
    // chart emitted in a previous message still resolves inline.
    const searchSource = props.charts ?? segs;
    const nextCache = new Map<string, ResolvedChunk>();
    const nextTextCache = new Map<string, ResolvedChunk>();
    const out: ResolvedChunk[] = splitTextByChartRefs(props.text).map((p, i) => {
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
      const key = i + ":" + p.content;
      const cached = textChunkCache.get(key);
      if (cached) {
        nextTextCache.set(key, cached);
        return cached;
      }
      const fresh: ResolvedChunk = { kind: "text", content: p.content };
      nextTextCache.set(key, fresh);
      return fresh;
    });
    chartChunkCache = nextCache;
    textChunkCache = nextTextCache;
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
