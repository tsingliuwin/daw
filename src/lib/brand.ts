import { createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { currentTheme } from "./theme";

export interface ScenarioText {
  label: string;
  subtitle: string;
  placeholder: string;
}

export interface BrandConfig {
  app_name: string;
  tagline: string;
  about_description: string;
  logo_light: string;
  logo_dark: string;
  home: {
    welcome_title: string;
    welcome_subtitle: string;
    task: ScenarioText;
    data_analysis: ScenarioText;
  };
}

/** Daw 默认品牌。后端 brand.json 缺失或读取失败时的兜底，与 Rust `BrandConfig::default` 一致。 */
const DEFAULTS: BrandConfig = {
  app_name: "寒鸦数据工作台",
  tagline: "Data Agent Workstation",
  about_description:
    "用对话驱动你的数据与任务。Daw 是开源的 Data Agent Workstation，改一份 brand.json 就能定制成你自己的专属工作台。",
  logo_light: "",
  logo_dark: "",
  home: {
    welcome_title: "寒鸦数据工作台",
    welcome_subtitle: "用对话驱动数据与任务",
    task: {
      label: "日常任务",
      subtitle: "信息检索、知识问答、文案撰写——用对话完成任务，随时待命。",
      placeholder: "试试：「调研一下 XX 行业的最新动态」或「帮我写一份本周工作总结」",
    },
    data_analysis: {
      label: "数据分析",
      subtitle: "查询数据库、生成图表、沉淀业务知识——用对话驱动数据分析。",
      placeholder: "试试：「查看有哪些数据表」或「统计各区域今年销量并画个柱状图」",
    },
  },
};

/** 生效中的品牌配置（app 名称、文案等）。 */
export const [brand, setBrand] = createSignal<BrandConfig>(DEFAULTS);

/** 自定义 logo（brand.json 配了文件时后端返回的 base64 data URI）。 */
const [customLogos, setCustomLogos] = createSignal<{ light?: string; dark?: string }>({});

/** 当前主题下的 logo 来源：自定义（data URI）优先，否则内置资源。 */
export const logoSrc = () =>
  currentTheme() === "light"
    ? customLogos().light ?? "/logo.png"
    : customLogos().dark ?? "/logo_white.png";

/** App onMount 调用：拉取品牌配置与自定义 logo，并同步浏览器标题。 */
export async function loadBrandFromBackend() {
  try {
    const cfg = await invoke<BrandConfig>("get_brand_config");
    if (cfg?.app_name) setBrand(cfg);
    const logos: { light?: string; dark?: string } = {};
    for (const kind of ["light", "dark"] as const) {
      const uri = await invoke<string | null>("get_brand_logo", { kind });
      if (uri) logos[kind] = uri;
    }
    setCustomLogos(logos);
  } catch {
    /* 后端不可用时静默用默认 Daw 品牌 */
  }
  document.title = brand().app_name;
}