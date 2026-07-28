/**
 * 认证路由：POST /auth/login
 *
 * 用户名密码 → 验证 → 签发 JWT（24h 有效）。
 */

import { Hono } from "hono";
import * as jose from "jose";
import { config, deleteSetupToken } from "../config.js";

const auth = new Hono();

auth.post("/login", async (c) => {
  const body = await c.req.json<{ username: string; password: string }>();

  if (!body.username || !body.password) {
    return c.json({ error: "用户名和密码不能为空" }, 400);
  }

  // 首期明文比较（后续改 bcrypt hash）。
  const user = config.users.find((u) => u.username === body.username);
  if (!user || user.password !== body.password) {
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
    serverName: config.serverName,
  });
});

/**
 * 首次认证（魔法链接）——管理员粘贴签名 URL 后，客户端调此端点。
 * 验证 setup token → 签发 JWT（username=admin）→ 删除 setup token（一次性）。
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

  // 签发 JWT（username=admin，24h 有效）。
  const jwt = await new jose.SignJWT({ username: "admin" })
    .setProtectedHeader({ alg: "HS256" })
    .setIssuedAt()
    .setExpirationTime("24h")
    .sign(new TextEncoder().encode(config.jwtSecret));

  // 一次性：用完即删。
  deleteSetupToken();
  config.setupToken = null;

  return c.json({
    token: jwt,
    user: { username: "admin" },
    serverName: config.serverName,
    enterpriseId: config.enterpriseId,
    needsConfig: config.providers.length === 0,
  });
});

/**
 * 返回是否需要 setup token（客户端判断是否首次运行）。
 */
auth.get("/setup-status", (c) => {
  return c.json({
    needsSetup: !!config.setupToken,
    isFirstRun: config.users.length === 0 ||
      (config.users.length === 1 && config.users[0].username === "admin" && config.users[0].password === "admin"),
  });
});

export default auth;
