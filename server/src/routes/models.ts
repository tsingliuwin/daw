/**
 * 模型列表路由：GET /models
 *
 * JWT 保护。返回服务端配置的所有已启用 provider 及其模型。
 * 客户端在企业模式下用这个列表填充模型选择器（不含 apiKey）。
 */

import { Hono } from "hono";
import { config } from "../config.js";
import { jwtAuth } from "../middleware/jwt.js";

const models = new Hono();

models.get("/", jwtAuth, (c) => {
  const providers = config.providers
    .filter((p) => p.enabled)
    .map((p) => ({
      id: p.id,
      name: p.name,
      apiFormat: p.apiFormat,
      models: p.models.map((m) => ({
        id: m.id,
        contextWindow: m.contextWindow,
        maxTokens: m.maxTokens,
      })),
    }));

  return c.json({ providers });
});

export default models;
