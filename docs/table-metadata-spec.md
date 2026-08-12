# 表元数据参数规范

## 设计目标

统一描述每张注册的远程表的访问方式，工具内部根据元数据参数自动选择处理逻辑（catalog 路径或 postgres_query 下推），不依赖 preamble 引导。

## 参数层级

### 数据源级别（DataSourceConfig + db_connections 表）

| 参数 | 值 | 说明 |
|------|-----|------|
| db_type | "postgres" \| "mysql" \| "sqlite" | DuckDB 驱动类型 |
| db_product | "postgresql" \| "hologres" \| "oceanbase" \| "unknown" | 数据库产品 |
| db_mode | "standard" \| "external" \| "unknown" | 库类型（Hologres 外部库 vs 标准库） |

### 表级别（table_registry 表）

| 参数 | 值 | 说明 |
|------|-----|------|
| table_type | "native" \| "foreign" | 内表 vs 外表 |
| access_mode | "catalog" \| "pushdown" | 访问模式 |

## 参数组合与处理逻辑

| db_type | db_product | db_mode | table_type | access_mode | 处理逻辑 |
|---------|-----------|---------|------------|-------------|---------|
| postgres | postgresql | standard | native | catalog | SELECT FROM db_xxx.schema.table，DuckDB 自动下推 |
| postgres | postgresql | standard | foreign | catalog | 同上 |
| postgres | hologres | standard | native | catalog | 同上（标准库系统目录兼容） |
| postgres | hologres | standard | foreign | catalog | 同上（标准库系统目录兼容） |
| postgres | hologres | external | foreign | pushdown | 必须 postgres_query 下推（外部库系统目录不兼容） |
| mysql | * | * | * | catalog | DuckDB 自动下推 |
| sqlite | * | * | * | catalog | DuckDB 自动下推 |

## access_mode 自动检测逻辑

```
register_table 时：
1. 从 db_connections 查 db_product, db_mode
2. 如果 db_mode == "external" → access_mode = "pushdown"
3. 如果 db_mode == "standard" → 尝试 catalog 路径：
   a. CREATE VIEW v_xxx AS SELECT * FROM db_xxx.schema.table
   b. 成功 → access_mode = "catalog"
   c. 失败 → access_mode = "pushdown"
4. 用 postgres_query 查 pg_class.relkind → table_type = "native" | "foreign"
5. 全部写入 table_registry 表
```

## table_registry 表结构（SQLite）

```sql
CREATE TABLE IF NOT EXISTS table_registry (
    id TEXT PRIMARY KEY,
    workspace_path TEXT NOT NULL,
    connection_name TEXT NOT NULL,
    local_name TEXT NOT NULL,
    remote_schema TEXT NOT NULL,
    remote_table TEXT NOT NULL,
    db_type TEXT NOT NULL,
    db_product TEXT NOT NULL,
    db_mode TEXT NOT NULL,
    table_type TEXT NOT NULL,
    access_mode TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'available',
    unavailable_reason TEXT,
    last_explored INTEGER,
    FOREIGN KEY(workspace_path) REFERENCES workspaces(path) ON DELETE CASCADE
);
```

## 工具处理逻辑

### register_table(connection_name, table_name, local_name)
- 从 db_connections 查 db_product/db_mode
- 检测 relkind → table_type
- 检测 access_mode
- catalog → CREATE VIEW（DuckDB 自动下推）
- pushdown → 不创建视图，只记录 remote_ref
- 写入 table_registry

### execute_query(sql)
- 解析 SQL 提取 FROM 后的表名
- 从 table_registry 查 access_mode
- catalog → 直接执行
- pushdown → 报错"此表需要用 postgres_query 下推"

### sample_data(table_name)
- 从 table_registry 查 access_mode + connection + remote_schema + remote_table
- catalog → SELECT * FROM v_xxx LIMIT 5
- pushdown → SELECT * FROM postgres_query('db_xxx', 'SELECT * FROM "schema"."table" LIMIT 5')

### describe_table(table_name)
- 从 table_registry 查 access_mode + connection + remote_schema + remote_table
- catalog → SELECT * FROM v_xxx LIMIT 0
- pushdown → SELECT * FROM postgres_query('db_xxx', 'SELECT * FROM "schema"."table" LIMIT 0')

### list_tables()
- 从 table_registry 查所有已注册的表
- 返回 local_name + status + access_mode
- 不查 DuckDB 系统表

## OKF 与 table_registry 的分工

- table_registry：技术元数据（access_mode, table_type, status, connection）
- OKF：业务知识（字段释义, 关联关系, 排障记录, 探索备注）
