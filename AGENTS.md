# AGENTS.md — 项目开发经验记录

## DuckDB postgres_query 关键经验

**postgres_query 的第一个参数必须是 catalog 别名（如 `db_xxx`），不能是连接串（如 `host=... port=...`）。**

- **传连接串**：触发 postgres 扩展重新初始化 catalog（执行元数据扫描），在 Hologres 等兼容 PG 协议但系统目录表有差异的数据库上报 `missing FROM-clause entry for table "pg_namespace"` 错误。
- **传 catalog 别名**：复用已 ATTACH 的连接，不触发重新扫描。

正确用法：
```sql
-- ATTACH 后用别名调 postgres_query
SELECT * FROM postgres_query('db_yantubi', 'SELECT * FROM pg_catalog.pg_class ...')
```

错误用法：
```sql
-- 传连接串会触发元数据扫描
SELECT * FROM postgres_query('host=... port=... dbname=...', 'SELECT ...')
```

## Hologres 兼容性设计（三层隔离）

参考 lakemind 的设计，agent 从不直接查 ATTACH 的远程 catalog：

1. **list_tables**：只查本地 catalog（`database_name='memory'`），不枚举远程 `db_` catalog
2. **list_remote_tables**：postgres 类型用 `postgres_query` 下推查 `pg_catalog`（不触发 catalog 扫描）
3. **register_table**：检测 foreign table（`relkind='f'`），外表用 `postgres_query` 创建视图，普通表走 catalog 引用

外表在 `information_schema` 里可能不显示，需要用 `pg_catalog.pg_class` 查（`relkind IN ('r','v','m','f','p')`）。
