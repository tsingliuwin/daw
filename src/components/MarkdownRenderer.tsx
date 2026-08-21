import { createMemo, createResource } from "solid-js";
import { marked, Renderer } from "marked";
import {
  createHighlighter,
  type Highlighter,
  type ShikiTransformer,
} from "shiki";
import {
  activeCodeTheme,
  codeLineNumbers,
  ALL_CODE_THEMES,
} from "../lib/codeConfig";

// Configure marked for safe rendering
marked.setOptions({
  breaks: true,       // GitHub-style line breaks
  gfm: true,          // GitHub Flavored Markdown (tables, strikethrough, etc.)
});

/** Languages we highlight. OA scope: json + sql + markdown. Add a lang here
 *  AND in the highlighter init below to extend. */
const SUPPORTED_LANGS = ["sql", "markdown", "json"] as const;

// ---------------------------------------------------------------------------
// Shiki highlighter — single async instance, shared by all MarkdownRenderer
// instances. Loaded once with the supported langs + every selectable theme so
// theme switching never needs a reload.
// ---------------------------------------------------------------------------
const highlighterPromise: Promise<Highlighter> = createHighlighter({
  langs: [...SUPPORTED_LANGS],
  themes: [...ALL_CODE_THEMES],
});

// The resolved highlighter (null until the promise settles). Read in render.
const [highlighter] = createResource<Highlighter>(() => highlighterPromise);

/** Add a line-number gutter to Shiki's per-line output. Shiki gives us a
 *  `<span class="line">` per line; we tag it so CSS can render a counter. */
function transformerLineNumbers(enable: boolean): ShikiTransformer {
  return {
    name: "line-numbers",
    line(hast) {
      if (!enable) return;
      hast.properties.class = "line numbered-line";
    },
  };
}

/** Render a single code block to highlighted HTML. Falls back to a plain
 * `<pre><code>` for unsupported languages or before the highlighter loads. */
function highlightCode(code: string, langRaw: string): string {
  const lang = langRaw.toLowerCase().trim();
  const theme = activeCodeTheme();
  const hl = highlighter();
  const supported = (SUPPORTED_LANGS as readonly string[]).includes(lang);

  // 流式渲染期（streaming 实例，见 parseWithPlainCode）：Shiki 全量语法高亮是
  // 流式期最贵的单点（一个代码块几十毫秒，逐冲刷重算直接打满主线程），先出
  // 纯代码，段落闭合/流结束后由非 streaming 渲染补齐高亮。
  if (streamingParseDepth > 0) {
    const escaped = code.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    return `<pre><code class="language-${lang || "text"}">${escaped}</code></pre>`;
  }

  if (!hl || !supported) {
    const escaped = code
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
    return `<pre><code class="language-${lang || "text"}">${escaped}</code></pre>`;
  }

  try {
    return hl.codeToHtml(code, {
      lang,
      theme,
      transformers: [
        transformerLineNumbers(codeLineNumbers()),
      ],
    });
  } catch {
    // Unknown theme/lang edge cases — degrade gracefully to plain code.
    const escaped = code.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    return `<pre><code class="language-${lang}">${escaped}</code></pre>`;
  }
}

// Custom renderer: only override `code` so Shiki handles fenced code blocks;
// everything else uses marked's default.
const renderer = new Renderer();
renderer.code = ({ text, lang }: { text: string; lang?: string }) =>
  highlightCode(text, lang || "");

marked.use({ renderer });

// marked 的 renderer 是模块级共享的；marked.parse 同步执行，这里用计数器在
// streaming 实例解析期间打开 highlightCode 的"跳过 Shiki"开关（单线程同步安全）。
let streamingParseDepth = 0;
function parseMarked(content: string, streaming: boolean): string {
  streamingParseDepth += streaming ? 1 : 0;
  try {
    return marked.parse(content) as string;
  } finally {
    streamingParseDepth -= streaming ? 1 : 0;
  }
}

// Re-configure marked when the renderer closure needs refreshing is unnecessary —
// highlightCode reads signals at call time, and we re-run parse in the memo below
// whenever those signals change.

/**
 * Lightweight Markdown renderer for agent chat messages.
 *
 * Supports: headings, bold/italic, inline code, code blocks (Shiki-highlighted for
 * json/sql/markdown, with copy button via CSS), tables, lists, links. Strips dangerous
 * HTML (script, iframe, etc.). Re-renders when the active code theme or line-number
 * setting changes.
 */
export default function MarkdownRenderer(props: {
  content: string;
  onWikiLink?: (table: string) => void;
  /** 流式渲染中的实例：跳过 Shiki 高亮（见 highlightCode），闭合后由
   * streaming=false 的渲染补齐。 */
  streaming?: boolean;
}) {
  const html = createMemo(() => {
    if (!props.content) return "";
    void activeCodeTheme();
    void codeLineNumbers();
    void highlighter.state;
    const raw = parseMarked(props.content, !!props.streaming);
    return raw
      .replace(/<script\b[^<]*(?:(?!<\/script>)<[^<]*)*<\/script>/gi, "")
      .replace(/<iframe\b[^>]*>.*?<\/iframe>/gi, "")
      .replace(/<object\b[^>]*>.*?<\/object>/gi, "")
      .replace(/<embed\b[^>]*\/?>/gi, "")
      .replace(/on\w+\s*=/gi, "data-blocked=")
      .replace(/\{\{\s*chart:[^}<]*\}?\}?/g, '<span class="chart-ref-badge">📊</span>')
      // OKF 内链 [[table_name]] → 可点击链接（点击加载目标表的知识库内容）。
      .replace(/\[\[([^\]]+)\]\]/g, '<a class="okf-wikilink" data-okf-table="$1" href="#">$1</a>');
  });

  return (
    <div
      class="md-rendered"
      innerHTML={html()}
      onClick={(e) => {
        const target = (e.target as HTMLElement).closest(".okf-wikilink");
        if (target) {
          e.preventDefault();
          const table = target.getAttribute("data-okf-table");
          if (table && props.onWikiLink) {
            props.onWikiLink(table);
          }
        }
      }}
    />
  );
}
