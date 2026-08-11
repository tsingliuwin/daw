import { onCleanup, onMount, createSignal, For, Show, createEffect } from "solid-js";
import * as echarts from "echarts";
import { invoke } from "@tauri-apps/api/core";
import type { Segment, SqlResult } from "../lib/types";
import { currentTheme, Theme } from "../lib/theme";
import { Portal } from "solid-js/web";

/**
 * Inline chart segment — renders an ECharts visualization from a SqlResult.
 *
 * The agent's `render_chart` tool emits a `chart` segment with a chart type
 * (bar/line/pie/scatter), axis mapping (xField/yFields), and the raw query
 * data. This component converts that into an ECharts option and renders it.
 * The user can switch chart types via the toolbar.
 */

type ChartType = "bar" | "line" | "pie" | "scatter" | "funnel" | "gauge";

/** Chart types that can be freely switched between via the tab bar. */
const SWITCHABLE_TYPES: ChartType[] = ["bar", "line", "pie", "scatter"];

const CHART_TYPES: { type: ChartType; label: string; svg: string }[] = [
  { type: "bar", label: "柱状图", svg: '<rect x="3" y="12" width="4" height="9"/><rect x="10" y="7" width="4" height="14"/><rect x="17" y="4" width="4" height="17"/>' },
  { type: "line", label: "折线图", svg: '<polyline points="3 17 8 11 13 14 21 5" fill="none"/><circle cx="3" cy="17" r="1.5"/><circle cx="8" cy="11" r="1.5"/><circle cx="13" cy="14" r="1.5"/><circle cx="21" cy="5" r="1.5"/>' },
  { type: "pie", label: "饼图", svg: '<circle cx="12" cy="12" r="9"/><path d="M12 3 A9 9 0 0 1 21 12 L12 12 Z" fill="currentColor" stroke="none"/>' },
  { type: "scatter", label: "散点图", svg: '<circle cx="5" cy="18" r="1.8"/><circle cx="10" cy="8" r="1.8"/><circle cx="15" cy="14" r="1.8"/><circle cx="19" cy="5" r="1.8"/><circle cx="8" cy="16" r="1.8"/>' },
];

/** Theme styles helper mapping palette colors, grids, tooltips, and fonts. */
function getThemeStyles(theme: Theme) {
  const isLight = theme === "light";
  return {
    isLight,
    palette: isLight
      ? ["#4f46e5", "#0ea5e9", "#10b981", "#f59e0b", "#f43f5e", "#8b5cf6", "#f97316", "#64748b"]
      : ["#5b8ff9", "#61ddaa", "#f6bd16", "#7262fd", "#ff9d4d", "#e86452", "#6dc8ec", "#945fb9"],
    axisLineColor: isLight ? "#e5e7eb" : "#3a3a3e",
    axisTickColor: isLight ? "#e5e7eb" : "#3a3a3e",
    axisLabelColor: isLight ? "#6b7280" : "#9aa0a6",
    splitLineColor: isLight ? "#f3f4f6" : "#1f1f22",
    tooltipBg: isLight ? "#ffffff" : "#18181b",
    tooltipBorder: isLight ? "#e5e7eb" : "#3a3a3e",
    tooltipText: isLight ? "#374151" : "#e6e7eb",
    textColor: isLight ? "#1f2937" : "#e6e7eb",
    legendColor: isLight ? "#6b7280" : "#9aa0a6",
    lineStyleColor: isLight ? "#e5e7eb" : "#5c6066",
    gaugeLineColor: isLight ? "#e5e7eb" : "#2a2a2e",
  };
}

function hexToRgba(hex: string, alpha: number): string {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

const isMac = typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");

export default function ChartSegment(props: { seg: Extract<Segment, { type: "chart" }> }) {
  let container: HTMLDivElement | undefined;
  let chart: echarts.ECharts | undefined;
  const [chartType, setChartType] = createSignal<ChartType>(props.seg.chartType as ChartType);
  const [isFullScreen, setIsFullScreen] = createSignal(false);
  let fullscreenContainer: HTMLDivElement | undefined;
  let fullscreenChart: echarts.ECharts | undefined;

  function saveAsImage() {
    const activeChart = isFullScreen() ? fullscreenChart : chart;
    if (!activeChart) return;
    const isLight = currentTheme() === "light";
    const url = activeChart.getDataURL({
      type: "png",
      pixelRatio: 2,
      backgroundColor: isLight ? "#ffffff" : "#121214",
    });
    const fileName = `${props.seg.title || "chart"}.png`;
    invoke("save_image_from_base64", { base64Data: url, defaultName: fileName })
      .catch((e) => console.error("[chart] save image failed:", e));
  }

  function buildOption(type: ChartType, table: SqlResult, xField?: string, yFields?: string[], rightYFields?: string[], yFieldLabels?: Record<string, string>, title?: string): echarts.EChartsOption {
    const cols = table.columns;
    const xIdx = xField ? cols.indexOf(xField) : findDimensionCol(table.columnTypes);
    const yCols = yFields && yFields.length > 0 ? yFields : findNumericCols(cols, table.columnTypes, xIdx);
    const styles = getThemeStyles(currentTheme());

    const AXIS_STYLE = {
      axisLine: { lineStyle: { color: styles.axisLineColor } },
      axisTick: { show: false, lineStyle: { color: styles.axisTickColor } },
      axisLabel: { color: styles.axisLabelColor, fontSize: 11 },
      splitLine: { lineStyle: { color: styles.splitLineColor, type: "dashed" as const } },
    };
    const AXIS_NAME_STYLE = { color: styles.axisLabelColor, fontSize: 11 };
    const labelOf = (col: string) => (yFieldLabels && yFieldLabels[col]) ? yFieldLabels[col] : col;
    const TOOLTIP_STYLE = {
      backgroundColor: styles.tooltipBg, borderColor: styles.tooltipBorder, borderWidth: 1,
      padding: [8, 12], borderRadius: 8,
      textStyle: { color: styles.tooltipText, fontSize: 12 },
    };
    const TITLE_STYLE = (text: string) => ({
      text, left: "center",
      textStyle: { color: styles.textColor, fontSize: 13, fontWeight: 500 },
    });

    if (type === "pie") {
      const yIdx = yCols.length > 0 ? cols.indexOf(yCols[0]) : -1;
      const data = table.rows.filter((r) => r[xIdx] != null && r[yIdx] != null).map((r) => ({ name: String(r[xIdx]), value: num(r[yIdx]) }));
      return { color: styles.palette, title: title ? { ...TITLE_STYLE(title), top: 8 } : undefined, tooltip: { trigger: "item", formatter: "{b}: {c} ({d}%)", ...TOOLTIP_STYLE }, legend: { bottom: 2, type: "scroll", textStyle: { color: styles.legendColor, fontSize: 11 }, itemWidth: 8, itemHeight: 8, itemGap: 12 }, series: [{ type: "pie", radius: ["40%", "60%"], center: ["50%", "50%"], data, label: { color: styles.textColor, fontSize: 11, formatter: "{b}: {d}%", fontWeight: 500 }, labelLine: { lineStyle: { color: styles.lineStyleColor } }, itemStyle: { borderRadius: 6, borderColor: styles.tooltipBg, borderWidth: 2 } }] };
    }
    if (type === "scatter") {
      const yIdx = yCols.length > 0 ? cols.indexOf(yCols[0]) : -1;
      const data = table.rows.filter((r) => r[xIdx] != null && r[yIdx] != null).map((r) => [num(r[xIdx]), num(r[yIdx])]);
      return { color: styles.palette, title: title ? TITLE_STYLE(title) : undefined, tooltip: { trigger: "item", ...TOOLTIP_STYLE }, grid: { left: 60, right: 24, top: title ? 44 : 20, bottom: 32, containLabel: true }, xAxis: { type: "value", name: xField ?? cols[xIdx] ?? "X", nameTextStyle: { color: styles.axisLabelColor, fontSize: 11 }, scale: true, ...AXIS_STYLE }, yAxis: { type: "value", name: yCols[0] ?? "Y", nameTextStyle: { color: styles.axisLabelColor, fontSize: 11 }, scale: true, ...AXIS_STYLE }, series: [{ type: "scatter", data, symbolSize: 8, itemStyle: { opacity: 0.8, borderColor: styles.tooltipBg, borderWidth: 1.5 }, emphasis: { focus: "self", itemStyle: { opacity: 1 } } }] };
    }
    if (type === "funnel") {
      const yIdx = yCols.length > 0 ? cols.indexOf(yCols[0]) : -1;
      const data = table.rows.filter((r) => r[xIdx] != null && r[yIdx] != null).map((r) => ({ name: String(r[xIdx]), value: num(r[yIdx]) }));
      return { color: styles.palette, title: title ? { ...TITLE_STYLE(title), top: 8 } : undefined, tooltip: { trigger: "item", formatter: "{b}: {c}", ...TOOLTIP_STYLE }, legend: { bottom: 2, type: "scroll", textStyle: { color: styles.legendColor, fontSize: 11 }, itemWidth: 8, itemHeight: 8, itemGap: 12 }, series: [{ type: "funnel", data, sort: "descending", gap: 2, label: { color: styles.textColor, fontSize: 11, formatter: "{b}: {c}", fontWeight: 500 }, itemStyle: { borderRadius: 4, borderColor: styles.tooltipBg, borderWidth: 1.5 } }] };
    }
    if (type === "gauge") {
      const yIdx = yCols.length > 0 ? cols.indexOf(yCols[0]) : findFirstNumeric(table.columnTypes, xIdx);
      const label = yIdx >= 0 ? cols[yIdx] : (xField ?? "值");
      const value = yIdx >= 0 && table.rows.length > 0 ? num(table.rows[0][yIdx]) : 0;
      return { color: styles.palette, title: title ? { ...TITLE_STYLE(title), top: 8 } : undefined, tooltip: { ...TOOLTIP_STYLE }, series: [{ type: "gauge", center: ["50%", "55%"], radius: "70%", min: 0, max: (() => { if (value <= 0) return 100; const mag = Math.pow(10, Math.floor(Math.log10(value))); const norm = value / mag; const nice = norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10; return nice * mag; })(), progress: { show: true, width: 12, itemStyle: { color: styles.palette[0] } }, axisLine: { lineStyle: { width: 12, color: [[1, styles.gaugeLineColor]] } }, pointer: { width: 4, itemStyle: { color: styles.axisLabelColor } }, axisTick: { show: false }, splitLine: { length: 8, lineStyle: { color: styles.axisLineColor, width: 1.5 } }, axisLabel: { color: styles.legendColor, fontSize: 10, distance: 16 }, detail: { valueAnimation: true, color: styles.textColor, fontSize: 18, fontWeight: 600, offsetCenter: [0, "62%"], formatter: `{value}` }, data: [{ value, name: label }] }] };
    }

    // bar / line
    const rightSet = new Set(rightYFields ?? []);
    const hasRight = rightSet.size > 0 && yCols.some((yn) => rightSet.has(yn));
    const leftCols = yCols.filter((yn) => !rightSet.has(yn));
    const rightCols = yCols.filter((yn) => rightSet.has(yn));
    const leftAxisName = leftCols.length === 1 ? labelOf(leftCols[0]) : undefined;
    const rightAxisName = rightCols.length === 1 ? labelOf(rightCols[0]) : undefined;
    const categoryData = table.rows.map((r) => String(r[xIdx >= 0 ? xIdx : 0] ?? ""));
    const rotated = categoryData.length > 8;
    const series = yCols.map((yn, colorOffset) => {
      const yi = cols.indexOf(yn);
      const baseColor = styles.palette[colorOffset % styles.palette.length];
      return {
        name: labelOf(yn), type, yAxisIndex: rightSet.has(yn) ? 1 : 0,
        data: table.rows.map((r) => num(r[yi])), smooth: type === "line",
        ...(type === "bar" ? { itemStyle: { borderRadius: [4, 4, 0, 0], color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [{ offset: 0, color: baseColor }, { offset: 1, color: hexToRgba(baseColor, 0.35) }]) }, barMaxWidth: 24 } : {}),
        ...(type === "line" ? { symbol: "circle", symbolSize: 6, lineStyle: { width: 3, shadowColor: hexToRgba(baseColor, 0.2), shadowBlur: 6, shadowOffsetY: 3 }, itemStyle: { color: baseColor }, areaStyle: { color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [{ offset: 0, color: hexToRgba(baseColor, 0.15) }, { offset: 1, color: hexToRgba(baseColor, 0.01) }]) } } : {}),
      };
    });
    const yAxis = hasRight
      ? [{ type: "value" as const, position: "left" as const, name: leftAxisName, nameTextStyle: AXIS_NAME_STYLE, ...AXIS_STYLE }, { type: "value" as const, position: "right" as const, name: rightAxisName, nameTextStyle: AXIS_NAME_STYLE, ...AXIS_STYLE, splitLine: { show: false } }]
      : { type: "value" as const, name: yCols.length === 1 ? labelOf(yCols[0]) : undefined, nameTextStyle: AXIS_NAME_STYLE, ...AXIS_STYLE };
    return { color: styles.palette, title: title ? TITLE_STYLE(title) : undefined, tooltip: { trigger: "axis", ...TOOLTIP_STYLE }, legend: { bottom: 2, type: "scroll", textStyle: { color: styles.legendColor, fontSize: 11 }, itemWidth: 8, itemHeight: 8, itemGap: 12 }, grid: { left: 60, right: hasRight ? 56 : 24, top: title ? 44 : 20, bottom: rotated ? 72 : 52, containLabel: true }, xAxis: { type: "category", data: categoryData, ...AXIS_STYLE, axisLabel: { ...AXIS_STYLE.axisLabel, rotate: rotated ? 30 : 0 } }, yAxis, series };
  }

  function render() {
    if (!chart || !container) return;
    const opt = buildOption(chartType(), props.seg.table, props.seg.xField, props.seg.yFields, props.seg.rightYFields, props.seg.yFieldLabels, props.seg.title);
    chart.setOption(opt, true);
  }

  createEffect(() => {
    currentTheme();
    render();
    if (fullscreenChart) {
      const opt = buildOption(chartType(), props.seg.table, props.seg.xField, props.seg.yFields, props.seg.rightYFields, props.seg.yFieldLabels, props.seg.title);
      fullscreenChart.setOption(opt, true);
    }
  });

  onMount(() => {
    if (!container) return;
    chart = echarts.init(container);
    render();
    requestAnimationFrame(() => { chart?.resize(); });
    const ro = new ResizeObserver(() => { chart?.resize(); });
    ro.observe(container);
    onCleanup(() => { ro.disconnect(); if (chart) { chart.dispose(); chart = undefined; } });
  });

  function switchType(t: ChartType) {
    if (t === chartType()) return;
    setChartType(t);
    render();
    if (fullscreenChart) {
      const opt = buildOption(t, props.seg.table, props.seg.xField, props.seg.yFields, props.seg.rightYFields, props.seg.yFieldLabels, props.seg.title);
      fullscreenChart.setOption(opt, true);
    }
  }

  const switchable = SWITCHABLE_TYPES.includes(props.seg.chartType as ChartType);

  return (
    <div class="chart-seg">
      <div class="chart-seg__toolbar">
        <div class="chart-seg__toolbar-left">
          <Show when={switchable}>
            <For each={CHART_TYPES}>
              {(ct) => (
                <button class="chart-seg__type-btn" classList={{ active: chartType() === ct.type }} title={ct.label} onClick={() => switchType(ct.type)}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px; display: inline-block; vertical-align: middle;" innerHTML={ct.svg} />
                  <span>{ct.label}</span>
                </button>
              )}
            </For>
          </Show>
        </div>
        <div class="chart-seg__toolbar-right">
          <button class="chart-seg__action-btn" title="保存为图片" onClick={saveAsImage}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
              <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
              <polyline points="7 10 12 15 17 10"></polyline>
              <line x1="12" y1="15" x2="12" y2="3"></line>
            </svg>
          </button>
          <button class="chart-seg__action-btn" title="全屏查看" onClick={() => setIsFullScreen(true)}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
              <path d="M8 3H5a2 2 0 0 0-2 2v3m18 0V5a2 2 0 0 0-2-2h-3m0 18h3a2 2 0 0 0 2-2v-3M3 16v3a2 2 0 0 0 2 2h3"></path>
            </svg>
          </button>
        </div>
      </div>
      <div ref={container} class="chart-seg__canvas" />
      <Show when={isFullScreen()}>
        <Portal>
          <div class="chart-fullscreen-overlay">
            <div class="chart-fullscreen-header" classList={{ "mac-padding": isMac }} data-tauri-drag-region>
              <span class="chart-fullscreen-title">{props.seg.title || "图表预览"}</span>
              <Show when={switchable}>
                <div class="chart-fullscreen-tabs">
                  <For each={CHART_TYPES}>
                    {(ct) => (
                      <button class="chart-seg__type-btn" classList={{ active: chartType() === ct.type }} title={ct.label} onClick={() => switchType(ct.type)}>
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px; display: inline-block; vertical-align: middle;" innerHTML={ct.svg} />
                        <span>{ct.label}</span>
                      </button>
                    )}
                  </For>
                </div>
              </Show>
              <div class="chart-fullscreen-actions">
                <button class="chart-fullscreen-btn" onClick={saveAsImage} title="保存为图片">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 15px; height: 15px;">
                    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
                    <polyline points="7 10 12 15 17 10"></polyline>
                    <line x1="12" y1="15" x2="12" y2="3"></line>
                  </svg>
                </button>
                <button class="chart-fullscreen-btn" onClick={() => setIsFullScreen(false)} title="退出全屏">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 15px; height: 15px;">
                    <path d="M4 14h6v6m10-6h-6v6M4 10h6V4m10 6h-6V4"></path>
                  </svg>
                </button>
              </div>
            </div>
            <div class="chart-fullscreen-body" onClick={() => setIsFullScreen(false)}>
              <div
                ref={(el) => {
                  if (el) {
                    fullscreenContainer = el;
                    fullscreenChart = echarts.init(el);
                    const opt = buildOption(chartType(), props.seg.table, props.seg.xField, props.seg.yFields, props.seg.rightYFields, props.seg.yFieldLabels, props.seg.title);
                    fullscreenChart.setOption(opt);
                    requestAnimationFrame(() => { fullscreenChart?.resize(); });
                    const ro = new ResizeObserver(() => { fullscreenChart?.resize(); });
                    ro.observe(el);
                    (el as any)._cleanup = () => { ro.disconnect(); if (fullscreenChart) { fullscreenChart.dispose(); fullscreenChart = undefined; } };
                  } else {
                    if (fullscreenContainer && (fullscreenContainer as any)._cleanup) { (fullscreenContainer as any)._cleanup(); }
                    fullscreenContainer = undefined;
                  }
                }}
                class="chart-fullscreen-canvas"
                onClick={(e) => e.stopPropagation()}
              />
            </div>
          </div>
        </Portal>
      </Show>
    </div>
  );
}

// ── helpers ──

function num(v: unknown): number {
  if (typeof v === "number") return v;
  if (typeof v === "string") { const n = parseFloat(v); return isNaN(n) ? 0 : n; }
  if (typeof v === "boolean") return v ? 1 : 0;
  return 0;
}

function findFirstNumeric(types: string[], excludeIdx: number): number {
  for (let i = 0; i < types.length; i++) {
    if (i === excludeIdx) continue;
    const t = types[i].toUpperCase();
    if (t.includes("INT") || t.includes("FLOAT") || t.includes("DOUBLE") || t.includes("DECIMAL")) return i;
  }
  return -1;
}

function findDimensionCol(types: string[]): number {
  for (let i = 0; i < types.length; i++) {
    const t = types[i].toUpperCase();
    if (!t.includes("INT") && !t.includes("FLOAT") && !t.includes("DOUBLE") && !t.includes("DECIMAL")) return i;
  }
  return 0;
}

function findNumericCols(cols: string[], types: string[], excludeIdx: number): string[] {
  const out: string[] = [];
  for (let i = 0; i < cols.length; i++) {
    if (i === excludeIdx) continue;
    const t = types[i]?.toUpperCase() ?? "";
    if (t.includes("INT") || t.includes("FLOAT") || t.includes("DOUBLE") || t.includes("DECIMAL")) out.push(cols[i]);
  }
  if (out.length === 0 && cols.length > 0) return cols.filter((_, i) => i !== excludeIdx);
  return out;
}
