/**
 * 认证路由：POST /auth/login、POST /auth/setup、GET /auth/setup-status
 *
 * 用户名密码 -> 验证 -> 签发 JWT（24h 有效）。
 * 首次运行通过 setup token 完成魔法链接认证，并创建 admin 用户。
 */

import { Hono } from "hono";
import * as jose from "jose";
import { and, eq } from "drizzle-orm";
import { config } from "../config.js";
import { db, isFirstRun } from "../db.js";
import { enterprise, users, llmProviders } from "../schema.js";

const auth = new Hono();

/** 取首期单企业记录（PG 首期只有一个企业）。 */
async function getEnterprise() {
  const rows = await db.select().from(enterprise).limit(1);
  return rows[0] ?? null;
}

auth.post("/login", async (c) => {
  const body = await c.req.json<{ username: string; password: string }>();

  if (!body.username || !body.password) {
    return c.json({ error: "用户名和密码不能为空" }, 400);
  }

  const ent = await getEnterprise();
  if (!ent) {
    return c.json({ error: "企业未初始化" }, 500);
  }

  const rows = await db
    .select()
    .from(users)
    .where(and(eq(users.enterpriseId, ent.id), eq(users.username, body.username)));
  const user = rows[0];

  // 首期明文比较（后续改 bcrypt hash）。
  if (!user || user.passwordHash !== body.password) {
    return c.json({ error: "用户名或密码错误" }, 401);
  }

  // 签发 JWT（24h 有效）。
  const token = await new jose.SignJWT({ username: user.username })
    .setProtectedHeader({ alg: "HS256" })
    .setIssuedAt()
    .setExpirationTime("24h")
    .sign(new TextEncoder().encode(config.jwtSecret));

  return c.json({
    token,
    user: { username: user.username },
    serverName: ent.name,
  });
});

/**
 * 首次认证（魔法链接）--管理员粘贴签名 URL 后，客户端调此端点。
 * 验证 setup token -> 创建 admin 用户 -> 签发 JWT（username=admin）-> 清空 setup token（一次性）。
 * 返回 needsConfig: true 表示企业需要配置 LLM/搜索。
 */
auth.post("/setup", async (c) => {
  const body = await c.req.json<{ token: string }>();

  if (!body.token) {
    return c.json({ error: "缺少 setup token" }, 400);
  }

  if (!config.setupToken || body.token !== config.setupToken) {
    return c.json({ error: "setup token 无效或已过期" }, 401);
  }

  const ent = await getEnterprise();
  if (!ent) {
    return c.json({ error: "企业未初始化" }, 500);
  }

  // 创建 admin 用户（若不存在）。首期默认密码 admin，管理员后续可修改。
  const existing = await db
    .select()
    .from(users)
    .where(and(eq(users.enterpriseId, ent.id), eq(users.username, "admin")));
  if (existing.length === 0) {
    await db.insert(users).values({
      enterpriseId: ent.id,
      username: "admin",
      passwordHash: "admin",
    });
  }

  // 签发 JWT（username=admin，24h 有效）。
  const jwt = await new jose.SignJWT({ username: "admin" })
    .setProtectedHeader({ alg: "HS256" })
    .setIssuedAt()
    .setExpirationTime("24h")
    .sign(new TextEncoder().encode(config.jwtSecret));

  // 一次性：用完即清空内存 token。
  config.setupToken = null;

  // 检查是否已配置 LLM。
  const providers = await db
    .select()
    .from(llmProviders)
    .where(eq(llmProviders.enterpriseId, ent.id));

  return c.json({
    token: jwt,
    user: { username: "admin" },
    serverName: ent.name,
    enterpriseId: ent.id,
    needsConfig: providers.length === 0,
  });
});

/**
 * 返回是否需要 setup token（客户端判断是否首次运行）。
 */
auth.get("/setup-status", async (c) => {
  const ent = await getEnterprise();
  const firstRun = ent ? await isFirstRun(ent.id) : false;
  return c.json({
    needsSetup: !!config.setupToken,
    isFirstRun: firstRun,
  });
});

export default auth;
