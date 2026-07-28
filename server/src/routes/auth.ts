/**
 * 认证路由：POST /auth/login
 *
 * 用户名密码 → 验证 → 签发 JWT（24h 有效）。
 */

import { Hono } from "hono";
import * as jose from "jose";
import { config } from "../config.js";

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

export default auth;
