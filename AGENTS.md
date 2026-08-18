# AGENTS.md — 项目开发经验记录

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