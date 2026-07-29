/**
 * 企业管理路由：GET /enterprise/status + POST /enterprise/setup
 *
 * 管理员首次认证后，用这些端点完成企业基本信息配置
 * （企业名称 + LLM provider + 搜索服务）。全部读写 PostgreSQL。
 */

import { Hono } from "hono";
import { eq } from "drizzle-orm";
import { jwtAuth } from "../middleware/jwt.js";
import type { LlmProviderConfig } from "../config.js";
import { db, hasProviders, hasSearch } from "../db.js";
import { enterprise, llmProviders, llmModels, searchConfig } from "../schema.js";

const enterpriseRoute = new Hono();

/** 取首期单企业记录。 */
async function getEnterprise() {
  const rows = await db.select().from(enterprise).limit(1);
  return rows[0] ?? null;
}

/**
 * GET /enterprise/status - 返回企业是否已配置。
 */
enterpriseRoute.get("/status", jwtAuth, async (c) => {
  const ent = await getEnterprise();
  if (!ent) {
    return c.json({ error: "企业未初始化" }, 500);
  }
  const providers = await hasProviders(ent.id);
  const search = await hasSearch(ent.id);
  return c.json({
    configured: providers,
    serverName: ent.name,
    hasProviders: providers,
    hasSearch: search,
  });
});

/**
 * POST /enterprise/setup - 首次配置企业信息。
 * 接收 { serverName?, providers?, searchEngine?, searchApiKey? }，
 * 按字段写入 PG：企业名 UPDATE、providers+models 先删后插、search_config UPSERT。
 */
enterpriseRoute.post("/setup", jwtAuth, async (c) => {
  const body = await c.req.json<{
    serverName?: string;
    providers?: LlmProviderConfig[];
    searchEngine?: string;
    searchApiKey?: string;
  }>();

  const ent = await getEnterprise();
  if (!ent) {
    return c.json({ error: "企业未初始化" }, 500);
  }

  // 首次完整配置（providers 存在时）才要求 serverName + providers 都有值；
  // 部分更新（如只改 serverName 或只改 search）允许只传对应字段。
  if (body.providers !== undefined && (!body.serverName || body.providers.length === 0)) {
    return c.json({ error: "首次配置需要同时提供企业名称和至少一个 LLM provider" }, 400);
  }

  // 1. 更新企业名称。
  if (body.serverName !== undefined) {
    await db
      .update(enterprise)
      .set({ name: body.serverName })
      .where(eq(enterprise.id, ent.id));
  }

  // 2. 替换 providers + models（先删后插，llm_models 通过 ON DELETE CASCADE 跟随删除）。
  if (body.providers !== undefined) {
    await db.delete(llmProviders).where(eq(llmProviders.enterpriseId, ent.id));
    for (const p of body.providers) {
      await db.insert(llmProviders).values({
        id: p.id,
        enterpriseId: ent.id,
        name: p.name,
        endpoint: p.endpoint,
        apiKey: p.apiKey,
        apiFormat: p.apiFormat,
        enabled: p.enabled,
      });
      if (p.models.length > 0) {
        await db.insert(llmModels).values(
          p.models.map((m) => ({
            providerId: p.id,
            modelId: m.id,
            contextWindow: m.contextWindow,
            maxTokens: m.maxTokens ?? null,
          })),
        );
      }
    }
  }

  // 3. UPSERT 搜索配置（engine/apiKey 任一存在时处理，缺省字段沿用旧值）。
  if (body.searchEngine !== undefined || body.searchApiKey !== undefined) {
    const existing = await db
      .select()
      .from(searchConfig)
      .where(eq(searchConfig.enterpriseId, ent.id));
    const old = existing[0];
    const engine = body.searchEngine ?? old?.engine ?? "";
    const apiKey = body.searchApiKey ?? old?.apiKey ?? "";
    if (old) {
      await db
        .update(searchConfig)
        .set({ engine, apiKey })
        .where(eq(searchConfig.enterpriseId, ent.id));
    } else {
      await db.insert(searchConfig).values({
        enterpriseId: ent.id,
        engine,
        apiKey,
      });
    }
  }

  const updated = await getEnterprise();
  return c.json({ ok: true, serverName: updated?.name ?? ent.name });
});

export default enterpriseRoute;
