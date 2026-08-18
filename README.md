# Daw（寒鸦）— Data Agent Workstation

<blockquote>
<b>Daw</b> is an open-source **Data Agent Workstation**: a desktop workbench where you talk to an AI agent that searches the web, queries your databases, renders charts, and builds up a knowledge base of what it learns — for tasks and data alike. "Daw" stands for <i>Data Agent Workstation</i>, and is also the word for the jackdaw (寒鸦), the bird picturing its logo. Ship it as-is, or make it your own with a single <code>brand.json</code> — your name, your logo, your wording.
</blockquote>

**Daw** 是一个开源的「数据智能体工作台」桌面应用：你跟一个 AI 助手对话，它就能帮你搜索互联网、查询你的数据库、生成图表，并把每次学会的业务知识沉淀进工作区知识库。项目名 **Daw** 取自 *Data Agent Workstation* 的首字母，同时也是寒鸦（jackdaw）的意思——**中文名「寒鸦数据工作台」**，logo 就是一只寒鸦。界面上默认显示中文名「寒鸦数据工作台」；英文名 Daw 用于安装包、进程与代码标识。

Daw 不绑定任何一家公司：改一份 `brand.json` 配置文件，就能把它变成**你自己专属的工作台**（名字、logo、欢迎语、助手身份都归你）。

## 特性

- **对话式任务助手**：联网搜索、知识问答、文案撰写，主动搜索而不凭记忆编造。
- **对话式数据分析**：接入 PostgreSQL / Hologres / MySQL 等数据源，Agent 自动发现表、注册视图、写 SQL、生成图表（趋势/对比/占比），并沉淀字段释义与排障经验。
- **工作区知识库（OKF）**：每个工作区一份 git 版本化的知识库（表释义、业务概念、排障记录），新会话自动继承，无需重复探索。
- **多工作区、多任务**：工作区即文件夹，任务级隔离，图表内联渲染，全流式输出。
- **多 LLM 支持**：OpenAI / Anthropic / 任意 OpenAI 兼容服务商，模型级上下文窗口配置。
- **可深度定制**：`brand.json` 换名字/logo/文案/助手身份；三套主题自选。
- **本地优先**：数据全在你的机器上（`~/.daw/`），无云端依赖。

## 快速开始

环境要求：Rust（stable）、Node.js ≥ 18、`npm`。

```bash
npm install          # 安装前端依赖
npm run tauri dev    # 开发模式启动
```

打包发行版：

```bash
npm run tauri build
```

首次启动后，数据目录 `~/.daw/`（Windows 为 `%USERPROFILE%\.daw`）会自动创建，并在「设置」里配置你的大模型服务商即可开聊。

> 数据分析功能依赖 [DuckLake](https://ducklake.ai) 扩展；首次进入「数据分析」场景时按提示一键安装。

## 自动更新与发版

应用内置自动更新：启动后静默检查新版本（每 4 小时一次），发现后后台下载，就绪时在左下角品牌区出现提示，确认后自动重启安装；安装包经 minisign 签名校验。更新说明来自本仓库 `CHANGELOG.md` 对应小节，由发版 CI（`.github/workflows/release.yml`）自动生成 `updates/latest.json` 供更新器读取。

- 下载安装包：[GitHub Releases](https://github.com/tsingliuwin/daw/releases)
- 发版流程与密钥管理见 **[docs/RELEASE.md](docs/RELEASE.md)**

## 品牌定制：brand.json

`~/.daw/brand.json` 是唯一的品牌配置文件。首次启动会自动生成一份带默认值的模板；改完**重启应用即生效**，无需重新编译。

```json
{
  "app_name": "我的工作台",
  "tagline": "数据与任务的对话入口",
  "about_description": "这是我们团队自己的专属工作台。",
  "logo_light": "logo.png",
  "logo_dark": "logo_white.png",
  "home": {
    "welcome_title": "我的工作台",
    "welcome_subtitle": "用对话驱动数据与任务",
    "task": {
      "label": "日常任务",
      "subtitle": "信息检索、知识问答、文案撰写——用对话完成任务。",
      "placeholder": "试试：「帮我调研一下行业最新动态」"
    },
    "data_analysis": {
      "label": "数据分析",
      "subtitle": "查询数据库、生成图表、沉淀业务知识。",
      "placeholder": "试试：「统计各区域今年销量并画个柱状图」"
    }
  }
}
```

字段说明：

| 字段 | 含义 |
| --- | --- |
| `app_name` | 应用名称：窗口标题、侧边栏、首页、关于弹窗、AI 助手自称 |
| `tagline` | 名称下方的短口号 |
| `about_description` | 「关于」弹窗里的一行产品描述 |
| `logo_light` / `logo_dark` | 自定义 logo 文件名（放在 `~/.daw/` 目录下）；留空则用内置寒鸦 logo（浅色主题用 `logo_light`，深色主题用 `logo_dark`；支持 png/jpg） |
| `home.welcome_title` / `welcome_subtitle` | 首页欢迎大标题与小字副标题 |
| `home.task` / `home.data_analysis` | 两个场景卡片的文案（`label` 按钮名、`subtitle` 说明、`placeholder` 输入框示例）。场景的 `id` 固定为 `task` / `data_analysis`，决定 Agent 的工具集；文案可随意改 |

JSON 里缺什么字段补什么 default，配错也不用慌——解析失败时回落到默认 Daw 品牌，不会影响启动。需要恢复默认时直接删除 `brand.json` 即可。

## 数据与隐私

- 一切数据存本地：`~/.daw/daw.db`（任务/工作区/配置/日志）、`~/.daw/settings.json`（LLM 服务商与数据源）、`~/.daw/<工作区>/`（聊天记录、DuckLake 表、OKF 知识库）。
- 数据库连接凭据只保存在本机 SQLite 中，随工作区 ATTACH 转发到本地 DuckDB 会话。
- 从旧版本的 `~/.aioa` 目录会在首次启动时自动迁移为 `~/.daw`，历史数据原样保留。

## 架构速览

```
┌─────────────────────────── Tauri 桌面应用 ───────────────────────────┐
│  前端 SolidJS + Vite（聊天 UI / 工作区 / 设置 / 图表渲染）             │
│        ▲ invoke / event（agent-event 流式协议）                       │
│  后端 Rust：命令层 ── Agent runner（rig）── Skills（搜索/数据分析工具） │
│        │                    │                                        │
│   SQLite（元数据）   DuckDB in-memory + DuckLake（查询/视图/知识库）    │
└──────────────────────────────────────────────────────────────────────┘
```

- **前端**：SolidJS + Vite，流式聊天界面、ECharts 图表、Shiki 代码高亮。
- **后端**：Tauri v2 + Rust，`rig` 做多服务商 LLM 编排，工具以 Skill 组织（联网搜索、数据库发现/注册/查询/画图、OKF 知识读写）。
- **数据**：SQLite 存元数据；DuckDB（DuckLake 扩展）做每个工作区的内存查询会话与持久化表；OKF 工作区知识库带 git 版本历史。
- **品牌层**：`brand.rs`（读取/兜底）→ `brand.ts`（前端 store）→ 各组件与系统提示词统一消费，一处配置全局生效。

## 参与贡献

欢迎提 issue、PR。约定：与现有代码保持一致（中文注释与提交习惯、SolidJS/`ln-*` CSS 命名、Rust 模块边界）。详见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## License

[AGPL-3.0](LICENSE) © Daw contributors。你的定制部署、二次分发均须遵守 AGPL-3.0 条款（含网络使用义务）。