// SPDX-License-Identifier: MIT OR Apache-2.0

//! Per-service resource collectors. Each runs for one account and region and
//! reports failures as warnings, so one denied API never hides the rest.

mod apigateway;
pub mod cloudformation;
mod cloudfront;
mod containers;
mod data;
mod ec2;
mod elb;
mod lambda;
pub mod lightsail;
mod messaging;
mod tagging;

use aws_config::SdkConfig;
use aws_sdk_ec2::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_ec2::primitives::{DateTime, DateTimeFormat};
use serde_json::Value;

use super::Collected;
use crate::error::{Error, Result};

/// Region whose endpoints serve the global APIs (S3 bucket listing, CloudFront).
pub const HOME_REGION: &str = "us-east-1";

pub struct Ctx<'a> {
    /// SDK configuration already bound to `region`.
    pub config: &'a SdkConfig,
    pub account: &'a str,
    pub region: &'a str,
}

/// Concise `Code: message` for collector warnings.
pub(crate) fn aws_err<E, R>(err: SdkError<E, R>) -> Error
where
    E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
    R: std::fmt::Debug + Send + Sync + 'static,
{
    match (err.code(), err.message()) {
        (Some(code), Some(msg)) => Error::Aws(format!("{code}: {msg}")),
        (Some(code), None) => Error::Aws(code.to_owned()),
        _ => Error::aws(err),
    }
}

pub(crate) fn s(v: Option<&str>) -> Option<Value> {
    v.filter(|v| !v.is_empty())
        .map(|v| Value::String(v.to_owned()))
}

pub(crate) fn time(v: Option<&DateTime>) -> Option<Value> {
    v.and_then(|d| d.fmt(DateTimeFormat::DateTime).ok())
        .map(Value::String)
}

pub(crate) fn num(v: impl Into<serde_json::Number>) -> Option<Value> {
    Some(Value::Number(v.into()))
}

async fn guarded(
    service: &str,
    region: &str,
    f: impl Future<Output = Result<Collected>>,
) -> Collected {
    match f.await {
        Ok(c) => c,
        Err(e) => Collected {
            warnings: vec![format!("{service} in {region}: {e}")],
            ..Default::default()
        },
    }
}

/// Runs every collector for one region, concurrently. `lightsail` is false in
/// regions without Lightsail, whose endpoints do not exist.
pub async fn collect_region(ctx: &Ctx<'_>, lightsail: bool) -> Collected {
    let r = ctx.region;
    let (a, b, c, d, e, f, g, h) = futures::join!(
        guarded("ec2", r, ec2::collect(ctx)),
        guarded("elasticloadbalancing", r, elb::collect(ctx)),
        guarded("lambda", r, lambda::collect(ctx)),
        guarded("rds", r, data::rds(ctx)),
        guarded("dynamodb", r, data::dynamodb(ctx)),
        guarded("elasticache", r, data::elasticache(ctx)),
        guarded("ecs", r, containers::ecs(ctx)),
        guarded("eks", r, containers::eks(ctx)),
    );
    let (i, j, k, l, m, n, o) = futures::join!(
        guarded("sqs", r, messaging::sqs(ctx)),
        guarded("sns", r, messaging::sns(ctx)),
        guarded("apigateway", r, apigateway::rest(ctx)),
        guarded("apigatewayv2", r, apigateway::http(ctx)),
        guarded("tagging", r, tagging::collect(ctx)),
        guarded("cloudformation", r, cloudformation::collect(ctx)),
        async {
            if lightsail {
                guarded("lightsail", r, lightsail::collect(ctx)).await
            } else {
                Collected::default()
            }
        },
    );
    let mut out = Collected::default();
    for part in [a, b, c, d, e, f, g, h, i, j, k, l, m, n, o] {
        out.extend(part);
    }
    out
}

/// Services with a single global API (S3 bucket listing, CloudFront).
/// `ctx` must be bound to [`HOME_REGION`].
pub async fn collect_global(ctx: &Ctx<'_>) -> Collected {
    let (s3, cf) = futures::join!(
        guarded("s3", ctx.region, data::s3(ctx)),
        guarded("cloudfront", ctx.region, cloudfront::collect(ctx)),
    );
    let mut out = s3;
    out.extend(cf);
    out
}

/// Regions enabled for the account (opt-in regions included only if enabled).
pub async fn enabled_regions(config: &SdkConfig) -> Result<Vec<String>> {
    let out = aws_sdk_ec2::Client::new(config)
        .describe_regions()
        .send()
        .await
        .map_err(aws_err)?;
    let mut regions: Vec<String> = out
        .regions()
        .iter()
        .filter_map(|r| r.region_name().map(str::to_owned))
        .collect();
    regions.sort();
    Ok(regions)
}
