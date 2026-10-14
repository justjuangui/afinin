// SPDX-License-Identifier: MIT OR Apache-2.0

//! ECS clusters and services, EKS clusters.

use serde_json::json;

use super::{Ctx, aws_err, s, time};
use crate::error::Result;
use crate::inventory::{Collected, RelationKind as K, Resource, ResourceType as T, arn};

pub async fn ecs(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_ecs::Client::new(ctx.config);
    let (acct, region) = (ctx.account, ctx.region);
    let mut out = Collected::default();

    let mut cluster_arns = Vec::new();
    for page in client
        .list_clusters()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        cluster_arns.extend(page.cluster_arns().iter().cloned());
    }
    for chunk in cluster_arns.chunks(100) {
        let desc = client
            .describe_clusters()
            .set_clusters(Some(chunk.to_vec()))
            .send()
            .await
            .map_err(aws_err)?;
        for c in desc.clusters() {
            let (Some(me), Some(name)) = (c.cluster_arn(), c.cluster_name()) else {
                continue;
            };
            let mut r = Resource::new(T::EcsCluster, me, acct, region, name);
            r.name = Some(name.to_owned());
            r.attr("status", s(c.status()))
                .attr("running_tasks", Some(json!(c.running_tasks_count())))
                .attr("active_services", Some(json!(c.active_services_count())))
                .attr(
                    "container_instances",
                    Some(json!(c.registered_container_instances_count())),
                );
            out.resources.push(r);
        }
    }

    for cluster in &cluster_arns {
        let mut service_arns = Vec::new();
        for page in client
            .list_services()
            .cluster(cluster)
            .into_paginator()
            .send()
            .try_collect()
            .await
            .map_err(aws_err)?
        {
            service_arns.extend(page.service_arns().iter().cloned());
        }
        for chunk in service_arns.chunks(10) {
            let desc = client
                .describe_services()
                .cluster(cluster)
                .set_services(Some(chunk.to_vec()))
                .send()
                .await
                .map_err(aws_err)?;
            for svc in desc.services() {
                let (Some(me), Some(name)) = (svc.service_arn(), svc.service_name()) else {
                    continue;
                };
                let net = svc
                    .network_configuration()
                    .and_then(|n| n.awsvpc_configuration());
                let mut r = Resource::new(T::EcsService, me, acct, region, name);
                r.name = Some(name.to_owned());
                r.attr("status", s(svc.status()))
                    .attr("launch_type", s(svc.launch_type().map(|l| l.as_str())))
                    .attr("desired_count", Some(json!(svc.desired_count())))
                    .attr("running_count", Some(json!(svc.running_count())))
                    .attr("task_definition", s(svc.task_definition()))
                    .attr("create_time", time(svc.created_at()));
                out.relate(me, cluster.as_str(), K::MemberOf);
                for tg in svc
                    .load_balancers()
                    .iter()
                    .filter_map(|l| l.target_group_arn())
                {
                    out.relate(tg, me, K::RoutesTo);
                }
                if let Some(n) = net {
                    for subnet in n.subnets() {
                        out.relate(me, arn::ec2(region, acct, "subnet", subnet), K::InNetwork);
                    }
                    for sg in n.security_groups() {
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
    }
    Ok(out)
}

pub async fn eks(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_eks::Client::new(ctx.config);
    let (acct, region) = (ctx.account, ctx.region);
    let mut out = Collected::default();
    for page in client
        .list_clusters()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for name in page.clusters() {
            let desc = client
                .describe_cluster()
                .name(name)
                .send()
                .await
                .map_err(aws_err)?;
            let Some(c) = desc.cluster() else { continue };
            let Some(me) = c.arn() else { continue };
            let vpc = c.resources_vpc_config();
            let mut r = Resource::new(T::EksCluster, me, acct, region, name);
            r.name = Some(name.clone());
            r.with_tags(
                c.tags()
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| (Some(k.as_str()), Some(v.as_str()))),
            )
            .attr("version", s(c.version()))
            .attr("status", s(c.status().map(|x| x.as_str())))
            .attr("vpc_id", s(vpc.and_then(|v| v.vpc_id())))
            .attr("create_time", time(c.created_at()));
            if let Some(v) = vpc {
                for subnet in v.subnet_ids() {
                    out.relate(me, arn::ec2(region, acct, "subnet", subnet), K::InNetwork);
                }
                for sg in v
                    .security_group_ids()
                    .iter()
                    .map(String::as_str)
                    .chain(v.cluster_security_group_id())
                {
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
    Ok(out)
}
