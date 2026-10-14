// SPDX-License-Identifier: MIT OR Apache-2.0

//! Read-only tools the model calls instead of receiving data dumps (FR-42).
//! Numbers always come from the local query engine (FR-46).

use serde_json::{Value, json};

use super::ToolSpec;
use crate::Afinin;
use crate::cost::{CostFilter, CostMetric, CostQuery, Granularity, GroupBy, dates};
use crate::error::{Error, Result};
use crate::findings::FindingKind;
use crate::inventory::graph::GraphFilter;

const DATE: &str = "Date as YYYY-MM-DD (UTC)";

fn group_by_schema() -> Value {
    json!({"type": "string", "enum": ["service", "account", "region", "usage_type", "resource", "tag"],
           "description": "Dimension to break costs down by. `resource` needs CUR data; `tag` needs tag_key."})
}

fn common_cost_props() -> serde_json::Map<String, Value> {
    let v = json!({
        "group_by": group_by_schema(),
        "tag_key": {"type": "string", "description": "Cost allocation tag key when group_by is `tag`."},
        "metric": {"type": "string", "enum": ["unblended", "amortized", "net_unblended"], "description": "Default unblended."},
        "account_ids": {"type": "array", "items": {"type": "string"}, "description": "Only these AWS account IDs."},
        "services": {"type": "array", "items": {"type": "string"}, "description": "Only these services (Cost Explorer names); with Cost Explorer data only when grouping by service or account."},
        "top_n": {"type": "integer", "minimum": 1, "maximum": 50, "description": "Groups to return; the rest is summed as other. Default 10."}
    });
    v.as_object().cloned().unwrap_or_default()
}

pub fn specs() -> Vec<ToolSpec> {
    let mut query = common_cost_props();
    query.insert(
        "start".into(),
        json!({"type": "string", "description": format!("{DATE}, inclusive")}),
    );
    query.insert(
        "end".into(),
        json!({"type": "string", "description": format!("{DATE}, exclusive")}),
    );
    query.insert(
        "granularity".into(),
        json!({"type": "string", "enum": ["daily", "monthly"]}),
    );
    query.insert(
        "include_periods".into(),
        json!({"type": "boolean", "description": "Also return the total per day or month."}),
    );

    let mut compare = common_cost_props();
    for (k, d) in [
        ("current_start", "inclusive"),
        ("current_end", "exclusive"),
        ("previous_start", "inclusive"),
        ("previous_end", "exclusive"),
    ] {
        compare.insert(
            k.into(),
            json!({"type": "string", "description": format!("{DATE}, {d}")}),
        );
    }

    vec![
        ToolSpec {
            name: "get_cost_overview",
            description: "Month-to-date spend, change vs the same days last month, last month's total and the month-end forecast.",
            schema: json!({"type": "object", "properties": {
                "account_ids": {"type": "array", "items": {"type": "string"}, "description": "Only these AWS account IDs."}
            }}),
        },
        ToolSpec {
            name: "query_costs",
            description: "Total cost per group (service, account, region, usage type, resource or tag value) over a date range, from synced Cost Explorer or CUR data.",
            schema: json!({"type": "object", "properties": query, "required": ["group_by", "start", "end"]}),
        },
        ToolSpec {
            name: "compare_periods",
            description: "Cost per group in two periods with the change, largest changes first. Use it to explain why spend went up or down.",
            schema: json!({"type": "object", "properties": compare,
                           "required": ["group_by", "current_start", "current_end", "previous_start", "previous_end"]}),
        },
        ToolSpec {
            name: "list_findings",
            description: "Open findings: waste with estimated monthly savings, cost anomalies and patterns, most severe first.",
            schema: json!({"type": "object", "properties": {
                "kind": {"type": "string", "enum": ["waste", "anomaly", "pattern"]},
                "limit": {"type": "integer", "minimum": 1, "maximum": 50}
            }}),
        },
        ToolSpec {
            name: "get_finding",
            description: "Full details of one finding: evidence, affected resources and how to fix it.",
            schema: json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        },
        ToolSpec {
            name: "search_resources",
            description: "Find discovered resources by name, ID or ARN fragment, optionally of one type (e.g. `rds:db`, `ec2:instance`).",
            schema: json!({"type": "object", "properties": {
                "query": {"type": "string"},
                "resource_type": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 50}
            }, "required": ["query"]}),
        },
        ToolSpec {
            name: "get_resource",
            description: "One resource's details, tags, monthly cost (per resource with CUR, otherwise its service's) and direct connections.",
            schema: json!({"type": "object", "properties": {"arn": {"type": "string"}}, "required": ["arn"]}),
        },
        ToolSpec {
            name: "get_neighbors",
            description: "Resources connected to a resource within `depth` hops (load balancers, targets, volumes, event sources, databases…).",
            schema: json!({"type": "object", "properties": {
                "arn": {"type": "string"},
                "depth": {"type": "integer", "minimum": 1, "maximum": 3}
            }, "required": ["arn"]}),
        },
        ToolSpec {
            name: "get_insights",
            description: "Top movers by service (last 30 days vs previous 30), data transfer breakdown and Savings Plans / RI coverage.",
            schema: json!({"type": "object", "properties": {}}),
        },
    ]
}

fn str_arg<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    input[key]
        .as_str()
        .ok_or_else(|| Error::Other(format!("missing `{key}`")))
}

fn strings(input: &Value, key: &str) -> Vec<String> {
    input[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

fn round(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn cost_query(workspace_id: &str, input: &Value, start: &str, end: &str) -> Result<CostQuery> {
    let group_by = match str_arg(input, "group_by")? {
        "service" => GroupBy::Service,
        "account" => GroupBy::Account,
        "region" => GroupBy::Region,
        "usage_type" => GroupBy::UsageType,
        "resource" => GroupBy::Resource,
        "tag" => GroupBy::Tag(str_arg(input, "tag_key")?.to_owned()),
        other => return Err(Error::Other(format!("unknown group_by `{other}`"))),
    };
    let metric = match input["metric"].as_str() {
        Some("amortized") => CostMetric::Amortized,
        Some("net_unblended") => CostMetric::NetUnblended,
        _ => CostMetric::Unblended,
    };
    let days = (dates::parse(end)? - dates::parse(start)?).whole_days();
    let granularity = match input["granularity"].as_str() {
        Some("daily") => Granularity::Daily,
        Some("monthly") => Granularity::Monthly,
        _ if days > 62 => Granularity::Monthly,
        _ => Granularity::Daily,
    };
    Ok(CostQuery {
        workspace_id: workspace_id.to_owned(),
        start: start.to_owned(),
        end: end.to_owned(),
        granularity,
        metric,
        group_by,
        filter: CostFilter {
            linked_accounts: strings(input, "account_ids"),
            services: strings(input, "services"),
        },
    })
}

fn top_n(input: &Value) -> usize {
    input["top_n"].as_u64().unwrap_or(10).clamp(1, 50) as usize
}

/// Runs one tool. Errors are returned to the model as tool errors.
pub fn execute(afinin: &Afinin, workspace_id: &str, name: &str, input: &Value) -> Result<Value> {
    match name {
        "get_cost_overview" => {
            let o = afinin.cost_overview(
                workspace_id,
                afinin.settings()?.default_metric,
                &strings(input, "account_ids"),
            )?;
            Ok(json!({
                "metric": o.metric, "unit": o.unit,
                "month_to_date": round(o.month_to_date),
                "previous_month_same_days": round(o.previous_month_to_date),
                "month_over_month_pct": o.month_over_month_pct.map(round),
                "last_month_total": round(o.last_month_total),
                "forecast_month_end": o.forecast_month_end.map(round),
                "data_through": o.data_through,
                "source": "Cost Explorer",
            }))
        }
        "query_costs" => {
            let q = cost_query(
                workspace_id,
                input,
                str_arg(input, "start")?,
                str_arg(input, "end")?,
            )?;
            let s = afinin.cost_series(&q)?;
            let n = top_n(input);
            let other: f64 = s.groups.iter().skip(n).map(|g| g.total).sum();
            let mut out = json!({
                "source": if q.group_by == GroupBy::Resource { "CUR 2.0" } else { "Cost Explorer" },
                "metric": q.metric, "unit": s.unit, "start": q.start, "end": q.end,
                "total": round(s.total),
                "groups": s.groups.iter().take(n).map(|g| json!({
                    "key": g.key, "label": g.label, "total": round(g.total),
                    "share_pct": if s.total > 0.0 { round(g.total / s.total * 100.0) } else { 0.0 },
                })).collect::<Vec<_>>(),
                "other_groups": s.groups.len().saturating_sub(n),
                "other_total": round(other),
                "estimated_periods": s.estimated_periods,
            });
            if input["include_periods"].as_bool() == Some(true) {
                out["periods"] = json!(
                    s.periods
                        .iter()
                        .zip(&s.period_totals)
                        .map(|(p, t)| json!({"period": p, "total": round(*t)}))
                        .collect::<Vec<_>>()
                );
            }
            Ok(out)
        }
        "compare_periods" => {
            let q = cost_query(
                workspace_id,
                input,
                str_arg(input, "current_start")?,
                str_arg(input, "current_end")?,
            )?;
            let (ps, pe) = (
                str_arg(input, "previous_start")?,
                str_arg(input, "previous_end")?,
            );
            let rows = afinin.compare_costs(&q, ps, pe)?;
            let current: f64 = rows.iter().map(|r| r.current).sum();
            let previous: f64 = rows.iter().map(|r| r.previous).sum();
            Ok(json!({
                "metric": q.metric,
                "current": {"start": q.start, "end": q.end, "total": round(current)},
                "previous": {"start": ps, "end": pe, "total": round(previous)},
                "change": round(current - previous),
                "change_pct": crate::cost::pct_change(current, previous).map(round),
                "rows": rows.iter().take(top_n(input)).map(|r| json!({
                    "label": r.label, "current": round(r.current), "previous": round(r.previous),
                    "change": round(r.delta), "change_pct": r.delta_pct.map(round),
                })).collect::<Vec<_>>(),
            }))
        }
        "list_findings" => {
            let kind = input["kind"].as_str();
            let limit = input["limit"].as_u64().unwrap_or(20).clamp(1, 50) as usize;
            let all = afinin.findings(workspace_id, false)?;
            let list: Vec<Value> = all
                .iter()
                .filter(|f| match kind {
                    Some("waste") => f.kind == FindingKind::Waste,
                    Some("anomaly") => f.kind == FindingKind::Anomaly,
                    Some("pattern") => f.kind == FindingKind::Pattern,
                    _ => true,
                })
                .take(limit)
                .map(|f| json!({
                    "id": f.id, "kind": f.kind, "severity": f.severity, "title": f.title,
                    "account_id": f.account_id, "region": f.region,
                    "monthly_savings": f.monthly_savings, "savings_basis": f.savings_basis, "impact": f.impact,
                }))
                .collect();
            let savings: f64 = all.iter().filter_map(|f| f.monthly_savings).sum();
            Ok(
                json!({"open_findings": all.len(), "total_monthly_savings": round(savings), "findings": list}),
            )
        }
        "get_finding" => {
            let id = str_arg(input, "id")?;
            let f = afinin
                .findings(workspace_id, true)?
                .into_iter()
                .find(|f| f.id == id)
                .ok_or_else(|| Error::NotFound(format!("finding {id}")))?;
            serde_json::to_value(&f).map_err(|e| Error::Other(e.to_string()))
        }
        "search_resources" => {
            let q = str_arg(input, "query")?.to_ascii_lowercase();
            let wanted = input["resource_type"].as_str();
            let limit = input["limit"].as_u64().unwrap_or(20).clamp(1, 50) as usize;
            let graph = afinin.inventory_graph(
                workspace_id,
                &GraphFilter {
                    include_network: true,
                    ..Default::default()
                },
            )?;
            let hits: Vec<Value> = graph
                .nodes
                .iter()
                .filter(|n| wanted.is_none_or(|t| n.resource_type.id() == t))
                .filter(|n| {
                    n.label.to_ascii_lowercase().contains(&q) || n.native_id.to_ascii_lowercase().contains(&q) || n.id.to_ascii_lowercase().contains(&q)
                })
                .take(limit)
                .map(|n| json!({
                    "arn": n.id, "type": n.resource_type.id(), "name": n.label, "region": n.region,
                    "account_id": n.account_id, "state": n.attrs.get("state").or_else(|| n.attrs.get("status")),
                }))
                .collect();
            Ok(json!({"matches": hits}))
        }
        "get_resource" => {
            let arn = str_arg(input, "arn")?;
            let graph = afinin.inventory_graph(
                workspace_id,
                &GraphFilter {
                    include_network: true,
                    ..Default::default()
                },
            )?;
            let node = graph
                .nodes
                .iter()
                .find(|n| n.id == arn)
                .ok_or_else(|| Error::NotFound(format!("resource {arn}")))?;
            let label = |id: &str| {
                graph
                    .nodes
                    .iter()
                    .find(|n| n.id == id)
                    .map(|n| (n.label.clone(), n.resource_type.id()))
            };
            let connections: Vec<Value> = graph
                .edges
                .iter()
                .filter(|e| e.source == arn || e.target == arn)
                .map(|e| {
                    let (other, direction) = if e.source == arn { (&e.target, "to") } else { (&e.source, "from") };
                    let (name, t) = label(other).unwrap_or_default();
                    json!({"relation": e.kind, "direction": direction, "arn": other, "name": name, "type": t})
                })
                .collect();
            Ok(json!({
                "arn": node.id, "type": node.resource_type.id(), "name": node.label, "account_id": node.account_id,
                "region": node.region, "vpc_id": node.vpc_id, "attributes": node.attrs, "tags": node.tags,
                "monthly_cost_30d": node.cost.map(round),
                "cost_basis": if graph.cost_basis == crate::inventory::graph::CostBasis::Resource { "this resource (CUR)" } else { "its whole service in this account (Cost Explorer)" },
                "connections": connections,
            }))
        }
        "get_neighbors" => {
            let arn = str_arg(input, "arn")?;
            let depth = input["depth"].as_u64().unwrap_or(2).clamp(1, 3) as usize;
            let found = afinin.resource_neighbors(workspace_id, arn, depth)?;
            Ok(
                json!({"resource": arn, "neighbors": found.iter().map(|(r, d)| json!({
                "arn": r.arn, "type": r.resource_type.id(), "name": r.display_name(), "distance": d,
            })).collect::<Vec<_>>()}),
            )
        }
        "get_insights" => serde_json::to_value(afinin.insights(workspace_id)?)
            .map_err(|e| Error::Other(e.to_string())),
        other => Err(Error::Other(format!("unknown tool `{other}`"))),
    }
}
