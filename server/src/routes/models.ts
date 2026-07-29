/**
 * 模型列表路由：GET /models
 *
 * JWT 保护。返回服务端配置的所有已启用 provider 及其模型（从 PG 查）。
 * 客户端在企业模式下用这个列表填充模型选择器（不含 apiKey）。
 */

import { Hono } from "hono";
import { and, eq } from "drizzle-orm";
import { jwtAuth } from "../middleware/jwt.js";
import { db } from "../db.js";
import { enterprise, llmProviders, llmModels } from "../schema.js";

const models = new Hono();

models.get("/", jwtAuth, async (c) => {
  // 取首期单企业。
  const entRows = await db.select().from(enterprise).limit(1);
  const ent = entRows[0];
  if (!ent) {
    return c.json({ providers: [] });
  }

  // 联查 provider + model（左连接，provider 无模型时 modelId 为 null）。
  // 注意：不 select apiKey（安全）。
  const rows = await db
    .select({
      providerId: llmProviders.id,
      providerName: llmProviders.name,
      apiFormat: llmProviders.apiFormat,
      modelId: llmModels.modelId,
      contextWindow: llmModels.contextWindow,
      maxTokens: llmModels.maxTokens,
    })
    .from(llmProviders)
    .leftJoin(llmModels, eq(llmModels.providerId, llmProviders.id))
    .where(
      and(
        eq(llmProviders.enterpriseId, ent.id),
        eq(llmProviders.enabled, true),
      ),
    );

  // 按 provider 分组。
  const providerMap = new Map<
    string,
    {
      id: string;
      name: string;
      apiFormat: string;
      models: { id: string; contextWindow: number; maxTokens: number | null }[];
    }
  >();
  for (const row of rows) {
    let p = providerMap.get(row.providerId);
    if (!p) {
      p = {
        id: row.providerId,
        name: row.providerName,
        apiFormat: row.apiFormat,
        models: [],
      };
      providerMap.set(row.providerId, p);
    }
    if (row.modelId) {
      p.models.push({
        id: row.modelId,
        // modelId 非空说明 join 命中，contextWindow 一定有值（schema 为 notNull）。
        contextWindow: row.contextWindow!,
        maxTokens: row.maxTokens,
      });
    }
  }

  return c.json({ providers: [...providerMap.values()] });
});

export default models;
