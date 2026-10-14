// SPDX-License-Identifier: MIT OR Apache-2.0

//! API Gateway REST (v1) and HTTP/WebSocket (v2) APIs and their integrations.

use serde_json::json;

use super::{Ctx, aws_err, s, time};
use crate::error::Result;
use crate::inventory::{Collected, Relation, RelationKind as K, Resource, ResourceType as T, arn};

/// Relationship for an integration target: Lambda, an ALB/NLB listener
/// (private integration) or an HTTP endpoint resolved by host name.
fn integration(out: &mut Collected, api: &str, uri: &str) {
    if let Some(f) = arn::lambda_from_integration_uri(uri) {
        out.relate(api, f, K::RoutesTo);
    } else if let Some(lb) = listener_to_load_balancer(uri) {
        out.relate(api, lb, K::RoutesTo);
    } else if let Some(host) = uri
        .split("://")
        .nth(1)
        .and_then(|r| r.split(['/', ':']).next())
    {
        out.relations.push(Relation::to_dns(api, host, K::RoutesTo));
    }
}

/// `…:listener/app/name/lbid/listenerid` → `…:loadbalancer/app/name/lbid`.
fn listener_to_load_balancer(uri: &str) -> Option<String> {
    if !uri.starts_with("arn:") || !uri.contains(":listener/") {
        return None;
    }
    let lb = uri.replacen(":listener/", ":loadbalancer/", 1);
    let cut = lb.rfind('/')?;
    Some(lb[..cut].to_owned())
}

pub async fn rest(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_apigateway::Client::new(ctx.config);
    let (acct, region) = (ctx.account, ctx.region);
    let mut out = Collected::default();
    for page in client
        .get_rest_apis()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for api in page.items() {
            let Some(id) = api.id() else { continue };
            let me = format!(
                "arn:{}:apigateway:{region}::/restapis/{id}",
                arn::partition(region)
            );
            let mut r = Resource::new(T::ApiGatewayRestApi, &me, acct, region, id);
            r.name = api.name().map(str::to_owned);
            r.with_tags(
                api.tags()
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| (Some(k.as_str()), Some(v.as_str()))),
            )
            .attr(
                "dns_name",
                Some(json!(format!("{id}.execute-api.{region}.amazonaws.com"))),
            )
            .attr(
                "endpoint_types",
                api.endpoint_configuration()
                    .map(|e| json!(e.types().iter().map(|t| t.as_str()).collect::<Vec<_>>())),
            )
            .attr("create_time", time(api.created_date()));
            let resources = client
                .get_resources()
                .rest_api_id(id)
                .embed("methods")
                .into_paginator()
                .send()
                .try_collect()
                .await
                .map_err(aws_err)?;
            for res in resources.iter().flat_map(|p| p.items()) {
                for method in res.resource_methods().into_iter().flat_map(|m| m.values()) {
                    if let Some(uri) = method.method_integration().and_then(|i| i.uri()) {
                        integration(&mut out, &me, uri);
                    }
                }
            }
            out.resources.push(r);
        }
    }
    Ok(out)
}

pub async fn http(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_apigatewayv2::Client::new(ctx.config);
    let (acct, region) = (ctx.account, ctx.region);
    let mut out = Collected::default();
    let mut token = None;
    loop {
        let page = client
            .get_apis()
            .set_next_token(token)
            .send()
            .await
            .map_err(aws_err)?;
        for api in page.items() {
            let Some(id) = api.api_id() else { continue };
            let me = format!(
                "arn:{}:apigateway:{region}::/apis/{id}",
                arn::partition(region)
            );
            let mut r = Resource::new(T::ApiGatewayHttpApi, &me, acct, region, id);
            r.name = api.name().map(str::to_owned);
            r.with_tags(
                api.tags()
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| (Some(k.as_str()), Some(v.as_str()))),
            )
            .attr("protocol", s(api.protocol_type().map(|p| p.as_str())))
            .attr(
                "dns_name",
                Some(json!(format!("{id}.execute-api.{region}.amazonaws.com"))),
            )
            .attr("create_time", time(api.created_date()));
            let mut itoken = None;
            loop {
                let ints = client
                    .get_integrations()
                    .api_id(id)
                    .set_next_token(itoken)
                    .send()
                    .await
                    .map_err(aws_err)?;
                for i in ints.items() {
                    if let Some(uri) = i.integration_uri() {
                        integration(&mut out, &me, uri);
                    }
                }
                itoken = ints.next_token().map(str::to_owned);
                if itoken.is_none() {
                    break;
                }
            }
            out.resources.push(r);
        }
        token = page.next_token().map(str::to_owned);
        if token.is_none() {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_maps_to_load_balancer() {
        assert_eq!(
            listener_to_load_balancer(
                "arn:aws:elasticloadbalancing:us-east-1:1:listener/app/web/abc/def"
            )
            .as_deref(),
            Some("arn:aws:elasticloadbalancing:us-east-1:1:loadbalancer/app/web/abc")
        );
    }
}
