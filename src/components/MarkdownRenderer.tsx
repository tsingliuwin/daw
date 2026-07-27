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
 *  `<pre><code>` for unsupported languages or before the highlighter loads. */
function highlightCode(code: string, langRaw: string): string {
  const lang = langRaw.toLowerCase().trim();
  const theme = activeCodeTheme();
  const hl = highlighter();
  const supported = (SUPPORTED_LANGS as readonly string[]).includes(lang);

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
export default function MarkdownRenderer(props: { content: string }) {
  const html = createMemo(() => {
    if (!props.content) return "";
    // Read these signals so the memo re-runs on theme/lineNumber change. The
    // highlighter() resource read also retriggers once it resolves.
    void activeCodeTheme();
    void codeLineNumbers();
    void highlighter.state;
    const raw = marked.parse(props.content) as string;
    // Basic XSS sanitization: strip script/iframe/object/embed tags
    return raw
      .replace(/<script\b[^<]*(?:(?!<\/script>)<[^<]*)*<\/script>/gi, "")
      .replace(/<iframe\b[^>]*>.*?<\/iframe>/gi, "")
      .replace(/<object\b[^>]*>.*?<\/object>/gi, "")
      .replace(/<embed\b[^>]*\/?>/gi, "")
      .replace(/on\w+\s*=/gi, "data-blocked=");
  });

  return <div class="md-rendered" innerHTML={html()} />;
}
