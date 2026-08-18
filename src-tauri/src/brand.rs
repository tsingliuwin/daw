//! Brand customization: the user-editable `~/.daw/brand.json`.
//!
//! On first startup a template file is generated from the Daw defaults, and
//! the user edits it to turn the app into their own workstation (name, tagline,
//! logos, welcome copy, about text, assistant identity). Changes take effect
//! on restart — no rebuild required. Every field falls back to the default
//! when missing or unparseable, so a broken config never blocks startup.

use serde::{Deserialize, Serialize};

/// Copy for one home-view scenario card ("task" = 日常任务, "data_analysis" = 数据分析).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScenarioText {
    pub label: String,
    pub subtitle: String,
    pub placeholder: String,
}

impl Default for ScenarioText {
    fn default() -> Self {
        Self {
            label: String::new(),
            subtitle: String::new(),
            placeholder: String::new(),
        }
    }
}

/// Home-view welcome texts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HomeTexts {
    pub welcome_title: String,
    pub welcome_subtitle: String,
    pub task: ScenarioText,
    pub data_analysis: ScenarioText,
}

impl Default for HomeTexts {
    fn default() -> Self {
        Self {
            welcome_title: String::new(),
            welcome_subtitle: String::new(),
            task: ScenarioText::default(),
            data_analysis: ScenarioText::default(),
        }
    }
}

/// The full brand surface. `#[serde(default)]` means a user file with missing
/// fields still parses, filling the gaps with [`BrandConfig::default`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BrandConfig {
    /// App name shown in the UI, window title, about dialog, and agent identity.
    pub app_name: String,
    /// Short tagline shown next to the name.
    pub tagline: String,
    /// One-line product description in the about dialog.
    pub about_description: String,
    /// Custom logo filename relative to the app data dir (`~/.daw/`); empty
    /// means "use the built-in logo" (`/logo.png` on light, `/logo_white.png`
    /// on dark themes).
    pub logo_light: String,
    pub logo_dark: String,
    pub home: HomeTexts,
}

impl Default for BrandConfig {
    fn default() -> Self {
        Self {
            app_name: "Daw".to_string(),
            tagline: "Data Agent Workstation".to_string(),
            about_description:
                "用对话驱动你的数据与任务。Daw 是开源的 Data Agent Workstation，改一份 brand.json 就能定制成你自己的专属工作台。"
                    .to_string(),
            logo_light: String::new(),
            logo_dark: String::new(),
            home: HomeTexts {
                welcome_title: "Daw".to_string(),
                welcome_subtitle: "用对话驱动数据与任务".to_string(),
                task: ScenarioText {
                    label: "日常任务".to_string(),
                    subtitle: "信息检索、知识问答、文案撰写——用对话完成任务，随时待命。".to_string(),
                    placeholder: "试试：「调研一下 XX 行业的最新动态」或「帮我写一份本周工作总结」"
                        .to_string(),
                },
                data_analysis: ScenarioText {
                    label: "数据分析".to_string(),
                    subtitle: "查询数据库、生成图表、沉淀业务知识——用对话驱动数据分析。".to_string(),
                    placeholder: "试试：「查看有哪些数据表」或「统计各区域今年销量并画个柱状图」"
                        .to_string(),
                },
            },
        }
    }
}

/// Read `~/.daw/brand.json`, creating a template file from the defaults on
/// first run. Parse failures fall back to the defaults — brand config errors
/// must never block startup, and the template doubles as documentation.
pub fn load_brand() -> BrandConfig {
    let Ok(dir) = crate::db::get_app_dir() else {
        return BrandConfig::default();
    };
    let path = dir.join("brand.json");
    if !path.exists() {
        if let Ok(template) = serde_json::to_string_pretty(&BrandConfig::default()) {
            let _ = std::fs::write(&path, template);
        }
        return BrandConfig::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => BrandConfig::default(),
    }
}

/// Resolve a custom logo (`kind` = "light" | "dark") to a base64 data URI the
/// webview can render directly. `Ok(None)` when the user hasn't configured one
/// or the file is missing — the frontend then falls back to the built-in logo.
pub fn load_logo(kind: &str) -> Result<Option<String>, String> {
    let brand = load_brand();
    let filename = if kind == "light" {
        &brand.logo_light
    } else {
        &brand.logo_dark
    };
    if filename.is_empty() {
        return Ok(None);
    }
    let dir = crate::db::get_app_dir()?;
    // Only plain filenames inside the data dir; reject absolute/escaping paths.
    let logo_path = dir.join(filename);
    if !logo_path.starts_with(&dir) || !logo_path.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(&logo_path).map_err(|e| format!("读取自定义 logo 失败: {e}"))?;
    let is_jpeg = logo_path
        .extension()
        .and_then(|e| e.to_str())
        .map_or(false, |e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
    let mime = if is_jpeg { "image/jpeg" } else { "image/png" };
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(Some(format!("data:{mime};base64,{b64}")))
}