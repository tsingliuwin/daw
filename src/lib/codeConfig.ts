import { createSignal, createMemo } from "solid-js";
import { currentTheme } from "./theme";

/**
 * 代码预览的全局配置（设置页「代码预览」tab）。
 *
 * 与 theme.ts 同样的 signal 模式，但这里带 localStorage 持久化——用户选的
 * 高亮主题/字号等刷新后仍生效。MarkdownRenderer 读取这些信号来驱动 Shiki
 * 渲染，设置页改动后所有已渲染的代码块会随信号自动重渲染。
 *
 * 支持的语言范围刻意收窄为 sql + markdown（数据探索场景），如需扩展在
 * MarkdownRenderer 的 highlighter 初始化里加 lang 即可。
 */

/** 浅色代码主题可选值（Shiki theme id）。
 *  刻意只保留一个，使其与 CodeMirror 侧的 githubLight 一一对应——
 *  CodeMirror 与 Shiki 主题体系不互通，保证「切主题编辑器也变」的唯一稳妥
 *  做法是两边都用同名 GitHub 主题。如要扩主题，需同时在 CodeMirror 侧找到
 *  对应包并在 cmTheme() 里加映射。 */
export const LIGHT_CODE_THEMES = ["github-light"] as const;

/** 深色代码主题可选值（Shiki theme id）。与 LIGHT_CODE_THEMES 同理，对应
 *  CodeMirror 的 githubDark。 */
export const DARK_CODE_THEMES = ["github-dark"] as const;

type LightCodeTheme = (typeof LIGHT_CODE_THEMES)[number];
type DarkCodeTheme = (typeof DARK_CODE_THEMES)[number];

const STORAGE_KEY = "code_preview_config";

interface CodeConfig {
  lightTheme: LightCodeTheme;
  darkTheme: DarkCodeTheme;
  fontSize: number;
  lineNumbers: boolean;
  wrap: boolean;
}

const DEFAULT_CONFIG: CodeConfig = {
  lightTheme: "github-light",
  darkTheme: "github-dark",
  fontSize: 13,
  lineNumbers: true,
  wrap: true,
};

function loadConfig(): CodeConfig {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_CONFIG;
    const parsed = JSON.parse(raw);

    const lightTheme = (LIGHT_CODE_THEMES as readonly string[]).includes(parsed.lightTheme)
      ? parsed.lightTheme
      : "github-light";
    const darkTheme = (DARK_CODE_THEMES as readonly string[]).includes(parsed.darkTheme)
      ? parsed.darkTheme
      : "github-dark";

    return {
      ...DEFAULT_CONFIG,
      ...parsed,
      lightTheme,
      darkTheme,
    };
  } catch {
    return DEFAULT_CONFIG;
  }
}

const initial = loadConfig();

export const [lightCodeTheme, setLightCodeTheme] = createSignal<LightCodeTheme>(initial.lightTheme);
export const [darkCodeTheme, setDarkCodeTheme] = createSignal<DarkCodeTheme>(initial.darkTheme);
export const [codeFontSize, setCodeFontSize] = createSignal<number>(initial.fontSize);
export const [codeLineNumbers, setCodeLineNumbers] = createSignal<boolean>(initial.lineNumbers);
export const [codeWrap, setCodeWrap] = createSignal<boolean>(initial.wrap);

/** 持久化：任一配置变更后写回 localStorage。 */
function persist() {
  const cfg: CodeConfig = {
    lightTheme: lightCodeTheme(),
    darkTheme: darkCodeTheme(),
    fontSize: codeFontSize(),
    lineNumbers: codeLineNumbers(),
    wrap: codeWrap(),
  };
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(cfg));
  } catch {
    /* quota / private mode — silently ignore */
  }
}

// 包装 setter 以便同时持久化。设置页直接用这些即可。
// 注：主题选项已收敛为单一 GitHub Light/Dark，浅/深主题不再可切换，故无
// 对应的 setLightCodeThemeP / setDarkCodeThemeP；其余配置仍可调。
export const setCodeFontSizeP = (v: number) => { setCodeFontSize(v); persist(); };
export const setCodeLineNumbersP = (v: boolean) => { setCodeLineNumbers(v); persist(); };
export const setCodeWrapP = (v: boolean) => { setCodeWrap(v); persist(); };

/** 当前界面主题下应使用的高亮主题 id：
 *  light 界面 → 浅色代码主题；geek-dark / classic-dark → 深色代码主题。 */
export const activeCodeTheme = createMemo<string>(() =>
  currentTheme() === "light" ? lightCodeTheme() : darkCodeTheme(),
);

/** 当前生效的代码主题是否为浅色。SQL 编辑器（CodeMirror）据此在
 *  githubLight / githubDark 间切换——CodeMirror 主题与 Shiki 主题是两套
 *  体系，无法逐一对应，故按明暗映射（浅色 Shiki → githubLight 等）。 */
export const isLightCodeTheme = createMemo<boolean>(() =>
  (LIGHT_CODE_THEMES as readonly string[]).includes(activeCodeTheme()),
);

/** MarkdownRenderer / 预览块需要加载的所有主题（浅色 + 深色合集），用于
 *  createHighlighter 的 themes 参数。 */
export const ALL_CODE_THEMES: readonly string[] = [
  ...LIGHT_CODE_THEMES,
  ...DARK_CODE_THEMES,
];
