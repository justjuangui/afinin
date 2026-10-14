// SPDX-License-Identifier: MIT OR Apache-2.0

//! Lambda functions and their event sources.

use aws_sdk_lambda::Client;
use serde_json::json;

use super::{Ctx, aws_err, s};
use crate::error::Result;
use crate::inventory::{Collected, RelationKind as K, Resource, ResourceType as T, arn};

pub async fn collect(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = Client::new(ctx.config);
    let (acct, region) = (ctx.account, ctx.region);
    let mut out = Collected::default();

    for page in client
        .list_functions()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for f in page.functions() {
            let (Some(me), Some(name)) = (f.function_arn(), f.function_name()) else {
                continue;
            };
            let vpc = f.vpc_config();
            let mut r = Resource::new(T::LambdaFunction, me, acct, region, name);
            r.name = Some(name.to_owned());
            r.attr("runtime", s(f.runtime().map(|x| x.as_str())))
                .attr("memory_mb", f.memory_size().map(|m| json!(m)))
                .attr("timeout_s", f.timeout().map(|t| json!(t)))
                .attr("code_size_bytes", Some(json!(f.code_size())))
                .attr(
                    "architectures",
                    Some(json!(
                        f.architectures()
                            .iter()
                            .map(|a| a.as_str())
                            .collect::<Vec<_>>()
                    )),
                )
                .attr("last_modified", s(f.last_modified()))
                .attr("vpc_id", s(vpc.and_then(|v| v.vpc_id())));
            if let Some(v) = vpc {
                for subnet in v.subnet_ids() {
                    out.relate(me, arn::ec2(region, acct, "subnet", subnet), K::InNetwork);
                }
                for sg in v.security_group_ids() {
                    out.relate(
                        me,
                        arn::ec2(region, acct, "security-group", sg),
                        K::SecuredBy,
                    );
                }
            }
            out.resources.push(r);
        }
    }

    for page in client
        .list_event_source_mappings()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for m in page.event_source_mappings() {
            let (Some(source), Some(function)) = (m.event_source_arn(), m.function_arn()) else {
                continue;
            };
            let source = if source.contains(":dynamodb:") {
                arn::dynamodb_table_from_stream(source)
            } else {
                source.to_owned()
            };
            out.relate(&source, arn::strip_lambda_qualifier(function), K::Triggers);
        }
    }

    Ok(out)
}
