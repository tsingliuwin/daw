/**
 * 对话代理路由：POST /chat/stream
 *
 * JWT 保护。接收 { model, messages, stream } 转发到对应 LLM 服务商，
 * 以 SSE 流式返回。首期为骨架——实际 SSE 转发逻辑后续实现。
 */

import { Hono } from "hono";
import { config } from "../config.js";
import { jwtAuth } from "../middleware/jwt.js";
import type { JwtPayload } from "../middleware/jwt.js";

const chat = new Hono<{
  Variables: { user: JwtPayload };
}>();

chat.post("/stream", jwtAuth, async (c) => {
  const body = await c.req.json<{
    model: string;
    messages: { role: string; content: string }[];
    stream?: boolean;
  }>();

  // 找到对应的 provider。
  let provider = null;
  let modelConfig = null;
  for (const p of config.providers) {
    if (!p.enabled) continue;
    const m = p.models.find((m) => m.id === body.model);
    if (m) {
      provider = p;
      modelConfig = m;
      break;
    }
  }

  if (!provider || !modelConfig) {
    return c.json({ error: `模型 ${body.model} 不可用` }, 400);
  }

  // TODO: 实际 SSE 流式转发到 LLM 服务商。
  // 首期返回骨架响应，后续实现完整的流式代理。
  return c.json({
    message: "chat/stream endpoint — SSE 流式代理待实现",
    model: body.model,
    provider: provider.name,
  });
});

export default chat;
