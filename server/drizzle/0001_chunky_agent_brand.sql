-- users 和 llm_models 的 id 从 serial 改为 uuid
-- 首期数据可丢弃，直接 DROP 重建
DROP TABLE IF EXISTS "llm_models";--> statement-breakpoint
DROP TABLE IF EXISTS "users";--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "llm_models" (
  "id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
  "provider_id" text REFERENCES "llm_providers"("id") ON DELETE cascade,
  "model_id" text NOT NULL,
  "context_window" integer DEFAULT 256000 NOT NULL,
  "max_tokens" integer DEFAULT 64000
);--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "users" (
  "id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
  "enterprise_id" uuid REFERENCES "enterprise"("id"),
  "username" text NOT NULL,
  "password_hash" text NOT NULL,
  "created_at" timestamptz DEFAULT now() NOT NULL
);--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS "users_enterprise_username_idx" ON "users" ("enterprise_id","username");