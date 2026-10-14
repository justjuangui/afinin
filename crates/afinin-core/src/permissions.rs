// SPDX-License-Identifier: MIT OR Apache-2.0

//! Permission probes for "Test connection" (FR-3): one cheap, free read call
//! per capability, reporting which IAM actions are denied. Cost Explorer is
//! not probed because every request costs $0.01; the first sync reports it.

use aws_config::{Region, SdkConfig};
use aws_sdk_ec2::error::{ProvideErrorMetadata, SdkError};

const DENIED: &[&str] = &[
    "AccessDenied",
    "AccessDeniedException",
    "UnauthorizedOperation",
    "AuthorizationError",
    "UnauthorizedException",
    "Forbidden",
];

fn denied<E: ProvideErrorMetadata, R>(result: Result<impl Sized, SdkError<E, R>>) -> bool {
    match result {
        Ok(_) => false,
        Err(e) => e.code().is_some_and(|c| DENIED.contains(&c)),
    }
}

/// IAM actions the account is missing, by probing read-only calls.
pub async fn missing_permissions(config: &SdkConfig) -> Vec<&'static str> {
    // Probe in the profile's region, or us-east-1 when it has none.
    let config = if config.region().is_none() {
        config.to_builder().region(Region::new("us-east-1")).build()
    } else {
        config.clone()
    };
    let ec2 = aws_sdk_ec2::Client::new(&config);
    let elb = aws_sdk_elasticloadbalancingv2::Client::new(&config);
    let rds = aws_sdk_rds::Client::new(&config);
    let lambda = aws_sdk_lambda::Client::new(&config);
    let s3 = aws_sdk_s3::Client::new(&config);
    let tagging = aws_sdk_resourcegroupstagging::Client::new(&config);

    let (regions, instances, volumes, lbs, dbs, functions, buckets, tags) = futures::join!(
        ec2.describe_regions().send(),
        ec2.describe_instances().max_results(5).send(),
        ec2.describe_volumes().max_results(5).send(),
        elb.describe_load_balancers().page_size(1).send(),
        rds.describe_db_instances().max_records(20).send(),
        lambda.list_functions().max_items(1).send(),
        s3.list_buckets().max_buckets(1).send(),
        tagging.get_resources().resources_per_page(1).send(),
    );
    let mut missing = Vec::new();
    for (is_denied, action) in [
        (denied(regions), "ec2:DescribeRegions"),
        (denied(instances), "ec2:DescribeInstances"),
        (denied(volumes), "ec2:DescribeVolumes"),
        (denied(lbs), "elasticloadbalancing:DescribeLoadBalancers"),
        (denied(dbs), "rds:DescribeDBInstances"),
        (denied(functions), "lambda:ListFunctions"),
        (denied(buckets), "s3:ListAllMyBuckets"),
        (denied(tags), "tag:GetResources"),
    ] {
        if is_denied {
            missing.push(action);
        }
    }
    missing
}
