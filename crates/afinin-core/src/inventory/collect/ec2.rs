// SPDX-License-Identifier: MIT OR Apache-2.0

//! EC2: instances, volumes, network interfaces, VPCs, subnets, security
//! groups, NAT gateways and Elastic IPs.

use aws_sdk_ec2::Client;
use aws_sdk_ec2::types::Tag;
use serde_json::{Value, json};

use super::{Ctx, aws_err, num, s, time};
use crate::error::Result;
use crate::inventory::{Collected, RelationKind as K, Resource, ResourceType as T, arn};

fn tags(t: &[Tag]) -> impl Iterator<Item = (Option<&str>, Option<&str>)> {
    t.iter().map(|t| (t.key(), t.value()))
}

pub async fn collect(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = Client::new(ctx.config);
    let (acct, region) = (ctx.account, ctx.region);
    let a = |kind: &str, id: &str| arn::ec2(region, acct, kind, id);
    let mut out = Collected::default();

    for page in client
        .describe_instances()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for i in page.reservations().iter().flat_map(|r| r.instances()) {
            let Some(id) = i.instance_id() else { continue };
            let me = a("instance", id);
            let state = i
                .state()
                .and_then(|s| s.name())
                .map(|n| n.as_str().to_owned());
            if state.as_deref() == Some("terminated") {
                continue;
            }
            let mut r = Resource::new(T::Ec2Instance, &me, acct, region, id);
            r.with_tags(tags(i.tags()))
                .attr("state", state.map(Value::String))
                .attr("instance_type", s(i.instance_type().map(|t| t.as_str())))
                .attr("private_ip", s(i.private_ip_address()))
                .attr("public_ip", s(i.public_ip_address()))
                .attr("vpc_id", s(i.vpc_id()))
                .attr("subnet_id", s(i.subnet_id()))
                .attr(
                    "availability_zone",
                    s(i.placement().and_then(|p| p.availability_zone())),
                )
                .attr("platform", s(i.platform_details()))
                .attr("state_reason", s(i.state_transition_reason()))
                .attr("launch_time", time(i.launch_time()));
            if let Some(subnet) = i.subnet_id() {
                out.relate(&me, a("subnet", subnet), K::InNetwork);
            }
            for sg in i.security_groups().iter().filter_map(|g| g.group_id()) {
                out.relate(&me, a("security-group", sg), K::SecuredBy);
            }
            for vol in i
                .block_device_mappings()
                .iter()
                .filter_map(|b| b.ebs()?.volume_id())
            {
                out.relate(&me, a("volume", vol), K::Attached);
            }
            out.resources.push(r);
        }
    }

    for page in client
        .describe_volumes()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for v in page.volumes() {
            let Some(id) = v.volume_id() else { continue };
            let me = a("volume", id);
            let attached: Vec<&str> = v
                .attachments()
                .iter()
                .filter_map(|x| x.instance_id())
                .collect();
            let mut r = Resource::new(T::EbsVolume, &me, acct, region, id);
            r.with_tags(tags(v.tags()))
                .attr("state", s(v.state().map(|x| x.as_str())))
                .attr("volume_type", s(v.volume_type().map(|x| x.as_str())))
                .attr("size_gib", v.size().and_then(num))
                .attr("iops", v.iops().and_then(num))
                .attr("throughput_mibps", v.throughput().and_then(num))
                .attr("snapshot_id", s(v.snapshot_id()))
                .attr("availability_zone", s(v.availability_zone()))
                .attr("attached_instances", Some(json!(attached)))
                .attr("create_time", time(v.create_time()));
            for inst in attached {
                out.relate(&a("instance", inst), &me, K::Attached);
            }
            out.resources.push(r);
        }
    }

    for page in client
        .describe_vpcs()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for v in page.vpcs() {
            let Some(id) = v.vpc_id() else { continue };
            let mut r = Resource::new(T::Vpc, a("vpc", id), acct, region, id);
            r.with_tags(tags(v.tags()))
                .attr("vpc_id", s(Some(id)))
                .attr("cidr_block", s(v.cidr_block()))
                .attr("is_default", v.is_default().map(Value::Bool));
            out.resources.push(r);
        }
    }

    for page in client
        .describe_subnets()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for sn in page.subnets() {
            let Some(id) = sn.subnet_id() else { continue };
            let me = a("subnet", id);
            let mut r = Resource::new(T::Subnet, &me, acct, region, id);
            r.with_tags(tags(sn.tags()))
                .attr("vpc_id", s(sn.vpc_id()))
                .attr("cidr_block", s(sn.cidr_block()))
                .attr("availability_zone", s(sn.availability_zone()));
            if let Some(vpc) = sn.vpc_id() {
                out.relate(&me, a("vpc", vpc), K::InNetwork);
            }
            out.resources.push(r);
        }
    }

    for page in client
        .describe_security_groups()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for g in page.security_groups() {
            let Some(id) = g.group_id() else { continue };
            let me = a("security-group", id);
            let mut r = Resource::new(T::SecurityGroup, &me, acct, region, id);
            r.name = g.group_name().map(str::to_owned);
            r.with_tags(tags(g.tags()))
                .attr("vpc_id", s(g.vpc_id()))
                .attr("description", s(g.description()));
            if let Some(vpc) = g.vpc_id() {
                out.relate(&me, a("vpc", vpc), K::InNetwork);
            }
            out.resources.push(r);
        }
    }

    for page in client
        .describe_nat_gateways()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for n in page.nat_gateways() {
            let Some(id) = n.nat_gateway_id() else {
                continue;
            };
            let state = n.state().map(|x| x.as_str().to_owned());
            if matches!(state.as_deref(), Some("deleted") | Some("deleting")) {
                continue;
            }
            let me = a("natgateway", id);
            let mut r = Resource::new(T::NatGateway, &me, acct, region, id);
            r.with_tags(tags(n.tags()))
                .attr("state", state.map(Value::String))
                .attr("vpc_id", s(n.vpc_id()))
                .attr("subnet_id", s(n.subnet_id()))
                .attr(
                    "connectivity_type",
                    s(n.connectivity_type().map(|x| x.as_str())),
                )
                .attr("create_time", time(n.create_time()));
            if let Some(subnet) = n.subnet_id() {
                out.relate(&me, a("subnet", subnet), K::InNetwork);
            }
            for addr in n.nat_gateway_addresses() {
                if let Some(alloc) = addr.allocation_id() {
                    out.relate(&me, a("elastic-ip", alloc), K::Attached);
                }
                if let Some(eni) = addr.network_interface_id() {
                    out.relate(&me, a("network-interface", eni), K::Owns);
                }
            }
            out.resources.push(r);
        }
    }

    let addresses = client.describe_addresses().send().await.map_err(aws_err)?;
    for e in addresses.addresses() {
        let Some(id) = e.allocation_id() else {
            continue;
        };
        let me = a("elastic-ip", id);
        let mut r = Resource::new(T::ElasticIp, &me, acct, region, id);
        r.name = e.public_ip().map(str::to_owned);
        r.with_tags(tags(e.tags()))
            .attr("public_ip", s(e.public_ip()))
            .attr(
                "associated",
                Some(Value::Bool(e.association_id().is_some())),
            )
            .attr("instance_id", s(e.instance_id()))
            .attr("network_interface_id", s(e.network_interface_id()));
        if let Some(inst) = e.instance_id() {
            out.relate(&a("instance", inst), &me, K::Attached);
        }
        out.resources.push(r);
    }

    for page in client
        .describe_network_interfaces()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for n in page.network_interfaces() {
            let Some(id) = n.network_interface_id() else {
                continue;
            };
            let me = a("network-interface", id);
            let mut r = Resource::new(T::NetworkInterface, &me, acct, region, id);
            r.with_tags(tags(n.tag_set()))
                .attr("vpc_id", s(n.vpc_id()))
                .attr("subnet_id", s(n.subnet_id()))
                .attr("private_ip", s(n.private_ip_address()))
                .attr("interface_type", s(n.interface_type().map(|x| x.as_str())))
                .attr("description", s(n.description()))
                .attr("status", s(n.status().map(|x| x.as_str())));
            if let Some(inst) = n.attachment().and_then(|x| x.instance_id()) {
                out.relate(&a("instance", inst), &me, K::Owns);
            }
            if let Some(subnet) = n.subnet_id() {
                out.relate(&me, a("subnet", subnet), K::InNetwork);
            }
            for sg in n.groups().iter().filter_map(|g| g.group_id()) {
                out.relate(&me, a("security-group", sg), K::SecuredBy);
            }
            out.resources.push(r);
        }
    }

    // Snapshots owned by this account (FR-31: old snapshots).
    for page in client
        .describe_snapshots()
        .owner_ids("self")
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for snap in page.snapshots() {
            let Some(id) = snap.snapshot_id() else {
                continue;
            };
            let me = a("snapshot", id);
            let mut r = Resource::new(T::EbsSnapshot, &me, acct, region, id);
            r.with_tags(tags(snap.tags()))
                .attr("state", s(snap.state().map(|x| x.as_str())))
                .attr("size_gib", snap.volume_size().and_then(num))
                .attr("volume_id", s(snap.volume_id()))
                .attr("storage_tier", s(snap.storage_tier().map(|x| x.as_str())))
                .attr("description", s(snap.description()))
                .attr("create_time", time(snap.start_time()));
            if let Some(vol) = snap.volume_id() {
                out.relate(&me, a("volume", vol), K::Attached);
            }
            out.resources.push(r);
        }
    }

    Ok(out)
}
