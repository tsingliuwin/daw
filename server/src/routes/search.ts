/**
 * 搜索代理路由：POST /search
 *
 * JWT 保护。接收 { query, num_results } 转发到 Exa/Brave，返回搜索结果。
 * 客户端企业模式下搜索走这里（不用本地 key）。engine + apiKey 从 PG 读取。
 */

import { Hono } from "hono";
import { eq } from "drizzle-orm";
import { jwtAuth } from "../middleware/jwt.js";
import { db } from "../db.js";
import { enterprise, searchConfig } from "../schema.js";

const search = new Hono();

search.post("/", jwtAuth, async (c) => {
  const body = await c.req.json<{ query: string; num_results?: number }>();

  if (!body.query) {
    return c.json({ error: "query 不能为空" }, 400);
  }

  // 取首期单企业 + 搜索配置。
  const entRows = await db.select().from(enterprise).limit(1);
  const ent = entRows[0];
  if (!ent) {
    return c.json({ error: "服务端未配置搜索服务" }, 500);
  }

  const cfgRows = await db
    .select()
    .from(searchConfig)
    .where(eq(searchConfig.enterpriseId, ent.id));
  const cfg = cfgRows[0];

  if (!cfg || !cfg.apiKey) {
    return c.json({ error: "服务端未配置搜索服务" }, 500);
  }

  const num = Math.min(10, Math.max(1, body.num_results ?? 5));

  // Exa 搜索。
  if (cfg.engine === "exa") {
    try {
      const resp = await fetch("https://api.exa.ai/search", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${cfg.apiKey}`,
        },
        body: JSON.stringify({
          query: body.query,
          type: "auto",
          numResults: num,
          contents: { highlights: true },
        }),
      });

      if (!resp.ok) {
        const text = await resp.text();
        return c.json({ error: `Exa 搜索失败 (${resp.status}): ${text}` }, 502);
      }

      const data = await resp.json() as {
        results?: { title?: string; url?: string; highlights?: string[]; text?: string }[];
      };

      const results = (data.results ?? []).map((item) => ({
        title: item.title ?? "",
        url: item.url ?? "",
        snippet: item.highlights?.join("\n") ?? item.text ?? "",
      }));

      return c.json({ engine: "exa", query: body.query, count: results.length, results });
    } catch (e) {
      return c.json({ error: `Exa 搜索请求失败: ${e}` }, 502);
    }
  }

  // TODO: Brave 搜索。

  return c.json({ error: `不支持的搜索引擎: ${cfg.engine}` }, 400);
});

export default search;
