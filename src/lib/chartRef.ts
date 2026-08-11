import type { Segment } from "./types";

/**
 * Inline chart-reference protocol.
 *
 * When the `render_chart` tool runs, it returns a marker like
 * `{{chart:tool-chart-1234-1}}` to the model (in its tool-result string). The
 * model is instructed to paste that marker into its final text summary wherever
 * it wants the chart to (re)appear. The frontend then splits the text on these
 * markers and, at each one, renders the matching `chart` segment inline.
 *
 * The token inside the marker is the chart segment's `id` (== render_chart's
 * `call_id`), so the match is exact and unambiguous.
 */

const CHART_REF_RE = /\{\{\s*chart:\s*([^}]+?)\s*\}\}/g;

export type TextPart = { kind: "text"; content: string };
export type ChartRefPart = { kind: "chartRef"; ref: string };
export type TextChunk = TextPart | ChartRefPart;

/**
 * Split a markdown text into alternating text / chart-reference chunks.
 *
 * An unmatched `{{chart:` (e.g. mid-stream before the closing `}}` arrives)
 * stays inside a trailing text chunk and self-heals into a marker once the
 * braces close — mirroring how markdown tolerates partially-typed syntax.
 */
export function splitTextByChartRefs(text: string): TextChunk[] {
  if (!text) return [];
  const parts: TextChunk[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  CHART_REF_RE.lastIndex = 0;
  while ((m = CHART_REF_RE.exec(text)) !== null) {
    if (m.index > last) parts.push({ kind: "text", content: text.slice(last, m.index) });
    parts.push({ kind: "chartRef", ref: m[1].trim() });
    last = m.index + m[0].length;
  }
  if (last < text.length) parts.push({ kind: "text", content: text.slice(last) });
  return parts;
}

type ChartSegment = Extract<Segment, { type: "chart" }>;

/**
 * Find the chart segment in a message that matches a reference token.
 *
 * Priority:
 * 1. Exact `id` match — the common path, since render_chart returns `call_id`
 *    (which is the segment's id) as the marker.
 * 2. Title fuzzy match — covers cross-turn references where the model writes
 *    the chart's title from memory instead of the id.
 * 3. Numeric ordinal — `{{chart:1}}` → first chart in the message.
 *
 * Returns `undefined` when nothing matches; callers render a fallback badge.
 */
export function findChartSegment(segments: Segment[], ref: string): ChartSegment | undefined {
  const charts = segments.filter((s): s is ChartSegment => s.type === "chart");
  if (charts.length === 0) return undefined;

  // 1. Exact id.
  let hit = charts.find((c) => c.id === ref);
  if (hit) return hit;

  // 2. Title fuzzy match.
  hit = charts.find(
    (c) => !!c.title && (c.title === ref || c.title.includes(ref) || ref.includes(c.title)),
  );
  if (hit) return hit;

  // 3. Ordinal (1-based).
  const n = parseInt(ref, 10);
  if (!Number.isNaN(n) && n >= 1 && n <= charts.length) return charts[n - 1];

  return undefined;
}

/** Quick check for whether a text contains any chart-reference marker. */
export function hasChartRefs(text: string): boolean {
  if (!text) return false;
  CHART_REF_RE.lastIndex = 0;
  return CHART_REF_RE.test(text);
}
