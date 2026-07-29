/**
 * 对话代理路由：POST /chat/stream
 *
 * JWT 保护。接收 { model, messages, stream } 转发到对应 LLM 服务商，
 * 以 SSE 流式返回。首期为骨架--实际 SSE 转发逻辑后续实现。
 * provider/model 从 PG 查询（按 modelId 联查 llm_providers）。
 */

import { Hono } from "hono";
import { and, eq } from "drizzle-orm";
import { jwtAuth } from "../middleware/jwt.js";
import type { JwtPayload } from "../middleware/jwt.js";
import { db } from "../db.js";
import { enterprise, llmProviders, llmModels } from "../schema.js";

const chat = new Hono<{
  Variables: { user: JwtPayload };
}>();

chat.post("/stream", jwtAuth, async (c) => {
  const body = await c.req.json<{
    model: string;
    messages: { role: string; content: string }[];
    stream?: boolean;
  }>();

  // 取首期单企业。
  const entRows = await db.select().from(enterprise).limit(1);
  const ent = entRows[0];
  if (!ent) {
    return c.json({ error: `模型 ${body.model} 不可用` }, 400);
  }

  // 按 modelId 联查 provider + model（要求 provider 已启用且属于本企业）。
  const rows = await db
    .select({
      providerName: llmProviders.name,
      modelId: llmModels.modelId,
    })
    .from(llmModels)
    .innerJoin(llmProviders, eq(llmProviders.id, llmModels.providerId))
    .where(
      and(
        eq(llmModels.modelId, body.model),
        eq(llmProviders.enterpriseId, ent.id),
        eq(llmProviders.enabled, true),
      ),
    );
  const row = rows[0];

  if (!row) {
    return c.json({ error: `模型 ${body.model} 不可用` }, 400);
  }

  // TODO: 实际 SSE 流式转发到 LLM 服务商。
  // 首期返回骨架响应，后续实现完整的流式代理。
  return c.json({
    message: "chat/stream endpoint - SSE 流式代理待实现",
    model: body.model,
    provider: row.providerName,
  });
});

export default chat;
