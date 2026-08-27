use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::super::agent::error::ToolError;
use super::super::super::agent::events::{emit_chart, emit_tool_call, emit_tool_result, emit_tool_result_with_meta, next_tool_id};
use super::super::super::duckdb::{execute, QUERY_HARD_TIMEOUT_SECS};
use super::super::super::model::SqlResult;
use super::super::super::state::AppState;

#[derive(Deserialize, Serialize)]
pub struct RenderChartArgs {
    sql: String,
    chart_type: String,
    #[serde(default)]
    x_field: Option<String>,
    #[serde(default)]
    y_fields: Option<Vec<String>>,
    #[serde(default)]
    right_y_fields: Option<Vec<String>>,
    #[serde(default)]
    y_field_labels: Option<HashMap<String, String>>,
    #[serde(default)]
    title: Option<String>,
}

pub struct RenderChartTool {
    pub app_state: AppState,
    pub task_id: String,
    pub window: tauri::Window,
    pub ws: crate::skill::WorkspaceRef,
}

/// Chinese label for a chart type (used in the tool result summary).
fn chart_type_cn(t: &str) -> &str {
    match t {
        "bar" => "柱状",
        "line" => "折线",
        "pie" => "饼",
        "scatter" => "散点",
        "funnel" => "漏斗",
        "gauge" => "仪表盘",
        _ => "图表",
    }
}

impl Tool for RenderChartTool {
    const NAME: &'static str = "render_chart";
    type Error = ToolError;
    type Args = RenderChartArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "render_chart".to_string(),
            description: "用图表可视化查询结果。先写好 SELECT 语句（和 execute_query 一样），指定图表类型和轴映射。适合趋势（折线 line）、对比（柱状 bar）、占比（饼图 pie）、相关性（散点 scatter）。数据超过 200 行只取前 200 行——数据点多时先在 SQL 里聚合或过滤。图表会展示在对话中，用户可切换图表类型。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "sql": { "type": "string", "description": "用于获取图表数据的 SELECT 查询语句" },
                    "chart_type": { "type": "string", "enum": ["bar", "line", "pie", "scatter", "funnel", "gauge"], "description": "图表类型：bar(柱状对比)、line(趋势)、pie(占比)、scatter(相关性)、funnel(转化漏斗)、gauge(单值指标)" },
                    "x_field": { "type": "string", "description": "X 轴/分类列名（饼图时为名称列）。若为时间维度（月份、季度、年份、日期等），SQL 应加 ORDER BY 按时间排序，避免横轴乱序" },
                    "y_fields": { "type": "array", "items": { "type": "string" }, "description": "Y 轴/数值列名，支持多列（多系列）。饼图时取第一个" },
                    "right_y_fields": { "type": "array", "items": { "type": "string" }, "description": "双 Y 轴：放到右轴的列名（须为 y_fields 的子集）。仅当序列数量级差异大、单轴会把小量级序列压成直线时使用。默认全部在左轴" },
                    "y_field_labels": { "type": "object", "additionalProperties": { "type": "string" }, "description": "列名→可读标签（含单位）的映射，如 revenue→销售额(万元)。图例与轴名会用它显示单位" },
                    "title": { "type": "string", "description": "图表标题（可选）" }
                },
                "required": ["sql", "chart_type"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let sql = args.sql.trim();
        if let Some(kw) = super::sql_forbidden_keyword(sql) {
            return Err(ToolError(format!("出于安全考虑，禁止执行包含 {} 操作的 SQL 语句。", kw)));
        }

        let valid_types = ["bar", "line", "pie", "scatter", "funnel", "gauge"];
        if !valid_types.contains(&args.chart_type.as_str()) {
            return Err(ToolError(format!(
                "不支持的图表类型「{}」，可选：bar / line / pie / scatter / funnel / gauge", args.chart_type
            )));
        }

        let call_id = next_tool_id("chart");
        emit_tool_call(&self.window, &self.task_id, &call_id, "render_chart", json!({
            "sql": sql,
            "chart_type": args.chart_type,
            "x_field": args.x_field,
            "y_fields": args.y_fields,
            "right_y_fields": args.right_y_fields,
            "y_field_labels": args.y_field_labels,
        }));

        let start = std::time::Instant::now();

        let wsc = match self.app_state.ensure_workspace_conn(&self.ws.path).await {
            Ok(w) => w,
            Err(msg) => {
                let full = format!("DuckDB 引擎未就绪: {msg}");
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", full.clone(), None, None, Some(0), None);
                return Err(ToolError(full));
            }
        };
        let conn = wsc.conn.clone();
        let ih = wsc.interrupt_handle.lock().ok().map(|g| g.clone());

        let sql_string = sql.to_string();
        // 实际落 DuckDB 的语句（行数包装），日志与 transcript meta 用。
        let sql_executed = execute::wrap_query(&sql_string, Some(200));
        let hard_secs = QUERY_HARD_TIMEOUT_SECS;
        let blocking_fut = tokio::task::spawn_blocking(move || -> Result<SqlResult, String> {
            let guard = conn.blocking_lock();
            execute::run_query(&guard, &sql_string, Some(200)).map_err(|e| e.to_string())
        });
        let run_res = if hard_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(hard_secs), blocking_fut).await {
                Ok(r) => r.map_err(|e| format!("线程生成失败: {e}")).and_then(|v| v),
                Err(_) => {
                    if let Some(ih) = ih {
                        ih.interrupt();
                    }
                    Err(format!("图表查询已达到最大等待时间（{} 秒）被强制终止", hard_secs))
                }
            }
        } else {
            blocking_fut.await.map_err(|e| format!("线程生成失败: {e}")).and_then(|v| v)
        };
        let res: SqlResult = match run_res {
            Ok(v) => v,
            Err(msg) => {
                let err_class = super::classify_sql_error_class(&msg);
                let meta = serde_json::json!({
                    "sql": sql_executed,
                    "sqlOriginal": sql,
                    "rows": 0,
                    "truncated": false,
                    "errorClass": err_class,
                });
                let ws_log = self.ws.path.clone();
                let task_log = self.task_id.clone();
                tracing::error!(
                    category = "sql",
                    workspace = ws_log.as_str(),
                    task_id = task_log.as_str(),
                    detail = ?meta,
                    "render_chart 失败({}): {}",
                    err_class, msg,
                );
                emit_tool_result_with_meta(
                    &self.window, &self.task_id, &call_id, "error",
                    msg.clone(), None, None, None, None, Some(meta),
                );
                return Err(ToolError(msg));
            }
        };

        let elapsed = start.elapsed().as_millis() as u64;
        {
            let row_count = res.row_count;
            let meta = serde_json::json!({
                "sql": sql_executed,
                "sqlOriginal": sql,
                "rows": row_count,
                "truncated": res.truncated,
                "errorClass": serde_json::Value::Null,
            });
            // Emit the chart segment — frontend renders it inline.
            emit_chart(
                &self.window, &self.task_id, &call_id,
                &args.chart_type,
                args.title.as_deref(),
                args.x_field.as_deref(),
                args.y_fields.as_deref(),
                args.right_y_fields.as_deref(),
                args.y_field_labels.as_ref(),
                res,
            );
            let user_summary = format!("已生成{}图，共 {} 个数据点", chart_type_cn(&args.chart_type), row_count);
            let ws_log = self.ws.path.clone();
            let task_log = self.task_id.clone();
            tracing::info!(
                category = "sql",
                workspace = ws_log.as_str(),
                task_id = task_log.as_str(),
                detail = ?meta,
                "render_chart 成功，{} 个数据点（{} ms）",
                row_count, elapsed,
            );
            emit_tool_result_with_meta(
                &self.window, &self.task_id, &call_id, "ok",
                user_summary.clone(), None, None, Some(elapsed), None, Some(meta),
            );
            // {{chart:<id>}} marker lets the model reference this chart in
            // its final text summary; the frontend matches id == call_id.
            let marker = "{{chart:".to_string() + &call_id + "}}";
            Ok(format!(
                "{}。在结论中引用此图，请将以下标记原样粘贴到结论对应位置：{}",
                user_summary, marker
            ))
        }
    }
}
