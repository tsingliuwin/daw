CREATE TABLE "enterprise" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"name" text DEFAULT 'AIOA 工作台' NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "llm_models" (
	"id" serial PRIMARY KEY NOT NULL,
	"provider_id" text,
	"model_id" text NOT NULL,
	"context_window" integer DEFAULT 256000 NOT NULL,
	"max_tokens" integer DEFAULT 64000
);
--> statement-breakpoint
CREATE TABLE "llm_providers" (
	"id" text PRIMARY KEY NOT NULL,
	"enterprise_id" uuid,
	"name" text NOT NULL,
	"endpoint" text NOT NULL,
	"api_key" text NOT NULL,
	"api_format" text NOT NULL,
	"enabled" boolean DEFAULT true NOT NULL
);
--> statement-breakpoint
CREATE TABLE "search_config" (
	"enterprise_id" uuid PRIMARY KEY NOT NULL,
	"engine" text DEFAULT '' NOT NULL,
	"api_key" text DEFAULT '' NOT NULL
);
--> statement-breakpoint
CREATE TABLE "users" (
	"id" serial PRIMARY KEY NOT NULL,
	"enterprise_id" uuid,
	"username" text NOT NULL,
	"password_hash" text NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
ALTER TABLE "llm_models" ADD CONSTRAINT "llm_models_provider_id_llm_providers_id_fk" FOREIGN KEY ("provider_id") REFERENCES "public"."llm_providers"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "llm_providers" ADD CONSTRAINT "llm_providers_enterprise_id_enterprise_id_fk" FOREIGN KEY ("enterprise_id") REFERENCES "public"."enterprise"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "search_config" ADD CONSTRAINT "search_config_enterprise_id_enterprise_id_fk" FOREIGN KEY ("enterprise_id") REFERENCES "public"."enterprise"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "users" ADD CONSTRAINT "users_enterprise_id_enterprise_id_fk" FOREIGN KEY ("enterprise_id") REFERENCES "public"."enterprise"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
CREATE UNIQUE INDEX "users_enterprise_username_idx" ON "users" USING btree ("enterprise_id","username");