// SPDX-License-Identifier: MIT OR Apache-2.0

//! Elastic Load Balancing v2: load balancers, target groups and their targets.

use aws_sdk_elasticloadbalancingv2::Client;
use serde_json::{Value, json};

use super::{Ctx, aws_err, s, time};
use crate::error::Result;
use crate::inventory::{Collected, Relation, RelationKind as K, Resource, ResourceType as T, arn};

pub async fn collect(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = Client::new(ctx.config);
    let (acct, region) = (ctx.account, ctx.region);
    let mut out = Collected::default();

    for page in client
        .describe_load_balancers()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for lb in page.load_balancers() {
            let Some(me) = lb.load_balancer_arn() else {
                continue;
            };
            let mut r = Resource::new(
                T::LoadBalancer,
                me,
                acct,
                region,
                lb.load_balancer_name().unwrap_or(me),
            );
            r.name = lb.load_balancer_name().map(str::to_owned);
            r.attr("type", s(lb.r#type().map(|t| t.as_str())))
                .attr("scheme", s(lb.scheme().map(|t| t.as_str())))
                .attr(
                    "state",
                    s(lb.state().and_then(|x| x.code()).map(|c| c.as_str())),
                )
                .attr("dns_name", s(lb.dns_name()))
                .attr("vpc_id", s(lb.vpc_id()))
                .attr("create_time", time(lb.created_time()));
            for subnet in lb.availability_zones().iter().filter_map(|z| z.subnet_id()) {
                out.relate(me, arn::ec2(region, acct, "subnet", subnet), K::InNetwork);
            }
            for sg in lb.security_groups() {
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
        .describe_target_groups()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for tg in page.target_groups() {
            let Some(me) = tg.target_group_arn() else {
                continue;
            };
            let mut r = Resource::new(
                T::TargetGroup,
                me,
                acct,
                region,
                tg.target_group_name().unwrap_or(me),
            );
            r.name = tg.target_group_name().map(str::to_owned);
            r.attr("target_type", s(tg.target_type().map(|t| t.as_str())))
                .attr("protocol", s(tg.protocol().map(|t| t.as_str())))
                .attr("port", tg.port().map(|p| json!(p)))
                .attr("vpc_id", s(tg.vpc_id()));
            for lb in tg.load_balancer_arns() {
                out.relate(lb, me, K::RoutesTo);
            }

            let health = client
                .describe_target_health()
                .target_group_arn(me)
                .send()
                .await
                .map_err(aws_err)?;
            let mut healthy = 0;
            let descriptions = health.target_health_descriptions();
            for d in descriptions {
                if d.target_health()
                    .and_then(|h| h.state())
                    .map(|s| s.as_str())
                    == Some("healthy")
                {
                    healthy += 1;
                }
                let Some(id) = d.target().and_then(|t| t.id()) else {
                    continue;
                };
                let target = if id.starts_with("arn:") {
                    id.to_owned()
                } else if id.starts_with("i-") {
                    arn::ec2(region, acct, "instance", id)
                } else {
                    // IP targets resolve to the network interface (or instance) holding that address.
                    format!("ip:{id}")
                };
                out.relations.push(Relation::new(me, target, K::RoutesTo));
            }
            r.attr("targets", Some(json!(descriptions.len())))
                .attr("healthy_targets", Some(json!(healthy)))
                .attr(
                    "has_load_balancer",
                    Some(Value::Bool(!tg.load_balancer_arns().is_empty())),
                );
            out.resources.push(r);
        }
    }

    Ok(out)
}
