# AGENTS.md — 项目开发经验记录

## 持续进化体系：用户画像 · 任务协议 · 知识生命周期（2026-08-27 落地，借鉴 deepseek-harness）

dsh 的「越用越懂」由三条闭环组成——技能/决策沉淀、会话内记忆（8 段大纲 compaction）、纪律句。本项目对应落三层（premble 引导 + OKF 类别 + 渲染标记）：

- **用户画像（users 类）**：目录/渲染/工具早就有，但 preamble 没引导就永远零沉淀。经验：**新机制要生效，必须有一条把「触发时机」写进提示词的路径**。触发词=「用户表达了任何偏好/习惯/纠错」（"我要的是…""这里不对"），立即写 users/default（五板块：角色与关注点/沟通与交付偏好/常用口径与基准/已拍板决策/纠错记录，纠错原话逐字保留）；合并更新单一权威版本。
- **任务协议（playbooks 类）**：把「偶发的高质量交付」固化为「适用问题+数据源+交付结构+验证清单」，大纲注入协议目录、命中即加载执行——任务型高价值场景不再临场发挥。首条协议：两期同比对比表。
- **知识生命周期（status）**：update_okf_metadata 支持 status=active|superseded；大纲渲染对作废条目标「⚠️已作废」。与「修正优先于并存」纪律配套成完整链条：同文件改写段落 / 整文件顶替标记作废。
- **复盘脚本的「用户纠正候选」检测**：句式匹配逐字列出用户纠正原话，提示沉淀为 users/concepts——好反馈别滞留在 jsonl 里。
- 验收模板（源自 dsh mcp-memory 三步验证法）：会话 A 沉淀偏好 → 新会话 B 验证召回 → 新会话 B 按偏好直接交付。跨会话「越用越懂」只有通过这条链才算数。

## 用-查-改-用复盘循环（2026-08-27 首轮确立）

真实使用 → `tools/review_transcript.py` 复盘 → 修复（配测试）→ 再用验证。首轮复盘（郑州 8 月归因一单）修出的问题，全部有复盘实证：

- **守卫的解析器必须吃 LLM 真实输出形状**：pushdown 守卫的表名提取用字面 `" FROM"`（空格）匹配，而模型写的是多行 SQL（FROM 前是换行符），守卫全数漏检——一单 15 次 24–45s 全表拉取、2 次 pg_namespace 报错全部绕过。同文件里第二个提取器（错误路径状态更新）同样从未生效。教训：**凡对 LLM 输出做解析的守卫，测试用例必须来自真实翻车的 SQL 原文**（多行/缩进/CTE/中文过滤值），不能只测单行理想形状。
- **reasoning 双写**：rig 流同时给 `ReasoningDelta`（增量）与 `Reasoning`（完整），都当增量发则思考段逐字翻倍。完整事件必须对增量缓冲做前缀去重只补余量。
- **禁词守卫用词边界，不用子串**：`contains("DELETE")` 会误伤 `is_deleted` 列名，把模型逼向更差的 SQL；先抹字符串字面量再词边界匹配，引号未闭合时退回保守子串（宁误拦不漏放）。
- **引擎级不确定性的实锤与根修（P0-B，2026-08-27 当天定案）**：同一时间窗、同一视图，会话中途两次查询返回不同数字（310/793.8 万 vs 334/861.1 万，jsonl payload 实证，非模型抄错）。复现实验（`src-tauri/src/bin/test_consistency.rs`）证明：根因是 **timestamptz 朴素字面量的解释权在求值方的会话时区**——DuckDB 会话时区随 ICU 扩展可用性/加载时机在 UTC 与系统 locale(+08) 间变化（SQL 出现 `AT TIME ZONE '…'` 即触发自动加载翻转），postgres_query 内层字面量则始终由 Hologres(+08) 解释；两条路径、两个时点 = 四种窗。`TZ=UTC` 下复现出与异常一字不差的 310/7,938,306。修复 = `create_workspace_conn` 显式 `SET TimeZone='Asia/Shanghai'`，配套约定改为**直接写北京墙钟字面量**（旧 -8h 纪律作废，OKF 知识已同步改写并留勘误）；修复后两路径数字验证一致（334/334、343/343），ICU 触发后不回退。SQL 审计 meta 的 firstRow 指纹即为定位此类问题的对账手段。
- **知识沉淀要「修正优先于并存」**：复盘发现知识库里时区纪律、业绩口径的新旧版本并存，下次会话模型被迫当仲裁（「以最新为准」），既耗推理又易选错。已写入 preamble 第五步。
- **大纲徽标必须自带行为指令**：`[pushdown]` 被模型读反成「视图已下推、直接查」，改 `[下推表·禁直查]`。徽标/标记是给模型看的 UI，语义自解释比术语精确更重要。
- 排查手法：jsonl 工具段的 `payload` 字段存结构化 SqlResult（columns/rows），复盘对账以它为准；`logs` 表（category=sql）只有摘要，别指望它回放结果。

## 提示词工程纪律（借鉴 deepseek-harness，2026-08-27 落地）

**静态/动态分离，system prompt 必须字节稳定**：

- system prompt（`usage.rs` 两个 preamble）只放**静态纪律**：品牌名注入后跨轮/跨会话字节不变，provider 前缀缓存才能持续命中。**禁止把每轮变化的事实（时间、OKF 大纲）拼进 preamble**——此前每写一条知识，下一轮整个 system prompt 缓存前缀就被击穿（数据分析核心循环恰恰鼓励每轮写知识）。
- 每轮变化的事实由 runner 以 `<runtime_context>` 快照 prepend 到本次用户消息（`llm_prompt`）：不进 system prompt、不写持久化历史（前端只存用户原文），下一轮自动被新快照取代。preamble 里只描述该块语义（「# 运行时上下文」段），不承载内容。

**每个事实只有一个 owner**：

- 同一规则只在一处权威定义、其余一行引用：`postgres_query` 下推规则的 owner 是 preamble 第三步（sample_data 提示与 execute_query 描述只一句话引用）；错误处置的 owner 是 preamble「错误处置」决策树（register_table 运行时文案同口径复述，运行时文案允许重复——它带上下文且便宜）。
- 工具参数里的 OKF 类别枚举必须从 `Category::prompt_list()`（`okf/model.rs`）派生，不要手写字符串——手写漏过 selections/users，导致 preamble 要求写选表经验、schema 里却无该类别可选。

**fail-loud 防漂移测试**（`skill/data_analysis/mod.rs` / `usage.rs` / `okf/model.rs`）：

- preamble 里反引号 snake_case 标识符必须是真实工具 `NAME` 常量或在 allowlist（防臆造工具名——历史上模型臆造工具名会被火山端点整请求拒绝）。新增/改名工具、或在 preamble 新写反引号标识符时测试失败，强制显式归类。
- 品牌替换后不允许残留 `{app_name}`；`prompt_list` 必须覆盖全部类别 `dir()`。

**具体教训**：

- 权限/不存在/存储层级错误对同表是**终局**：不重试、不换写法绕行、不静默跳过。三处指引（preamble 第一步「跳过换表」/ 纪律「停下等用户」/ register_table 文案「建议跳过」）曾互相打架，已统一为决策树：分析不依赖该表 → 继续其余 + 结论注明；依赖 → 列选项停下等用户。
- preamble 引用大纲段落名必须与 `okf/catalog.rs` 实际渲染标题一致（曾引用不存在的 `# 工作区数据记忆`）；改 catalog 渲染标题时同步 preamble。
- 工具描述与实现对齐：execute_query 禁词含 TRUNCATE/ATTACH/DETACH、50 行截断；render_chart 200 行截断——描述写漏会让模型带着错误预期干活。
- preamble 勿写死工具数量（「你有两个工具」）——工具集变化会静默失真；时间真源统一为 runtime_context 注入（`get_current_time` 仅用于时分秒精度核实）。
- **提示词里的示例必须通过真实 schema 校验**（借鉴 dsh issue #3204，2026-08-31）：dsh 的 PTC SDK 声明含无条件渲染的 bash 示例，模型绕过 `run_code` 直接调了未注册的 bash。修复两件事：显式声明「声明≠可直接调用」；示例只在真实 schema 能逐字接受示例参数（required/const/enum 全核对）时才渲染。daw 的 fail-loud 标识符测试只保工具名存在；**凡在 preamble/工具描述里写调用示例，参数形状必须与 schema 逐字对得上**，对不上就不写示例。

## 借鉴 deepseek-harness：turn 过程折叠 + turn 统计（2026-08-31 落地）

dsh web 的两个聊天 UI 机制移植进 ChatView（对照基线 dsh-v0.1.2-alpha.2）：

- **历史轮次过程折叠**：完成的 assistant 消息把结论前的 reasoning/tool/中间文本折成一行「已思考 X 秒 · N 次工具调用」（`deriveTurnProcess` 纯函数判定，折叠段**不渲染**而非 hidden——长对话 DOM 随历史线性下降，这是 daw 渲染债的延伸防线）；图表/错误段始终可见，无结论的轮次（错误收尾）绝不折叠，正在流式的消息绝不折叠。
- **turn 统计 pill**：后端 `emit_usage_run_summary`（turnComplete 事件，先于 done 到达）的 `runOutputTokens/runElapsedMs/tokPerSec` 挂到本次 run 的 assistant 消息（`ChatMessage.turnUsage`，随消息持久化），done 后在消息操作行显示「用量 X tok / 用时 X 秒」；缺事实省略不渲染占位。最新一条消息的操作行 CSS 常显（`:last-child`），历史消息维持 hover 显示。

## 布局尺寸纪律：壳层只用纯 CSS 百分比（三轮教训）

根容器/壳层尺寸**禁止内联像素校准**（b404fe9 引入、903d6a8 加固、4b1fedd 彻底移除）：

- **根因**：流式输出高峰期主线程被渲染任务洪水占满（实测 Runtime.evaluate 排队 12s+ 无响应），任何依赖 JS 事件（Tauri `onResized`、`window.resize`）+ IPC（`innerSize()`）的尺寸校准在该场景**整体失效**，内联像素尺寸闩在旧值 → 最大化后右侧大片空白、还原后内容被裁。
- **结论**：CSS `100%` 由渲染管线驱动，不占 JS 任务队列，洪水期截图验证最大化/还原均正确自愈；还天然避开界面缩放（`documentElement.style.zoom`）≠100% 时内联像素被 zoom 倍率放大的隐患。
- **排障手法**：WebView2 远程调试口（`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222` + 独立 `WEBVIEW2_USER_DATA_FOLDER`）+ 裸 CDP（Runtime.evaluate/Page.captureScreenshot）；合成器截图在主线程饿死时仍可用，是洪水期唯一可靠的观测手段。
- 同类风险：任何「窗口状态 → JS → 内联样式」的链路在流式期都不可依赖；需要样式响应窗口尺寸时优先 CSS（含 container query）。
- **节流不够，还要降单元成本**：delta 节流（150ms 冲刷）只降频率；真实重任务里每次冲刷仍对整段累积文本全量 marked 解析 + Shiki 高亮 + innerHTML 重建，渲染管线照样饱和（实测求值排队 14.5s、合成器帧停滞、WebView2 表面卡死且整页重载不恢复）。根治靠 ChatView 的 `isLiveTextTail`：流式尾部 text 段纯文本直出，流结束才 markdown 终渲。
- **`<For>` 按引用 diff 是隐形全量重建点**：MessageText 的 chunks memo 每次 recompute 新建 text chunk 对象 → For 每次销毁重建全部子树。流式期 `allCharts` 每次冲刷都产生新数组身份连带 memo 重跑，带图表引用的消息每 150ms 全量重走 marked+Shiki（订单类长对话 30 图表 → 渲染债滚雪球，流停后 143% CPU 再烧十分钟）。chunk 对象必须跨 recompute 缓存复用（按「序号+内容」键），图表块按 ref 缓存同理。任何喂给 `<For>` 的派生数组都要警惕这一点。

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