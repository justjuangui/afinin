// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tags through the Resource Groups Tagging API: one paginated call per region
//! covers services whose list APIs don't return tags (Lambda, SQS, SNS, ECS…).

use super::{Ctx, aws_err};
use crate::error::Result;
use crate::inventory::Collected;

pub async fn collect(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_resourcegroupstagging::Client::new(ctx.config);
    let mut out = Collected::default();
    for page in client
        .get_resources()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for m in page.resource_tag_mapping_list() {
            let Some(arn) = m.resource_arn() else {
                continue;
            };
            let tags = m
                .tags()
                .iter()
                .map(|t| (t.key().to_owned(), t.value().to_owned()))
                .collect();
            out.tags_by_arn.insert(arn.to_owned(), tags);
        }
    }
    Ok(out)
}
