// SPDX-License-Identifier: MIT OR Apache-2.0

//! SQS queues, SNS topics and subscriptions.

use super::{Ctx, aws_err, s};
use crate::error::Result;
use crate::inventory::{Collected, RelationKind as K, Resource, ResourceType as T, arn};

pub async fn sqs(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_sqs::Client::new(ctx.config);
    let mut out = Collected::default();
    for page in client
        .list_queues()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for url in page.queue_urls() {
            let Some(me) = arn::sqs_from_url(url) else {
                continue;
            };
            let name = url.rsplit('/').next().unwrap_or(url);
            let mut r = Resource::new(T::SqsQueue, me, ctx.account, ctx.region, name);
            r.name = Some(name.to_owned());
            r.attr("url", s(Some(url))).attr(
                "fifo",
                Some(serde_json::Value::Bool(name.ends_with(".fifo"))),
            );
            out.resources.push(r);
        }
    }
    Ok(out)
}

pub async fn sns(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_sns::Client::new(ctx.config);
    let mut out = Collected::default();
    for page in client
        .list_topics()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for t in page.topics() {
            let Some(me) = t.topic_arn() else { continue };
            let name = me.rsplit(':').next().unwrap_or(me);
            let mut r = Resource::new(T::SnsTopic, me, ctx.account, ctx.region, name);
            r.name = Some(name.to_owned());
            out.resources.push(r);
        }
    }
    for page in client
        .list_subscriptions()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for sub in page.subscriptions() {
            let (Some(topic), Some(endpoint), Some(protocol)) =
                (sub.topic_arn(), sub.endpoint(), sub.protocol())
            else {
                continue;
            };
            if matches!(protocol, "sqs" | "lambda" | "firehose") {
                out.relate(topic, arn::strip_lambda_qualifier(endpoint), K::PublishesTo);
            }
        }
    }
    Ok(out)
}
