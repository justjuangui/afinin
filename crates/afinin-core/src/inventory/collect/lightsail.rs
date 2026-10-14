// SPDX-License-Identifier: MIT OR Apache-2.0

//! Lightsail: instances, managed databases, load balancers, block storage
//! disks and static IPs. Lightsail has its own API (not EC2) and runs in a
//! subset of regions, so callers scan only [`regions`].

use std::collections::HashMap;

use aws_sdk_lightsail::Client;
use aws_sdk_lightsail::types::Tag;
use serde_json::Value;

use super::{Ctx, aws_err, num, s, time};
use crate::error::Result;
use crate::inventory::{Collected, RelationKind as K, Resource, ResourceType as T};

fn tags(t: &[Tag]) -> impl Iterator<Item = (Option<&str>, Option<&str>)> {
    t.iter().map(|t| (t.key(), t.value()))
}

/// Follows `nextPageToken` (the Lightsail SDK has no paginators).
macro_rules! pages {
    ($request:expr, $items:ident) => {{
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page = $request
                .set_page_token(token.take())
                .send()
                .await
                .map_err(aws_err)?;
            all.extend(page.$items().iter().cloned());
            match page.next_page_token() {
                Some(t) if !t.is_empty() => token = Some(t.to_owned()),
                _ => break,
            }
        }
        all
    }};
}

/// Regions where Lightsail is available. `config` may be bound to any of them.
pub async fn regions(config: &aws_config::SdkConfig) -> Result<Vec<String>> {
    let out = Client::new(config)
        .get_regions()
        .send()
        .await
        .map_err(aws_err)?;
    Ok(out
        .regions()
        .iter()
        .filter_map(|r| r.name().map(|n| n.as_str().to_owned()))
        .collect())
}

/// Monthly list price (USD) per bundle ID, for instances and databases.
/// Without CUR this is the best per-resource figure: the alternative is the
/// whole Lightsail bill repeated on every resource.
async fn bundle_prices(client: &Client) -> Result<HashMap<String, f64>> {
    let mut prices = HashMap::new();
    for b in pages!(client.get_bundles().include_inactive(true), bundles) {
        if let (Some(id), Some(p)) = (b.bundle_id(), b.price()) {
            prices.insert(id.to_owned(), f64::from(p));
        }
    }
    for b in pages!(
        client
            .get_relational_database_bundles()
            .include_inactive(true),
        bundles
    ) {
        if let (Some(id), Some(p)) = (b.bundle_id(), b.price()) {
            prices.insert(id.to_owned(), f64::from(p));
        }
    }
    Ok(prices)
}

/// A price as a JSON number, rounded to cents (f32 prices carry noise).
fn price(p: f64) -> Option<Value> {
    serde_json::Number::from_f64((p * 100.0).round() / 100.0).map(Value::Number)
}

pub async fn collect(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = Client::new(ctx.config);
    let (acct, region) = (ctx.account, ctx.region);
    let mut out = Collected::default();
    // Prices are a nicety: a denied GetBundles must not hide the resources.
    let prices = match bundle_prices(&client).await {
        Ok(p) => p,
        Err(e) => {
            out.warnings
                .push(format!("lightsail prices in {region}: {e}"));
            HashMap::new()
        }
    };
    let bundle_price = |id: Option<&str>| id.and_then(|id| prices.get(id)).copied().and_then(price);

    // Lightsail links resources by name; relations need ARNs.
    let instances = pages!(client.get_instances(), instances);
    let by_name: HashMap<&str, &str> = instances
        .iter()
        .filter_map(|i| Some((i.name()?, i.arn()?)))
        .collect();

    for i in &instances {
        let (Some(name), Some(me)) = (i.name(), i.arn()) else {
            continue;
        };
        let mut r = Resource::new(T::LightsailInstance, me, acct, region, name);
        r.name = Some(name.to_owned());
        r.with_tags(tags(i.tags()))
            .attr("state", s(i.state().and_then(|st| st.name())))
            .attr("bundle_id", s(i.bundle_id()))
            .attr("monthly_price_usd", bundle_price(i.bundle_id()))
            .attr("blueprint", s(i.blueprint_name().or(i.blueprint_id())))
            .attr(
                "cpu_count",
                i.hardware().and_then(|h| h.cpu_count()).and_then(num),
            )
            .attr(
                "ram_gib",
                i.hardware()
                    .and_then(|h| h.ram_size_in_gb())
                    .and_then(|g| serde_json::Number::from_f64(g.into()))
                    .map(Value::Number),
            )
            .attr("private_ip", s(i.private_ip_address()))
            .attr("public_ip", s(i.public_ip_address()))
            .attr(
                "availability_zone",
                s(i.location().and_then(|l| l.availability_zone())),
            )
            .attr("create_time", time(i.created_at()));
        out.resources.push(r);
    }

    for db in pages!(client.get_relational_databases(), relational_databases) {
        let (Some(name), Some(me)) = (db.name(), db.arn()) else {
            continue;
        };
        let endpoint = db.master_endpoint();
        let mut r = Resource::new(T::LightsailDatabase, me, acct, region, name);
        r.name = Some(name.to_owned());
        r.with_tags(tags(db.tags()))
            .attr("state", s(db.state()))
            .attr("engine", s(db.engine()))
            .attr("engine_version", s(db.engine_version()))
            .attr("bundle_id", s(db.relational_database_bundle_id()))
            .attr(
                "monthly_price_usd",
                bundle_price(db.relational_database_bundle_id()),
            )
            .attr("dns_name", s(endpoint.and_then(|e| e.address())))
            .attr("port", endpoint.and_then(|e| e.port()).and_then(num))
            .attr(
                "publicly_accessible",
                db.publicly_accessible().map(Value::Bool),
            )
            .attr("create_time", time(db.created_at()));
        out.resources.push(r);
    }

    for lb in pages!(client.get_load_balancers(), load_balancers) {
        let (Some(name), Some(me)) = (lb.name(), lb.arn()) else {
            continue;
        };
        let mut r = Resource::new(T::LightsailLoadBalancer, me, acct, region, name);
        r.name = Some(name.to_owned());
        r.with_tags(tags(lb.tags()))
            .attr("state", s(lb.state().map(|x| x.as_str())))
            .attr("protocol", s(lb.protocol().map(|x| x.as_str())))
            .attr("dns_name", s(lb.dns_name()))
            .attr("instance_port", lb.instance_port().and_then(num))
            .attr("create_time", time(lb.created_at()));
        for target in lb.instance_health_summary() {
            if let Some(inst) = target.instance_name().and_then(|n| by_name.get(n)) {
                out.relate(me, *inst, K::RoutesTo);
            }
        }
        out.resources.push(r);
    }

    for d in pages!(client.get_disks(), disks) {
        let (Some(name), Some(me)) = (d.name(), d.arn()) else {
            continue;
        };
        let mut r = Resource::new(T::LightsailDisk, me, acct, region, name);
        r.name = Some(name.to_owned());
        r.with_tags(tags(d.tags()))
            .attr("state", s(d.state().map(|x| x.as_str())))
            .attr("size_gib", d.size_in_gb().and_then(num))
            .attr("attached_to", s(d.attached_to()))
            .attr(
                "availability_zone",
                s(d.location().and_then(|l| l.availability_zone())),
            )
            .attr("create_time", time(d.created_at()));
        if let Some(inst) = d.attached_to().and_then(|n| by_name.get(n)) {
            out.relate(inst, me, K::Attached);
        }
        out.resources.push(r);
    }

    for ip in pages!(client.get_static_ips(), static_ips) {
        let (Some(name), Some(me)) = (ip.name(), ip.arn()) else {
            continue;
        };
        let mut r = Resource::new(T::LightsailStaticIp, me, acct, region, name);
        r.name = ip.ip_address().map(str::to_owned);
        r.attr("public_ip", s(ip.ip_address()))
            .attr("attached_to", s(ip.attached_to()))
            .attr("create_time", time(ip.created_at()));
        // Lightsail doesn't charge for a static IP attached to an instance.
        if ip.is_attached() == Some(true) {
            r.attr("monthly_price_usd", price(0.0));
        }
        if let Some(inst) = ip.attached_to().and_then(|n| by_name.get(n)) {
            out.relate(inst, me, K::Attached);
        }
        out.resources.push(r);
    }

    Ok(out)
}
