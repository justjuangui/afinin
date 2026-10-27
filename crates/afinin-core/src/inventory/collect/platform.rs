// SPDX-License-Identifier: MIT OR Apache-2.0

//! Platform services billed per item or per GB stored: Secrets Manager
//! secrets, KMS keys, ECR repositories, CloudWatch log groups, EFS file
//! systems (regional) and Route 53 hosted zones (global). Only metadata is
//! read: never secret values, key material, images or log events.

use serde_json::{Value, json};

use super::{Ctx, aws_err, s, time};
use crate::error::Result;
use crate::inventory::{Collected, RelationKind as K, Resource, ResourceType as T, arn};

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

fn gib(bytes: i64) -> Option<Value> {
    Some(json!(((bytes.max(0) as f64 / GIB) * 1000.0).round() / 1000.0))
}

/// RFC 3339 from epoch milliseconds (CloudWatch Logs reports times so).
fn millis(ms: Option<i64>) -> Option<Value> {
    let t = ::time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms?) * 1_000_000).ok()?;
    t.format(&::time::format_description::well_known::Rfc3339)
        .ok()
        .map(Value::String)
}

pub async fn secrets(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_secretsmanager::Client::new(ctx.config);
    let mut out = Collected::default();
    for page in client
        .list_secrets()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for sec in page.secret_list() {
            let (Some(me), Some(name)) = (sec.arn(), sec.name()) else {
                continue;
            };
            let mut r = Resource::new(T::SecretsManagerSecret, me, ctx.account, ctx.region, name);
            r.name = Some(name.to_owned());
            r.with_tags(sec.tags().iter().map(|t| (t.key(), t.value())))
                .attr("created", time(sec.created_date()))
                .attr("last_accessed", time(sec.last_accessed_date()))
                .attr("last_changed", time(sec.last_changed_date()))
                .attr("rotation_enabled", sec.rotation_enabled().map(Value::Bool))
                .attr("deleted", time(sec.deleted_date()))
                // Secrets that RDS, Redshift and others create and rotate.
                .attr("owning_service", s(sec.owning_service()));
            out.resources.push(r);
        }
    }
    Ok(out)
}

/// Customer-managed keys only: AWS-managed keys are free and not the
/// account's to manage.
pub async fn kms(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_kms::Client::new(ctx.config);
    let mut out = Collected::default();
    let mut aliases: std::collections::HashMap<String, String> = Default::default();
    for page in client
        .list_aliases()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for a in page.aliases() {
            if let (Some(key), Some(name)) = (a.target_key_id(), a.alias_name()) {
                aliases.entry(key.to_owned()).or_insert_with(|| name.to_owned());
            }
        }
    }
    for page in client
        .list_keys()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for k in page.keys() {
            let Some(id) = k.key_id() else { continue };
            let meta = match client.describe_key().key_id(id).send().await {
                Ok(d) => d.key_metadata,
                Err(e) => {
                    out.warnings
                        .push(format!("kms in {}: key {id}: {}", ctx.region, aws_err(e)));
                    continue;
                }
            };
            let Some(meta) = meta else { continue };
            if meta.key_manager().map(|m| m.as_str()) != Some("CUSTOMER") {
                continue;
            }
            let me = meta.arn().map_or_else(
                || format!("arn:{}:kms:{}:{}:key/{id}", arn::partition(ctx.region), ctx.region, ctx.account),
                str::to_owned,
            );
            let mut r = Resource::new(T::KmsKey, me, ctx.account, ctx.region, id);
            r.name = aliases.get(id).cloned();
            r.attr("state", s(meta.key_state().map(|v| v.as_str())))
                .attr("key_spec", s(meta.key_spec().map(|v| v.as_str())))
                .attr("usage", s(meta.key_usage().map(|v| v.as_str())))
                .attr("origin", s(meta.origin().map(|v| v.as_str())))
                .attr("multi_region", meta.multi_region().map(Value::Bool))
                .attr("description", s(meta.description()))
                .attr("created", time(meta.creation_date()))
                .attr("deletion_date", time(meta.deletion_date()));
            out.resources.push(r);
        }
    }
    Ok(out)
}

pub async fn ecr(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_ecr::Client::new(ctx.config);
    let mut out = Collected::default();
    for page in client
        .describe_repositories()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for repo in page.repositories() {
            let (Some(me), Some(name)) = (repo.repository_arn(), repo.repository_name()) else {
                continue;
            };
            let mut r = Resource::new(T::EcrRepository, me, ctx.account, ctx.region, name);
            r.name = Some(name.to_owned());
            r.attr("uri", s(repo.repository_uri()))
                .attr("created", time(repo.created_at()))
                .attr(
                    "tag_mutability",
                    s(repo.image_tag_mutability().map(|v| v.as_str())),
                );

            // Image counts and sizes. Sizes add up shared layers once per
            // image, so the total overstates what ECR bills.
            match client
                .describe_images()
                .repository_name(name)
                .into_paginator()
                .send()
                .try_collect()
                .await
            {
                Ok(pages) => {
                    let images: Vec<_> = pages.iter().flat_map(|p| p.image_details()).collect();
                    // Untagged and not attached to another image (signatures,
                    // SBOMs): candidates for a lifecycle policy. The
                    // per-architecture images of a multi-arch image are
                    // untagged too but in use; `multi_arch` flags repositories
                    // where that can't be told apart.
                    let loose = |i: &aws_sdk_ecr::types::ImageDetail| {
                        i.image_tags().is_empty() && i.subject_manifest_digest().is_none()
                    };
                    let multi_arch = images.iter().any(|i| {
                        i.image_manifest_media_type().is_some_and(|m| {
                            m.contains("manifest.list") || m.contains("image.index")
                        })
                    });
                    let size = |only_loose: bool| -> i64 {
                        images
                            .iter()
                            .filter(|i| !only_loose || loose(i))
                            .filter_map(|i| i.image_size_in_bytes())
                            .sum()
                    };
                    let latest = |f: fn(&aws_sdk_ecr::types::ImageDetail) -> Option<&aws_smithy_types::DateTime>| {
                        images.iter().filter_map(|i| f(i)).max_by_key(|d| d.secs())
                    };
                    r.attr("image_count", Some(json!(images.len())))
                        .attr(
                            "untagged_images",
                            Some(json!(images.iter().filter(|i| loose(i)).count())),
                        )
                        .attr("multi_arch", Some(Value::Bool(multi_arch)))
                        .attr("size_gib", gib(size(false)))
                        .attr("untagged_size_gib", gib(size(true)))
                        .attr("last_pushed", time(latest(|i| i.image_pushed_at())))
                        .attr("last_pulled", time(latest(|i| i.last_recorded_pull_time())));
                }
                Err(e) => out
                    .warnings
                    .push(format!("ecr in {}: images of {name}: {}", ctx.region, aws_err(e))),
            }
            let lifecycle = match client.get_lifecycle_policy().repository_name(name).send().await {
                Ok(_) => Some(true),
                Err(e)
                    if e.as_service_error()
                        .is_some_and(|e| e.is_lifecycle_policy_not_found_exception()) =>
                {
                    Some(false)
                }
                Err(e) => {
                    out.warnings.push(format!(
                        "ecr in {}: lifecycle policy of {name}: {}",
                        ctx.region,
                        aws_err(e)
                    ));
                    None
                }
            };
            r.attr("lifecycle_policy", lifecycle.map(Value::Bool));
            out.resources.push(r);
        }
    }
    Ok(out)
}

pub async fn log_groups(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_cloudwatchlogs::Client::new(ctx.config);
    let mut out = Collected::default();
    for page in client
        .describe_log_groups()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for g in page.log_groups() {
            let Some(name) = g.log_group_name() else { continue };
            // `arn` ends in `:*`; the tagging API uses the plain ARN.
            let me = g
                .log_group_arn()
                .map(str::to_owned)
                .or_else(|| g.arn().map(|a| a.trim_end_matches(":*").to_owned()))
                .unwrap_or_else(|| {
                    format!(
                        "arn:{}:logs:{}:{}:log-group:{name}",
                        arn::partition(ctx.region),
                        ctx.region,
                        ctx.account
                    )
                });
            let mut r = Resource::new(T::LogGroup, &me, ctx.account, ctx.region, name);
            r.name = Some(name.to_owned());
            r.attr("retention_days", g.retention_in_days().map(|d| json!(d)))
                .attr("stored_gib", g.stored_bytes().and_then(gib))
                .attr("class", s(g.log_group_class().map(|c| c.as_str())))
                .attr("created", millis(g.creation_time()));
            // Lambda writes to /aws/lambda/<function>.
            if let Some(function) = name.strip_prefix("/aws/lambda/") {
                out.relate(
                    &format!(
                        "arn:{}:lambda:{}:{}:function:{function}",
                        arn::partition(ctx.region),
                        ctx.region,
                        ctx.account
                    ),
                    me.clone(),
                    K::PublishesTo,
                );
            }
            out.resources.push(r);
        }
    }
    Ok(out)
}

pub async fn efs(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_efs::Client::new(ctx.config);
    let mut out = Collected::default();
    for page in client
        .describe_file_systems()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for fs in page.file_systems() {
            let id = fs.file_system_id();
            let me = fs.file_system_arn().map_or_else(
                || {
                    format!(
                        "arn:{}:elasticfilesystem:{}:{}:file-system/{id}",
                        arn::partition(ctx.region),
                        ctx.region,
                        ctx.account
                    )
                },
                str::to_owned,
            );
            let mut r = Resource::new(T::EfsFileSystem, me, ctx.account, ctx.region, id);
            r.with_tags(fs.tags().iter().map(|t| (Some(t.key()), Some(t.value()))));
            if r.name.is_none() {
                r.name = fs.name().map(str::to_owned);
            }
            let size = fs.size_in_bytes();
            r.attr("state", s(Some(fs.life_cycle_state().as_str())))
                .attr("mount_targets", Some(json!(fs.number_of_mount_targets())))
                .attr("size_gib", size.and_then(|z| gib(z.value())))
                .attr("standard_gib", size.and_then(|z| z.value_in_standard()).and_then(gib))
                .attr("ia_gib", size.and_then(|z| z.value_in_ia()).and_then(gib))
                .attr("archive_gib", size.and_then(|z| z.value_in_archive()).and_then(gib))
                .attr("throughput_mode", s(fs.throughput_mode().map(|m| m.as_str())))
                .attr(
                    "provisioned_mibps",
                    fs.provisioned_throughput_in_mibps().map(|v| json!(v)),
                )
                .attr("one_zone", Some(Value::Bool(fs.availability_zone_name().is_some())))
                // The destination of an EFS replication: no mount targets by design.
                .attr(
                    "replication_destination",
                    Some(Value::Bool(
                        fs.file_system_protection()
                            .and_then(|p| p.replication_overwrite_protection())
                            == Some(&aws_sdk_efs::types::ReplicationOverwriteProtection::Replicating),
                    )),
                )
                .attr("created", time(Some(fs.creation_time())));
            out.resources.push(r);
        }
    }
    Ok(out)
}

/// Hosted zones are global; `ctx` is bound to the home region.
pub async fn route53(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_route53::Client::new(ctx.config);
    let mut out = Collected::default();
    for page in client
        .list_hosted_zones()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for z in page.hosted_zones() {
            let id = z.id().trim_start_matches("/hostedzone/");
            let me = format!("arn:{}:route53:::hostedzone/{id}", arn::partition(ctx.region));
            let mut r = Resource::new(T::Route53HostedZone, me, ctx.account, "global", id);
            r.name = Some(z.name().trim_end_matches('.').to_owned());
            r.attr("record_sets", z.resource_record_set_count().map(|n| json!(n)))
                .attr(
                    "private",
                    z.config().map(|c| Value::Bool(c.private_zone())),
                )
                .attr("linked_service", s(z.linked_service().and_then(|l| l.service_principal())));
            out.resources.push(r);
        }
    }
    Ok(out)
}
