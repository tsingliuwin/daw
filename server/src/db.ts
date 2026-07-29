/**
 * PG 连接池 + Drizzle 实例 + 迁移 + 初始化。
 *
 * 启动时：连 PG -> 执行迁移 -> 初始化默认企业 -> 返回 db 实例。
 * 所有路由通过 `db` 查 PG，不再读 JSON 文件。
 */

import { drizzle } from "drizzle-orm/node-postgres";
import { migrate } from "drizzle-orm/node-postgres/migrator";
import { eq } from "drizzle-orm";
import { Pool } from "pg";
import * as schema from "./schema.js";

const pool = new Pool({
  connectionString: process.env.DATABASE_URL,
});

export const db = drizzle(pool, { schema });

/**
 * 启动时执行迁移 + 初始化默认企业。
 * 返回企业 ID（供 config 和路由使用）。
 */
export async function initDatabase(): Promise<string> {
  // 1. 执行迁移（自动建表）。
  await migrate(db, { migrationsFolder: "./drizzle" });
  console.log("  数据库迁移完成");

  // 2. 检查是否有企业记录，没有则插入默认行。
  const existing = await db.select().from(schema.enterprise).limit(1);
  if (existing.length === 0) {
    const [row] = await db
      .insert(schema.enterprise)
      .values({ name: "AIOA 工作台" })
      .returning();
    console.log(`  已创建默认企业: ${row.id}`);
    return row.id;
  }

  // 3. 检查是否有 admin 用户，没有则标记为首次运行（需要 setup token）。
  return existing[0].id;
}

/**
 * 检查是否首次运行（无用户）。
 */
export async function isFirstRun(enterpriseId: string): Promise<boolean> {
  const result = await db
    .select()
    .from(schema.users)
    .where(eq(schema.users.enterpriseId, enterpriseId));
  return result.length === 0;
}

/**
 * 检查企业是否已配置 LLM（有 provider）。
 */
export async function hasProviders(enterpriseId: string): Promise<boolean> {
  const result = await db
    .select()
    .from(schema.llmProviders)
    .where(eq(schema.llmProviders.enterpriseId, enterpriseId));
  return result.length > 0;
}

/**
 * 检查企业是否已配置搜索。
 */
export async function hasSearch(enterpriseId: string): Promise<boolean> {
  const result = await db
    .select()
    .from(schema.searchConfig)
    .where(eq(schema.searchConfig.enterpriseId, enterpriseId));
  return result.length > 0 && !!result[0].engine && !!result[0].apiKey;
}

/** 关闭连接池（优雅退出时调用）。 */
export async function closePool() {
  await pool.end();
}
