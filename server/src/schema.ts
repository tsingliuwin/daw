/**
 * Drizzle schema 定义--PostgreSQL 表结构。
 *
 * 修改表结构后运行 `npx drizzle-kit generate` 生成迁移文件，
 * 服务端启动时自动执行 `npx drizzle-kit migrate`（或代码里 migrate()）。
 */

import {
  pgTable,
  uuid,
  serial,
  text,
  boolean,
  integer,
  timestamp,
  uniqueIndex,
} from "drizzle-orm/pg-core";

/** 企业元数据（首期一个 PG 一个企业）。 */
export const enterprise = pgTable("enterprise", {
  id: uuid("id").primaryKey().defaultRandom(),
  name: text("name").notNull().default("AIOA 工作台"),
  createdAt: timestamp("created_at", { withTimezone: true }).defaultNow().notNull(),
});

/** 用户（企业下的登录账号）。 */
export const users = pgTable(
  "users",
  {
    id: serial("id").primaryKey(),
    enterpriseId: uuid("enterprise_id").references(() => enterprise.id),
    username: text("username").notNull(),
    passwordHash: text("password_hash").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true }).defaultNow().notNull(),
  },
  (t) => ({
    uniqueUser: uniqueIndex("users_enterprise_username_idx").on(t.enterpriseId, t.username),
  }),
);

/** LLM Provider 配置。 */
export const llmProviders = pgTable("llm_providers", {
  id: text("id").primaryKey(),
  enterpriseId: uuid("enterprise_id").references(() => enterprise.id),
  name: text("name").notNull(),
  endpoint: text("endpoint").notNull(),
  apiKey: text("api_key").notNull(),
  apiFormat: text("api_format").notNull(),
  enabled: boolean("enabled").default(true).notNull(),
});

/** LLM 模型（属于某个 provider）。 */
export const llmModels = pgTable("llm_models", {
  id: serial("id").primaryKey(),
  providerId: text("provider_id").references(() => llmProviders.id, { onDelete: "cascade" }),
  modelId: text("model_id").notNull(),
  contextWindow: integer("context_window").default(256000).notNull(),
  maxTokens: integer("max_tokens").default(64000),
});

/** 搜索服务配置（每个企业一条）。 */
export const searchConfig = pgTable("search_config", {
  enterpriseId: uuid("enterprise_id")
    .primaryKey()
    .references(() => enterprise.id),
  engine: text("engine").notNull().default(""),
  apiKey: text("api_key").notNull().default(""),
});
