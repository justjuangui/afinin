// SPDX-License-Identifier: MIT OR Apache-2.0

//! Data stores: RDS, DynamoDB, ElastiCache and S3.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::{Ctx, aws_err, s, time};
use crate::error::Result;
use crate::inventory::{
    Collected, RelationKind as K, Resource, ResourceType as T, arn, normalize_dns,
};

pub async fn rds(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_rds::Client::new(ctx.config);
    let (acct, region) = (ctx.account, ctx.region);
    let mut out = Collected::default();

    for page in client
        .describe_db_instances()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for db in page.db_instances() {
            let (Some(me), Some(id)) = (db.db_instance_arn(), db.db_instance_identifier()) else {
                continue;
            };
            let subnets = db.db_subnet_group();
            let mut r = Resource::new(T::RdsInstance, me, acct, region, id);
            r.name = Some(id.to_owned());
            r.with_tags(db.tag_list().iter().map(|t| (t.key(), t.value())))
                .attr("engine", s(db.engine()))
                .attr("engine_version", s(db.engine_version()))
                .attr("instance_class", s(db.db_instance_class()))
                .attr("status", s(db.db_instance_status()))
                .attr(
                    "allocated_storage_gib",
                    db.allocated_storage().map(|v| json!(v)),
                )
                .attr("multi_az", db.multi_az().map(Value::Bool))
                .attr(
                    "dns_name",
                    s(db.endpoint()
                        .and_then(|e| e.address())
                        .map(normalize_dns)
                        .as_deref()),
                )
                .attr("vpc_id", s(subnets.and_then(|g| g.vpc_id())))
                .attr("create_time", time(db.instance_create_time()));
            if let Some(cluster) = db.db_cluster_identifier() {
                out.relate(
                    me,
                    format!(
                        "arn:{}:rds:{region}:{acct}:cluster:{cluster}",
                        arn::partition(region)
                    ),
                    K::MemberOf,
                );
            }
            for subnet in subnets
                .into_iter()
                .flat_map(|g| g.subnets())
                .filter_map(|x| x.subnet_identifier())
            {
                out.relate(me, arn::ec2(region, acct, "subnet", subnet), K::InNetwork);
            }
            for sg in db
                .vpc_security_groups()
                .iter()
                .filter_map(|g| g.vpc_security_group_id())
            {
                out.relate(
                    me,
                    arn::ec2(region, acct, "security-group", sg),
                    K::SecuredBy,
                );
            }
            out.resources.push(r);
        }
    }

    for page in client
        .describe_db_clusters()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for c in page.db_clusters() {
            let (Some(me), Some(id)) = (c.db_cluster_arn(), c.db_cluster_identifier()) else {
                continue;
            };
            let mut r = Resource::new(T::RdsCluster, me, acct, region, id);
            r.name = Some(id.to_owned());
            r.with_tags(c.tag_list().iter().map(|t| (t.key(), t.value())))
                .attr("engine", s(c.engine()))
                .attr("engine_mode", s(c.engine_mode()))
                .attr("status", s(c.status()))
                .attr("dns_name", s(c.endpoint().map(normalize_dns).as_deref()))
                .attr("create_time", time(c.cluster_create_time()));
            for sg in c
                .vpc_security_groups()
                .iter()
                .filter_map(|g| g.vpc_security_group_id())
            {
                out.relate(
                    me,
                    arn::ec2(region, acct, "security-group", sg),
                    K::SecuredBy,
                );
            }
            out.resources.push(r);
        }
    }
    Ok(out)
}

/// CloudWatch GetMetricData, per metric requested.
pub const METRIC_COST_USD: f64 = 0.01 / 1000.0;

/// Days of CloudWatch capacity metrics read for provisioned tables.
pub const DYNAMODB_METRIC_DAYS: i64 = 14;

pub async fn dynamodb(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_dynamodb::Client::new(ctx.config);
    let (acct, region) = (ctx.account, ctx.region);
    let mut out = Collected::default();
    for page in client
        .list_tables()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for name in page.table_names() {
            let desc = client
                .describe_table()
                .table_name(name)
                .send()
                .await
                .map_err(aws_err)?;
            let Some(t) = desc.table() else { continue };
            let Some(me) = t.table_arn() else { continue };
            let mut r = Resource::new(T::DynamoDbTable, me, acct, region, name);
            r.name = Some(name.clone());
            let capacity = |p: Option<&aws_sdk_dynamodb::types::ProvisionedThroughputDescription>| {
                p.map_or((0, 0), |p| {
                    (
                        p.read_capacity_units().unwrap_or(0),
                        p.write_capacity_units().unwrap_or(0),
                    )
                })
            };
            let (read, write) = capacity(t.provisioned_throughput());
            let indexes = t.global_secondary_indexes();
            let (gsi_read, gsi_write) = indexes
                .iter()
                .map(|i| capacity(i.provisioned_throughput()))
                .fold((0, 0), |(r, w), (ir, iw)| (r + ir, w + iw));
            let index_bytes: i64 = indexes.iter().filter_map(|i| i.index_size_bytes()).sum();
            // Local indexes have no capacity of their own, but are stored and
            // backed up with the table.
            let local_bytes: i64 = t
                .local_secondary_indexes()
                .iter()
                .filter_map(|i| i.index_size_bytes())
                .sum();
            // Other regions holding a copy (global table); each copy is
            // collected, and billed, in its own region.
            let replicas: Vec<Value> = t
                .replicas()
                .iter()
                .filter_map(|x| x.region_name())
                .filter(|x| *x != region)
                .map(|x| json!(x))
                .collect();
            r.attr("status", s(t.table_status().map(|x| x.as_str())))
                .attr("item_count", t.item_count().map(|v| json!(v)))
                .attr("size_bytes", t.table_size_bytes().map(|v| json!(v)))
                .attr(
                    "billing_mode",
                    s(t.billing_mode_summary()
                        .and_then(|b| b.billing_mode())
                        .map(|m| m.as_str()))
                    .or(Some(json!("PROVISIONED"))),
                )
                .attr(
                    "table_class",
                    s(t.table_class_summary()
                        .and_then(|c| c.table_class())
                        .map(|c| c.as_str()))
                    .or(Some(json!("STANDARD"))),
                )
                .attr("read_capacity", Some(json!(read)))
                .attr("write_capacity", Some(json!(write)))
                .attr("gsi_count", Some(json!(indexes.len())))
                .attr("gsi_read_capacity", Some(json!(gsi_read)))
                .attr("gsi_write_capacity", Some(json!(gsi_write)))
                .attr("index_size_bytes", Some(json!(index_bytes)))
                .attr("local_index_size_bytes", Some(json!(local_bytes)))
                .attr(
                    "replica_regions",
                    (!replicas.is_empty()).then_some(Value::Array(replicas)),
                )
                .attr(
                    "stream_enabled",
                    Some(Value::Bool(t.latest_stream_arn().is_some())),
                )
                .attr("create_time", time(t.creation_date_time()));
            // Continuous backups are billed per GB; unknown when not allowed.
            match client
                .describe_continuous_backups()
                .table_name(name)
                .send()
                .await
            {
                Ok(b) => {
                    let on = b
                        .continuous_backups_description()
                        .and_then(|d| d.point_in_time_recovery_description())
                        .and_then(|p| p.point_in_time_recovery_status())
                        == Some(&aws_sdk_dynamodb::types::PointInTimeRecoveryStatus::Enabled);
                    r.attr("pitr", Some(Value::Bool(on)));
                }
                // One warning per region, not one per table.
                Err(e) if out.warnings.is_empty() => out.warnings.push(format!(
                    "dynamodb:DescribeContinuousBackups in {region}: {}",
                    aws_err(e)
                )),
                Err(_) => {}
            }
            out.resources.push(r);
        }
    }
    if out.resources.is_empty() {
        return Ok(out);
    }
    match autoscaled_tables(ctx).await {
        Ok(scaled) => {
            for r in &mut out.resources {
                let on = scaled.contains(&r.native_id);
                r.attr("autoscaling", Some(Value::Bool(on)));
            }
        }
        Err(e) => out.warnings.push(format!(
            "application-autoscaling:DescribeScalableTargets in {region}: {e}"
        )),
    }
    // Usage only matters where capacity is paid for whether used or not.
    let provisioned: Vec<String> = out
        .resources
        .iter()
        .filter(|r| r.attrs.get("billing_mode").and_then(|v| v.as_str()) == Some("PROVISIONED"))
        .map(|r| r.native_id.clone())
        .collect();
    if ctx.usage_metrics && !provisioned.is_empty() {
        match consumed_capacity(ctx, &provisioned, &mut out.metric_calls).await {
            Ok(usage) => {
                for r in &mut out.resources {
                    // A table whose metrics failed is unknown, not unused.
                    if let Some(u) = usage.get(&r.native_id).filter(|u| !u.failed) {
                        r.attr("metric_days", Some(json!(DYNAMODB_METRIC_DAYS)))
                            .attr("consumed_read_units", Some(json!(u.read_total)))
                            .attr("consumed_write_units", Some(json!(u.write_total)))
                            .attr("peak_read_per_s", Some(json!(u.read_peak)))
                            .attr("peak_write_per_s", Some(json!(u.write_peak)));
                    }
                }
            }
            Err(e) => out
                .warnings
                .push(format!("cloudwatch:GetMetricData in {region}: {e}")),
        }
    }
    Ok(out)
}

/// Tables (by name) whose own or an index's capacity is auto scaled.
async fn autoscaled_tables(ctx: &Ctx<'_>) -> Result<HashSet<String>> {
    use aws_sdk_applicationautoscaling::types::ServiceNamespace;
    let client = aws_sdk_applicationautoscaling::Client::new(ctx.config);
    let mut out = HashSet::new();
    for page in client
        .describe_scalable_targets()
        .service_namespace(ServiceNamespace::Dynamodb)
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for t in page.scalable_targets() {
            // `table/<name>` or `table/<name>/index/<index>`.
            if let Some(rest) = t.resource_id().strip_prefix("table/") {
                out.insert(rest.split('/').next().unwrap_or(rest).to_owned());
            }
        }
    }
    Ok(out)
}

#[derive(Default)]
struct Consumed {
    read_total: f64,
    write_total: f64,
    /// The busiest minute's average units per second.
    read_peak: f64,
    write_peak: f64,
    /// A metric query came back with an error.
    failed: bool,
}

/// Seconds per data point: DynamoDB publishes consumed capacity per minute
/// (kept at this resolution for 15 days), and burst capacity absorbs spikes
/// within a minute.
const METRIC_PERIOD: i64 = 60;

/// Capacity each table consumed over the last [`DYNAMODB_METRIC_DAYS`],
/// minute by minute. Indexes have metrics of their own and aren't
/// included. A table without data points consumed nothing. Every call made,
/// paginated ones included, goes to `calls` with the metrics it requested.
async fn consumed_capacity(
    ctx: &Ctx<'_>,
    tables: &[String],
    calls: &mut Vec<usize>,
) -> Result<HashMap<String, Consumed>> {
    use aws_sdk_cloudwatch::types::{Dimension, Metric, MetricDataQuery, MetricStat};
    let client = aws_sdk_cloudwatch::Client::new(ctx.config);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let end = now - now % METRIC_PERIOD;
    let start = end - DYNAMODB_METRIC_DAYS * 86_400;
    let mut out: HashMap<String, Consumed> = HashMap::new();
    // GetMetricData takes up to 500 queries: two per table.
    for chunk in tables.chunks(250) {
        let mut req = client
            .get_metric_data()
            .start_time(aws_smithy_types::DateTime::from_secs(start))
            .end_time(aws_smithy_types::DateTime::from_secs(end));
        for (i, table) in chunk.iter().enumerate() {
            out.entry(table.clone()).or_default();
            for (kind, metric) in [
                ("r", "ConsumedReadCapacityUnits"),
                ("w", "ConsumedWriteCapacityUnits"),
            ] {
                let stat = MetricStat::builder()
                    .metric(
                        Metric::builder()
                            .namespace("AWS/DynamoDB")
                            .metric_name(metric)
                            .dimensions(Dimension::builder().name("TableName").value(table).build())
                            .build(),
                    )
                    .period(METRIC_PERIOD as i32)
                    .stat("Sum")
                    .build();
                req = req.metric_data_queries(
                    MetricDataQuery::builder()
                        .id(format!("{kind}{i}"))
                        .metric_stat(stat)
                        .return_data(true)
                        .build(),
                );
            }
        }
        // Each page is a billed call: count it as it arrives, so pages
        // before a failed one are still counted.
        let mut pages = Vec::new();
        let mut stream = req.into_paginator().send();
        while let Some(page) = stream.next().await {
            pages.push(page.map_err(aws_err)?);
            calls.push(chunk.len() * 2);
        }
        for m in pages.iter().flat_map(|p| p.metric_data_results()) {
            let Some(id) = m.id() else { continue };
            let (kind, i) = id.split_at(1);
            let Some(table) = i.parse::<usize>().ok().and_then(|i| chunk.get(i)) else {
                continue;
            };
            let u = out.entry(table.clone()).or_default();
            // CloudWatch answers per query: `Complete`, or `PartialData` on
            // all but the last page; anything else means no reliable data.
            use aws_sdk_cloudwatch::types::StatusCode;
            if !matches!(m.status_code(), Some(StatusCode::Complete | StatusCode::PartialData)) {
                u.failed = true;
            }
            let total: f64 = m.values().iter().sum();
            let peak = m.values().iter().copied().fold(0.0, f64::max) / METRIC_PERIOD as f64;
            if kind == "r" {
                u.read_total += total;
                u.read_peak = u.read_peak.max(peak);
            } else {
                u.write_total += total;
                u.write_peak = u.write_peak.max(peak);
            }
        }
    }
    Ok(out)
}

pub async fn elasticache(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_elasticache::Client::new(ctx.config);
    let (acct, region) = (ctx.account, ctx.region);
    let mut out = Collected::default();

    let mut subnet_groups: HashMap<String, (Option<String>, Vec<String>)> = HashMap::new();
    for page in client
        .describe_cache_subnet_groups()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for g in page.cache_subnet_groups() {
            if let Some(name) = g.cache_subnet_group_name() {
                let subnets = g
                    .subnets()
                    .iter()
                    .filter_map(|x| x.subnet_identifier().map(str::to_owned))
                    .collect();
                subnet_groups.insert(name.to_owned(), (g.vpc_id().map(str::to_owned), subnets));
            }
        }
    }

    for page in client
        .describe_cache_clusters()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for c in page.cache_clusters() {
            let (Some(me), Some(id)) = (c.arn(), c.cache_cluster_id()) else {
                continue;
            };
            let group = c
                .cache_subnet_group_name()
                .and_then(|n| subnet_groups.get(n));
            let mut r = Resource::new(T::ElastiCacheCluster, me, acct, region, id);
            r.name = Some(c.replication_group_id().unwrap_or(id).to_owned());
            r.attr("engine", s(c.engine()))
                .attr("engine_version", s(c.engine_version()))
                .attr("node_type", s(c.cache_node_type()))
                .attr("nodes", c.num_cache_nodes().map(|n| json!(n)))
                .attr("status", s(c.cache_cluster_status()))
                .attr("replication_group", s(c.replication_group_id()))
                .attr("vpc_id", s(group.and_then(|g| g.0.as_deref())))
                .attr("create_time", time(c.cache_cluster_create_time()));
            for subnet in group.into_iter().flat_map(|g| &g.1) {
                out.relate(me, arn::ec2(region, acct, "subnet", subnet), K::InNetwork);
            }
            for sg in c
                .security_groups()
                .iter()
                .filter_map(|g| g.security_group_id())
            {
                out.relate(
                    me,
                    arn::ec2(region, acct, "security-group", sg),
                    K::SecuredBy,
                );
            }
            out.resources.push(r);
        }
    }
    Ok(out)
}

/// Buckets are listed once (the API is global) and placed in their own region.
pub async fn s3(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_s3::Client::new(ctx.config);
    let mut out = Collected::default();
    for page in client
        .list_buckets()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for b in page.buckets() {
            let Some(name) = b.name() else { continue };
            let region = b.bucket_region().unwrap_or(super::HOME_REGION);
            let mut r = Resource::new(
                T::S3Bucket,
                arn::s3_bucket(region, name),
                ctx.account,
                region,
                name,
            );
            r.name = Some(name.to_owned());
            r.attr("create_time", time(b.creation_date()))
                .attr("dns_name", Some(json!(format!("{name}.s3.amazonaws.com"))));
            out.resources.push(r);
        }
    }
    Ok(out)
}
