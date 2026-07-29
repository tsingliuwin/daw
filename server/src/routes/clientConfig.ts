/**
 * 客户端配置路由：GET /client-config
 *
 * 无需认证。返回服务名 + 可用性，让客户端验证服务地址正确且服务在线。
 * 也返回是否需要登录（hasUsers）。全部从 PG 实时查询。
 */

import { Hono } from "hono";
import { db, isFirstRun, hasProviders, hasSearch } from "../db.js";
import { enterprise } from "../schema.js";

const clientConfig = new Hono();

clientConfig.get("/", async (c) => {
  const entRows = await db.select().from(enterprise).limit(1);
  const ent = entRows[0];

  if (!ent) {
    return c.json({
      serverName: "",
      enterpriseId: "",
      hasUsers: false,
      hasProviders: false,
      hasSearch: false,
    });
  }

  const firstRun = await isFirstRun(ent.id);
  const providers = await hasProviders(ent.id);
  const search = await hasSearch(ent.id);

  return c.json({
    serverName: ent.name,
    enterpriseId: ent.id,
    hasUsers: !firstRun,
    hasProviders: providers,
    hasSearch: search,
  });
});

export default clientConfig;
