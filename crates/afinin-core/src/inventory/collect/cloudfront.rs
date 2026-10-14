// SPDX-License-Identifier: MIT OR Apache-2.0

//! CloudFront distributions (global) and their origins.

use serde_json::{Value, json};

use super::{Ctx, aws_err, s};
use crate::error::Result;
use crate::inventory::{Collected, Relation, RelationKind as K, Resource, ResourceType as T, arn};

pub async fn collect(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_cloudfront::Client::new(ctx.config);
    let mut out = Collected::default();
    for page in client
        .list_distributions()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for d in page.distribution_list().into_iter().flat_map(|l| l.items()) {
            let me = d.arn();
            let mut r = Resource::new(T::CloudFrontDistribution, me, ctx.account, "global", d.id());
            r.name = d
                .aliases()
                .and_then(|a| a.items().first().cloned())
                .or_else(|| Some(d.domain_name().to_owned()));
            r.attr("status", s(Some(d.status())))
                .attr("enabled", Some(Value::Bool(d.enabled())))
                .attr("dns_name", s(Some(d.domain_name())))
                .attr("price_class", s(Some(d.price_class().as_str())));
            for origin in d.origins().map(|o| o.items()).unwrap_or_default() {
                let domain = origin.domain_name();
                match arn::s3_bucket_from_domain(domain) {
                    Some(bucket) => {
                        out.relate(me, arn::s3_bucket("us-east-1", bucket), K::RoutesTo)
                    }
                    None => out
                        .relations
                        .push(Relation::to_dns(me, domain, K::RoutesTo)),
                }
            }
            r.attr(
                "origins",
                Some(json!(d.origins().map(|o| o.items().len()).unwrap_or(0))),
            );
            out.resources.push(r);
        }
    }
    Ok(out)
}
