// SPDX-License-Identifier: MIT OR Apache-2.0

//! Data stores: RDS, DynamoDB, ElastiCache and S3.

use std::collections::HashMap;

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
                    "stream_enabled",
                    Some(Value::Bool(t.latest_stream_arn().is_some())),
                )
                .attr("create_time", time(t.creation_date_time()));
            out.resources.push(r);
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
