# AGENTS.md — 项目开发经验记录

## 布局尺寸纪律：壳层只用纯 CSS 百分比（三轮教训）

根容器/壳层尺寸**禁止内联像素校准**（b404fe9 引入、903d6a8 加固、4b1fedd 彻底移除）：

- **根因**：流式输出高峰期主线程被渲染任务洪水占满（实测 Runtime.evaluate 排队 12s+ 无响应），任何依赖 JS 事件（Tauri `onResized`、`window.resize`）+ IPC（`innerSize()`）的尺寸校准在该场景**整体失效**，内联像素尺寸闩在旧值 → 最大化后右侧大片空白、还原后内容被裁。
- **结论**：CSS `100%` 由渲染管线驱动，不占 JS 任务队列，洪水期截图验证最大化/还原均正确自愈；还天然避开界面缩放（`documentElement.style.zoom`）≠100% 时内联像素被 zoom 倍率放大的隐患。
- **排障手法**：WebView2 远程调试口（`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222` + 独立 `WEBVIEW2_USER_DATA_FOLDER`）+ 裸 CDP（Runtime.evaluate/Page.captureScreenshot）；合成器截图在主线程饿死时仍可用，是洪水期唯一可靠的观测手段。
- 同类风险：任何「窗口状态 → JS → 内联样式」的链路在流式期都不可依赖；需要样式响应窗口尺寸时优先 CSS（含 container query）。
- **节流不够，还要降单元成本**：delta 节流（150ms 冲刷）只降频率；真实重任务里每次冲刷仍对整段累积文本全量 marked 解析 + Shiki 高亮 + innerHTML 重建，渲染管线照样饱和（实测求值排队 14.5s、合成器帧停滞、WebView2 表面卡死且整页重载不恢复）。根治靠 ChatView 的 `isLiveTextTail`：流式尾部 text 段纯文本直出，流结束才 markdown 终渲。

## DuckDB postgres_query 关键经验

**postgres_query 的第一个参数必须是 catalog 别名（如 `db_xxx`），不能是连接串（如 `host=... port=...`）。**

- **传连接串**：触发 postgres 扩展重新初始化 catalog（执行元数据扫描），在 Hologres 等兼容 PG 协议但系统目录表有差异的数据库上报 `missing FROM-clause entry for table "pg_namespace"` 错误。
- **传 catalog 别名**：复用已 ATTACH 的连接，不触发重新扫描。

正确用法：
```sql
-- ATTACH 后用别名调 postgres_query
SELECT * FROM postgres_query('db_demo', 'SELECT * FROM pg_catalog.pg_class ...')
```

错误用法：
```sql
-- 传连接串会触发元数据扫描
SELECT * FROM postgres_query('host=... port=... dbname=...', 'SELECT ...')
```

## Hologres 兼容性设计（三层隔离）

Agent 从不直接查 ATTACH 的远程 catalog（沿用早期数据湖原型的设计）：

1. **list_tables**：只查本地默认 catalog（`lake`，DuckLake 挂载的工作区 catalog），不枚举远程 `db_` catalog
2. **list_remote_tables**：postgres 类型用 `postgres_query` 下推查 `pg_catalog`（不触发 catalog 扫描）
3. **register_table**：检测 foreign table（`relkind='f'`），外表用 `postgres_query` 创建视图，普通表走 catalog 引用

外表在 `information_schema` 里可能不显示，需要用 `pg_catalog.pg_class` 查（`relkind IN ('r','v','m','f','p')`）。

## 品牌定制（brand.json）

本项目不绑定任何公司，品牌面（名称/logo/文案/助手身份）全部由 `~/.daw/brand.json` 驱动：

- Rust 侧: `src-tauri/src/brand.rs`——首次启动生成默认模板（Daw 品牌），解析失败回落默认值，绝不阻断启动；`get_brand_config` / `get_brand_logo` 两个命令暴露给前端。
- 前端侧: `src/lib/brand.ts`——`brand()` 信号 + `logoSrc()`，App onMount 时 `loadBrandFromBackend()`；窗口标题由 Rust setup 按 `app_name` set_title。
- 系统提示词: `usage.rs` 的 PREAMBLE 以 `{app_name}` 占位符声明助手身份，`general_preamble` / `data_analysis_preamble` 在 runner 组装前替换品牌名。
- **纪律**: 组件里不得硬编码产品名/欢迎文案；新加品牌相关文案先考虑进 `brand.json`（README 有字段表）。

## 数据目录（~/.daw）

- 全部本机数据集中在 `~/.daw/`：`daw.db`（元数据）、`settings.json`（LLM/搜索/数据源）、`<工作区>/`（聊天 jsonl、.lake、okf）。
- `db::get_app_dir()` 是唯一入口；旧版 `~/.aioa` 首次启动时自动 rename 迁移。
- 新代码写路径一律走 `db::get_app_dir()`，不要手拼 `~/.daw`。

## 发版与自动更新

- **双远程**:日常开发提交走 Gitee（origin）避免 GitHub 直连不稳；发版时把 main + 注解 tag（`vX.Y.Z`）推到 GitHub（`git@github.com:tsingliuwin/daw.git`），构建全部由 GitHub Actions 完成。
- **两个工作流**:`.github/workflows/ci.yml`（缓存预热）与 `release.yml`（发版）通过 `swatinem/rust-cache` 的 `shared-key: "tauri-<os>-<target>"` 打通缓存，改动时两文件必须同步。
- **更新器**:`tauri-plugin-updater`，endpoint = `https://raw.githubusercontent.com/tsingliuwin/daw/main/updates/latest.json`（由 release.yml 的 update-manifest job 生成并推回 main）。签名私钥 `~/.tauri/daw.key`（密码在 `~/.tauri/daw.key.password`）不进仓库，CI 从 GitHub secrets `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 读取（分别为两个文件的内容）。
- **前端更新状态机**统一在 `src/lib/updater.ts`（TitleBar 菜单 + BrandFooter badge/弹窗共用），不要在组件里各自实现。
- 发版步骤、密钥轮换、失败回滚见 `docs/RELEASE.md`；版本号四文件联动（package.json / tauri.conf.json / Cargo.toml / Cargo.lock）。