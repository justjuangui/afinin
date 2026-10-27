// SPDX-License-Identifier: MIT OR Apache-2.0

//! Waste rules (FR-31, MVP set D-2), each with an estimated monthly saving.

use std::collections::{HashMap, HashSet};

use time::Date;

use super::pricing::{HOURS_PER_MONTH, PriceBook, PriceItem, PriceSource, builtin, platform_monthly};
use super::{Finding, FindingKind, FindingResource, SavingsBasis, Severity};
use crate::cost::dates;
use crate::inventory::graph::GraphEdge;
use crate::inventory::{RelationKind, Resource, ResourceType as T};

/// Everything the rules look at.
pub struct RuleInput<'a> {
    pub resources: &'a [Resource],
    pub edges: &'a [GraphEdge],
    pub prices: &'a PriceBook,
    /// Last-30-day net cost per resource ID/ARN, from CUR (empty without CUR).
    pub actual_costs: &'a HashMap<String, f64>,
    /// Last-30-day NAT gateway GB processed per resource ID (CUR), if known.
    pub nat_gb: Option<&'a HashMap<String, f64>>,
    /// Last-30-day NAT data processing cost per resource ID (CUR).
    pub nat_processing_cost: &'a HashMap<String, f64>,
    pub snapshot_age_days: u32,
    pub today: Date,
}

impl RuleInput<'_> {
    fn actual(&self, r: &Resource) -> Option<f64> {
        self.actual_costs
            .get(&r.arn)
            .or_else(|| self.actual_costs.get(&r.native_id))
            .copied()
            .filter(|c| *c > 0.0)
    }

    /// CUR actual cost when known, else the public-price estimate.
    fn savings(&self, r: &Resource, estimate: f64) -> (f64, SavingsBasis) {
        match self.actual(r) {
            Some(c) => (c, SavingsBasis::Actual),
            None => (estimate, SavingsBasis::PublicPrice),
        }
    }
}

fn str_attr<'a>(r: &'a Resource, key: &str) -> Option<&'a str> {
    r.attrs.get(key).and_then(|v| v.as_str())
}

fn num_attr(r: &Resource, key: &str) -> Option<f64> {
    r.attrs.get(key).and_then(|v| v.as_f64())
}

fn subject(r: &Resource) -> FindingResource {
    FindingResource {
        arn: r.arn.clone(),
        label: r.display_name().to_owned(),
        resource_type: Some(r.resource_type),
    }
}

fn base(rule: &'static str, r: &Resource) -> Finding {
    let mut f = Finding::new(rule, FindingKind::Waste, &r.arn);
    f.account_id = r.account_id.clone();
    f.region = r.region.clone();
    f.resources.push(subject(r));
    f
}

fn finish(
    mut f: Finding,
    savings: f64,
    basis: SavingsBasis,
    source: Option<PriceSource>,
) -> Finding {
    f.severity = Severity::from_amount(savings);
    f.monthly_savings = Some((savings * 100.0).round() / 100.0);
    f.savings_basis = Some(basis);
    if basis == SavingsBasis::PublicPrice {
        f = f.evidence(
            "price_source",
            match source {
                Some(PriceSource::PriceList) => "AWS Price List (public price)",
                Some(PriceSource::LightsailBundle) => "Lightsail bundle price (public price)",
                _ => "built-in us-east-1 list price (public price)",
            },
        );
    }
    f
}

/// Monthly public-price cost of an EBS volume, with the price source used.
pub fn volume_monthly_cost(prices: &PriceBook, v: &Resource) -> (f64, PriceSource) {
    let kind = str_attr(v, "volume_type").unwrap_or("gp2").to_owned();
    let size = num_attr(v, "size_gib").unwrap_or(0.0);
    let (storage, src) = prices.get(&v.region, &PriceItem::EbsStorage(kind.clone()));
    let mut cost = size * storage;
    let iops = num_attr(v, "iops").unwrap_or(0.0);
    match kind.as_str() {
        "gp3" => {
            cost += (iops - 3000.0).max(0.0)
                * prices.get(&v.region, &PriceItem::EbsIops(kind.clone())).0;
            let tp = num_attr(v, "throughput_mibps").unwrap_or(125.0);
            cost += (tp - 125.0).max(0.0) * prices.get(&v.region, &PriceItem::EbsThroughput).0;
        }
        "io1" | "io2" => cost += iops * prices.get(&v.region, &PriceItem::EbsIops(kind.clone())).0,
        _ => {}
    }
    (cost, src)
}

/// Lightsail block storage, USD per GB-month (us-east-1 list price; the
/// Lightsail API doesn't return disk prices).
const LIGHTSAIL_DISK_GB_MONTH: f64 = 0.10;

fn attached(r: &Resource) -> Option<bool> {
    r.attrs.get("attached").and_then(|v| v.as_bool())
}

/// Unused for this long, a secret, repository or key is worth a look.
const IDLE_DAYS: i64 = 90;

/// Log groups smaller than this without retention aren't worth a finding.
const LOGS_MIN_GIB: f64 = 1.0;

fn age_days(r: &Resource, key: &str, today: Date) -> Option<i64> {
    let created = str_attr(r, key)?;
    let date = dates::parse(created.get(..10)?).ok()?;
    Some((today - date).whole_days())
}

pub fn evaluate(input: &RuleInput) -> Vec<Finding> {
    let mut out = Vec::new();
    let by_arn: HashMap<&str, &Resource> = input
        .resources
        .iter()
        .map(|r| (r.arn.as_str(), r))
        .collect();

    for r in input.resources {
        match r.resource_type {
            T::EbsVolume if str_attr(r, "state") == Some("available") => {
                let (estimate, src) = volume_monthly_cost(input.prices, r);
                let (savings, basis) = input.savings(r, estimate);
                let mut f = base("ebs-unattached", r)
                    .evidence("size_gib", num_attr(r, "size_gib"))
                    .evidence("volume_type", str_attr(r, "volume_type"))
                    .evidence("age_days", age_days(r, "create_time", input.today));
                f.title = format!("Unattached EBS volume {}", r.display_name());
                f.summary = format!(
                    "{} GiB {} volume not attached to any instance. You pay for provisioned storage whether it's used or not.",
                    num_attr(r, "size_gib").unwrap_or(0.0),
                    str_attr(r, "volume_type").unwrap_or("EBS")
                );
                f.how_to_fix = "Check that no one needs the data. Create a snapshot if you might (snapshots cost less per GB), \
                                then delete the volume in the EC2 console (Volumes → Actions → Delete) or remove it from your IaC."
                    .into();
                out.push(finish(f, savings, basis, Some(src)));
            }
            T::ElasticIp if r.attrs.get("associated").and_then(|v| v.as_bool()) == Some(false) => {
                let (hourly, src) = input.prices.get(&r.region, &PriceItem::PublicIpv4Idle);
                let (savings, basis) = input.savings(r, hourly * HOURS_PER_MONTH);
                let mut f =
                    base("eip-unassociated", r).evidence("public_ip", str_attr(r, "public_ip"));
                f.title = format!("Unassociated Elastic IP {}", r.display_name());
                f.summary = "An allocated public IPv4 address not attached to anything is billed every hour.".into();
                f.how_to_fix =
                    "If nothing depends on this address (DNS records, allow-lists), release it: \
                                EC2 console → Elastic IPs → Actions → Release."
                        .into();
                out.push(finish(f, savings, basis, Some(src)));
            }
            T::LightsailStaticIp if attached(r) == Some(false) => {
                let (hourly, src) = input.prices.get(&r.region, &PriceItem::PublicIpv4Idle);
                let (savings, basis) = input.savings(r, hourly * HOURS_PER_MONTH);
                let mut f = base("lightsail-static-ip-unattached", r)
                    .evidence("public_ip", str_attr(r, "public_ip"));
                f.title = format!("Unattached Lightsail static IP {}", r.display_name());
                f.summary = "Lightsail static IPs are free only while attached to an instance; an unattached one is billed every hour.".into();
                f.how_to_fix = "If nothing depends on this address (DNS records, allow-lists), delete it: \
                                Lightsail console → Networking → the static IP → Delete. Or attach it to the instance that should use it."
                    .into();
                out.push(finish(f, savings, basis, Some(src)));
            }
            T::LightsailInstance if str_attr(r, "state") == Some("stopped") => {
                // Without a bundle price there is nothing honest to estimate.
                let Some(price) = num_attr(r, "monthly_price_usd") else {
                    continue;
                };
                let (savings, basis) = input.savings(r, price);
                let mut f = base("lightsail-instance-stopped", r)
                    .evidence("bundle", str_attr(r, "bundle_id"))
                    .evidence("monthly_list_price_usd", price);
                f.title = format!(
                    "Stopped Lightsail instance {} is still billed",
                    r.display_name()
                );
                f.summary = "Lightsail bills an instance's whole bundle for as long as it exists, running or stopped.".into();
                f.how_to_fix = "If it is no longer needed, create a snapshot (billed per GB, far less than the bundle) and delete \
                                the instance in the Lightsail console. To keep it, consider a smaller bundle."
                    .into();
                out.push(finish(
                    f,
                    savings,
                    basis,
                    Some(PriceSource::LightsailBundle),
                ));
            }
            T::LightsailDisk if attached(r) == Some(false) => {
                let size = num_attr(r, "size_gib").unwrap_or(0.0);
                let (savings, basis) = input.savings(r, size * LIGHTSAIL_DISK_GB_MONTH);
                let mut f = base("lightsail-disk-unattached", r).evidence("size_gib", size);
                f.title = format!("Unattached Lightsail disk {}", r.display_name());
                f.summary = "A Lightsail block storage disk not attached to any instance keeps billing for its full size.".into();
                f.how_to_fix = "Check that no one needs the data. Create a disk snapshot if you might, then delete the disk: \
                                Lightsail console → Storage → the disk → Delete."
                    .into();
                out.push(finish(f, savings, basis, Some(PriceSource::BuiltIn)));
            }
            // Never read in 90 days (or never, and older than that). Secrets
            // another service owns (RDS, Redshift…) are its to manage.
            T::SecretsManagerSecret
                if str_attr(r, "deleted").is_none()
                    && str_attr(r, "owning_service").is_none()
                    && match age_days(r, "last_accessed", input.today) {
                        Some(days) => days > IDLE_DAYS,
                        None => age_days(r, "created", input.today).is_some_and(|d| d > IDLE_DAYS),
                    } =>
            {
                let (savings, basis) = input.savings(r, builtin::SECRET);
                let mut f = base("secret-unused", r)
                    .evidence("last_accessed", str_attr(r, "last_accessed").unwrap_or("never"))
                    .evidence("created", str_attr(r, "created"));
                f.title = format!("Secret {} hasn't been read in {IDLE_DAYS}+ days", r.display_name());
                f.summary = "Each secret is billed every month whether anything reads it or not.".into();
                f.how_to_fix = "Check that no application, pipeline or person still needs it (search your code and \
                                configuration for its name). Then delete it in the Secrets Manager console: deletion \
                                waits 7–30 days, during which you can restore it."
                    .into();
                out.push(finish(f, savings, basis, Some(PriceSource::BuiltIn)));
            }
            // A disabled key can't be used but is still billed.
            T::KmsKey if str_attr(r, "state") == Some("Disabled") => {
                let (savings, basis) = input.savings(r, builtin::KMS_KEY);
                let mut f = base("kms-key-disabled", r).evidence("key_id", r.native_id.as_str());
                f.title = format!("Disabled KMS key {} is still billed", r.display_name());
                f.summary = "Customer-managed keys cost the same enabled or disabled.".into();
                f.how_to_fix = "Only if nothing encrypted with this key is still needed: data encrypted with a deleted key \
                                can never be decrypted. Check CloudTrail for recent use and which resources reference it, \
                                then schedule deletion in the KMS console (7–30 day waiting period, cancellable)."
                    .into();
                out.push(finish(f, savings, basis, Some(PriceSource::BuiltIn)));
            }
            // Nothing pulled or pushed in 90 days.
            T::EcrRepository
                if num_attr(r, "size_gib").is_some_and(|g| g >= 0.1)
                    && match age_days(r, "last_pulled", input.today) {
                        Some(days) => days > IDLE_DAYS,
                        None => age_days(r, "last_pushed", input.today).is_some_and(|d| d > IDLE_DAYS),
                    }
                    && age_days(r, "last_pushed", input.today).is_none_or(|d| d > IDLE_DAYS) =>
            {
                let gib = num_attr(r, "size_gib").unwrap_or(0.0);
                let (savings, basis) = input.savings(r, gib * builtin::ECR_GB);
                let mut f = base("ecr-repository-idle", r)
                    .evidence("last_pulled", str_attr(r, "last_pulled").unwrap_or("never recorded"))
                    .evidence("last_pushed", str_attr(r, "last_pushed"))
                    .evidence("size_gib", gib);
                f.title = format!("ECR repository {} hasn't been used in {IDLE_DAYS}+ days", r.display_name());
                f.summary = "No image was pulled or pushed recently, but all of them are still stored and billed. \
                             Shared layers are counted once per image, so the saving is an upper bound."
                    .into();
                f.how_to_fix = "If no deployment, cluster or pipeline uses it any more, delete the repository or its \
                                old images; otherwise add a lifecycle policy that keeps only recent images."
                    .into();
                out.push(finish(f, savings, basis, Some(PriceSource::BuiltIn)));
            }
            // Untagged images pile up without a lifecycle policy.
            T::EcrRepository
                if r.attrs.get("lifecycle_policy").and_then(|v| v.as_bool()) == Some(false)
                    && num_attr(r, "untagged_size_gib").is_some_and(|g| g >= 0.5) =>
            {
                let gib = num_attr(r, "untagged_size_gib").unwrap_or(0.0);
                let (savings, basis) = input.savings(r, gib * builtin::ECR_GB);
                let mut f = base("ecr-untagged-images", r)
                    .evidence("untagged_images", num_attr(r, "untagged_images"))
                    .evidence("untagged_size_gib", gib);
                f.title = format!("ECR repository {} keeps untagged images", r.display_name());
                f.summary = "Without a lifecycle policy, every superseded image stays stored and billed. The size \
                             counts layers shared between images more than once, so the saving is an upper bound."
                    .into();
                f.how_to_fix = "Add a lifecycle policy (ECR console → the repository → Lifecycle policy), e.g. expire \
                                untagged images after 14 days and keep the last 20 tagged ones."
                    .into();
                out.push(finish(f, savings, basis, Some(PriceSource::BuiltIn)));
            }
            // "Never expire" keeps every log event forever.
            T::LogGroup
                if !r.attrs.contains_key("retention_days")
                    && num_attr(r, "stored_gib").is_some_and(|g| g >= LOGS_MIN_GIB) =>
            {
                let gib = num_attr(r, "stored_gib").unwrap_or(0.0);
                let (savings, basis) = input.savings(r, gib * builtin::LOGS_GB);
                let mut f = base("logs-no-retention", r).evidence("stored_gib", gib);
                f.title = format!("Log group {} never expires", r.display_name());
                f.summary = "Without a retention period, every log event is stored forever and the bill grows each \
                             month. The saving shown is the whole storage cost, the most a retention period could save."
                    .into();
                f.how_to_fix = "Set a retention period (CloudWatch console → Log groups → the group → Actions → Edit \
                                retention), e.g. 30 days, or what your audit rules require. Export older logs to S3 first \
                                if you must keep them."
                    .into();
                out.push(finish(f, savings, basis, Some(PriceSource::BuiltIn)));
            }
            // No mount target: nothing can reach the data.
            T::EfsFileSystem
                if num_attr(r, "mount_targets") == Some(0.0) && str_attr(r, "state") == Some("available") =>
            {
                let (savings, basis) = input.savings(r, platform_monthly(r).unwrap_or(0.0));
                let mut f = base("efs-no-mount-targets", r).evidence("size_gib", num_attr(r, "size_gib"));
                f.title = format!("EFS file system {} has no mount targets", r.display_name());
                f.summary = "Without a mount target no instance, container or function can use it, but its storage \
                             is still billed."
                    .into();
                f.how_to_fix = "If the data is no longer needed, back it up with AWS Backup if in doubt, then delete the \
                                file system in the EFS console."
                    .into();
                out.push(finish(f, savings, basis, Some(PriceSource::BuiltIn)));
            }
            // Only the NS and SOA records every zone starts with.
            T::Route53HostedZone
                if num_attr(r, "record_sets").is_some_and(|n| n <= 2.0)
                    && str_attr(r, "linked_service").is_none() =>
            {
                let (savings, basis) = input.savings(r, builtin::ROUTE53_ZONE);
                let mut f = base("route53-zone-empty", r).evidence("record_sets", num_attr(r, "record_sets"));
                f.title = format!("Hosted zone {} has no records", r.display_name());
                f.summary = "It holds only the NS and SOA records every zone starts with, but is billed monthly.".into();
                f.how_to_fix = "If the domain isn't delegated to this zone or no longer used, delete the zone in the \
                                Route 53 console."
                    .into();
                out.push(finish(f, savings, basis, Some(PriceSource::BuiltIn)));
            }
            T::LoadBalancer if str_attr(r, "state") == Some("active") => {
                let groups: Vec<&Resource> = input
                    .edges
                    .iter()
                    .filter(|e| e.source == r.arn && e.kind == RelationKind::RoutesTo)
                    .filter_map(|e| by_arn.get(e.target.as_str()).copied())
                    .filter(|t| t.resource_type == T::TargetGroup)
                    .collect();
                let targets: f64 = groups.iter().filter_map(|g| num_attr(g, "targets")).sum();
                if targets > 0.0 {
                    continue;
                }
                let kind = str_attr(r, "type").unwrap_or("application").to_owned();
                let (hourly, src) = input
                    .prices
                    .get(&r.region, &PriceItem::LoadBalancerHour(kind.clone()));
                let (savings, basis) = input.savings(r, hourly * HOURS_PER_MONTH);
                let mut f = base("elb-idle", r)
                    .evidence("type", &kind)
                    .evidence("target_groups", groups.len())
                    .evidence("registered_targets", 0);
                f.title = format!("Idle load balancer {}", r.display_name());
                f.summary = if groups.is_empty() {
                    "This load balancer has no target groups, so it can't serve traffic, but it is billed every hour.".into()
                } else {
                    "None of this load balancer's target groups have registered targets.".into()
                };
                f.how_to_fix = "Confirm nothing routes to its DNS name (Route 53 records, CloudFront origins), \
                                then delete it in the EC2 console → Load Balancers, or in your IaC."
                    .into();
                out.push(finish(f, savings, basis, Some(src)));
            }
            T::Ec2Instance if str_attr(r, "state") == Some("stopped") => {
                let volumes: Vec<&Resource> = input
                    .edges
                    .iter()
                    .filter(|e| e.source == r.arn && e.kind == RelationKind::Attached)
                    .filter_map(|e| by_arn.get(e.target.as_str()).copied())
                    .filter(|v| v.resource_type == T::EbsVolume)
                    .collect();
                if volumes.is_empty() {
                    continue;
                }
                let mut savings = 0.0;
                let mut basis = SavingsBasis::Actual;
                let mut source = None;
                for v in &volumes {
                    let (estimate, src) = volume_monthly_cost(input.prices, v);
                    let (s, b) = input.savings(v, estimate);
                    savings += s;
                    if b == SavingsBasis::PublicPrice {
                        basis = SavingsBasis::PublicPrice;
                        source = Some(src);
                    }
                }
                let mut f = base("ec2-stopped-with-ebs", r)
                    .evidence("volumes", volumes.len())
                    .evidence("stopped_reason", str_attr(r, "state_reason"));
                f.resources.extend(volumes.iter().map(|v| subject(v)));
                f.title = format!(
                    "Stopped instance {} still pays for {} volume(s)",
                    r.display_name(),
                    volumes.len()
                );
                f.summary = "Stopped instances don't pay for compute, but their EBS volumes (and any Elastic IPs) keep billing.".into();
                f.how_to_fix = "If the instance is no longer needed, create an AMI or snapshot for safekeeping and terminate it. \
                                If it is needed occasionally, consider a smaller root volume or gp3."
                    .into();
                out.push(finish(f, savings, basis, source));
            }
            T::EbsSnapshot => {
                let Some(age) = age_days(r, "create_time", input.today) else {
                    continue;
                };
                if age < input.snapshot_age_days as i64
                    || str_attr(r, "storage_tier") == Some("archive")
                {
                    continue;
                }
                let size = num_attr(r, "size_gib").unwrap_or(0.0);
                let (per_gb, src) = input.prices.get(&r.region, &PriceItem::EbsSnapshot);
                // Snapshots are incremental: the full volume size is an upper bound.
                let (savings, basis) = input.savings(r, size * per_gb);
                let mut f = base("ebs-snapshot-old", r)
                    .evidence("age_days", age)
                    .evidence("volume_size_gib", size)
                    .evidence("upper_bound", basis == SavingsBasis::PublicPrice);
                f.title = format!("EBS snapshot {} is {age} days old", r.display_name());
                f.summary = format!(
                    "Snapshot of a {size} GiB volume taken {age} days ago. Old snapshots are often forgotten backups."
                );
                f.how_to_fix = "Delete it if no retention policy requires it and no AMI uses it (EC2 → Snapshots → Delete), \
                                or move it to the archive tier. Consider Amazon Data Lifecycle Manager to expire snapshots automatically."
                    .into();
                out.push(finish(f, savings, basis, Some(src)));
            }
            T::NatGateway if str_attr(r, "state") == Some("available") => {
                if let Some(f) = idle_nat(input, r) {
                    out.push(f);
                }
                if let Some(cost) = input
                    .nat_processing_cost
                    .get(&r.native_id)
                    .or_else(|| input.nat_processing_cost.get(&r.arn))
                    && *cost >= 50.0
                {
                    let mut f = Finding::new("nat-data-processing", FindingKind::Pattern, &r.arn);
                    f.account_id = r.account_id.clone();
                    f.region = r.region.clone();
                    f.resources.push(subject(r));
                    f.severity = Severity::from_amount(*cost);
                    f.impact = Some(*cost);
                    f = f.evidence("processing_cost_30d", (cost * 100.0).round() / 100.0);
                    f.title = format!("High NAT data processing on {}", r.display_name());
                    f.summary = format!(
                        "NAT gateway data processing cost {:.2} USD in the last 30 days. Traffic to S3 or DynamoDB doesn't need to go through NAT.",
                        cost
                    );
                    f.how_to_fix = "Add free gateway VPC endpoints for S3 and DynamoDB, and interface endpoints for heavily used \
                                    AWS APIs (ECR, CloudWatch Logs). Check which instances send the most traffic with VPC Flow Logs."
                        .into();
                    out.push(f);
                }
            }
            _ => {}
        }
    }
    out
}

/// A NAT gateway is idle when CUR shows (almost) no bytes processed, or —
/// without CUR — when nothing that could use it runs in its VPC.
fn idle_nat(input: &RuleInput, r: &Resource) -> Option<Finding> {
    let vpc = r.vpc_id()?;
    let evidence = match input.nat_gb {
        Some(gb) => {
            let processed = gb
                .get(&r.native_id)
                .or_else(|| gb.get(&r.arn))
                .copied()
                .unwrap_or(0.0);
            if processed > 1.0 {
                return None;
            }
            (
                "gb_processed_30d",
                processed,
                "Processed under 1 GB in the last 30 days.",
            )
        }
        None => {
            let compute: HashSet<T> = [
                T::Ec2Instance,
                T::LambdaFunction,
                T::EcsService,
                T::EksCluster,
            ]
            .into();
            let running = input.resources.iter().any(|x| {
                compute.contains(&x.resource_type)
                    && x.vpc_id() == Some(vpc)
                    && !matches!(str_attr(x, "state"), Some("stopped") | Some("stopping"))
            });
            if running {
                return None;
            }
            (
                "compute_in_vpc",
                0.0,
                "No running instances, functions or containers in its VPC.",
            )
        }
    };
    let (hourly, src) = input.prices.get(&r.region, &PriceItem::NatGatewayHour);
    let (savings, basis) = input.savings(r, hourly * HOURS_PER_MONTH);
    let mut f = base("nat-idle", r)
        .evidence(evidence.0, evidence.1)
        .evidence("vpc_id", vpc);
    f.title = format!("Idle NAT gateway {}", r.display_name());
    f.summary = format!(
        "{} NAT gateways bill every hour even with no traffic.",
        evidence.2
    );
    f.how_to_fix = "Confirm no private subnet route table needs it for outbound internet access, \
                    then delete the NAT gateway and release its Elastic IP."
        .into();
    Some(finish(f, savings, basis, Some(src)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::arn;
    use crate::inventory::graph::resolve;
    use crate::inventory::{Relation, RelationKind as K};
    use serde_json::json;

    const A: &str = "111111111111";
    const R: &str = "us-east-1";

    fn res(t: T, kind: &str, id: &str, attrs: serde_json::Value) -> Resource {
        let mut r = Resource::new(t, arn::ec2(R, A, kind, id), A, R, id);
        for (k, v) in attrs.as_object().unwrap() {
            r.attrs.insert(k.clone(), v.clone());
        }
        r
    }

    fn run(
        resources: &[Resource],
        relations: &[Relation],
        actual: &HashMap<String, f64>,
        nat_gb: Option<&HashMap<String, f64>>,
    ) -> Vec<Finding> {
        let edges = resolve(resources, relations);
        evaluate(&RuleInput {
            resources,
            edges: &edges,
            prices: &PriceBook::default(),
            actual_costs: actual,
            nat_gb,
            nat_processing_cost: &HashMap::new(),
            snapshot_age_days: 90,
            today: dates::parse("2026-10-14").unwrap(),
        })
    }

    #[test]
    fn unattached_ebs_volume_savings() {
        // Acceptance: a seeded unattached volume yields the correct estimate.
        let gp3 = res(
            T::EbsVolume,
            "volume",
            "vol-1",
            json!({"state": "available", "volume_type": "gp3", "size_gib": 500, "iops": 6000, "throughput_mibps": 250}),
        );
        let io1 = res(
            T::EbsVolume,
            "volume",
            "vol-2",
            json!({"state": "available", "volume_type": "io1", "size_gib": 100, "iops": 1000}),
        );
        let used = res(
            T::EbsVolume,
            "volume",
            "vol-3",
            json!({"state": "in-use", "volume_type": "gp3", "size_gib": 50}),
        );
        let findings = run(&[gp3, io1, used], &[], &HashMap::new(), None);
        assert_eq!(findings.len(), 2);

        let f = findings.iter().find(|f| f.id.ends_with("vol-1")).unwrap();
        // 500 × 0.08 + 3000 extra IOPS × 0.005 + 125 extra MiB/s × 0.04 = 40 + 15 + 5
        assert_eq!(f.monthly_savings, Some(60.0));
        assert_eq!(f.savings_basis, Some(SavingsBasis::PublicPrice));
        assert_eq!(f.severity, Severity::Medium);
        let f = findings.iter().find(|f| f.id.ends_with("vol-2")).unwrap();
        // 100 × 0.125 + 1000 × 0.065
        assert_eq!(f.monthly_savings, Some(77.5));

        // With CUR, the actual cost wins.
        let vol = res(
            T::EbsVolume,
            "volume",
            "vol-1",
            json!({"state": "available", "volume_type": "gp3", "size_gib": 500}),
        );
        let actual = HashMap::from([("vol-1".to_owned(), 37.5)]);
        let f = &run(&[vol], &[], &actual, None)[0];
        assert_eq!(
            (f.monthly_savings, f.savings_basis),
            (Some(37.5), Some(SavingsBasis::Actual))
        );
    }

    #[test]
    fn other_waste_rules() {
        let eip = res(
            T::ElasticIp,
            "elastic-ip",
            "eipalloc-1",
            json!({"associated": false}),
        );
        let lb = {
            let mut r = Resource::new(
                T::LoadBalancer,
                "arn:aws:elasticloadbalancing:us-east-1:1:loadbalancer/app/old/1",
                A,
                R,
                "old",
            );
            r.attrs.insert("state".into(), json!("active"));
            r.attrs.insert("type".into(), json!("application"));
            r
        };
        let tg = {
            let mut r = Resource::new(
                T::TargetGroup,
                "arn:aws:elasticloadbalancing:us-east-1:1:targetgroup/old/2",
                A,
                R,
                "old-tg",
            );
            r.attrs.insert("targets".into(), json!(0));
            r
        };
        let stopped = res(
            T::Ec2Instance,
            "instance",
            "i-1",
            json!({"state": "stopped", "vpc_id": "vpc-1"}),
        );
        let vol = res(
            T::EbsVolume,
            "volume",
            "vol-9",
            json!({"state": "in-use", "volume_type": "gp2", "size_gib": 100}),
        );
        let snap_old = res(
            T::EbsSnapshot,
            "snapshot",
            "snap-1",
            json!({"size_gib": 20, "create_time": "2026-01-01T00:00:00Z"}),
        );
        let snap_new = res(
            T::EbsSnapshot,
            "snapshot",
            "snap-2",
            json!({"size_gib": 20, "create_time": "2026-10-01T00:00:00Z"}),
        );
        let nat = res(
            T::NatGateway,
            "natgateway",
            "nat-1",
            json!({"state": "available", "vpc_id": "vpc-1"}),
        );
        let relations = vec![
            Relation::new(&lb.arn, &tg.arn, K::RoutesTo),
            Relation::new(&stopped.arn, &vol.arn, K::Attached),
        ];
        let resources = vec![eip, lb, tg, stopped, vol, snap_old, snap_new, nat];
        let findings = run(&resources, &relations, &HashMap::new(), None);
        let by_rule = |rule: &str| {
            findings
                .iter()
                .filter(|f| f.rule == rule)
                .collect::<Vec<_>>()
        };

        assert_eq!(by_rule("eip-unassociated")[0].monthly_savings, Some(3.65));
        assert_eq!(by_rule("elb-idle")[0].monthly_savings, Some(16.43));
        let stopped = by_rule("ec2-stopped-with-ebs");
        assert_eq!(stopped[0].monthly_savings, Some(10.0));
        assert_eq!(stopped[0].resources.len(), 2);
        assert_eq!(by_rule("ebs-snapshot-old").len(), 1);
        assert_eq!(by_rule("ebs-snapshot-old")[0].monthly_savings, Some(1.0));
        // The only instance in the VPC is stopped → NAT likely idle: 0.045 × 730.
        assert_eq!(by_rule("nat-idle")[0].monthly_savings, Some(32.85));

        // With CUR traffic data, a busy NAT is not idle.
        let busy = HashMap::from([("nat-1".to_owned(), 250.0)]);
        let findings = run(&resources, &relations, &HashMap::new(), Some(&busy));
        assert!(findings.iter().all(|f| f.rule != "nat-idle"));
    }

    #[test]
    fn lightsail_waste_rules() {
        let ls = |t: T, name: &str, attrs: serde_json::Value| {
            let mut r = Resource::new(t, format!("arn:aws:lightsail:{R}:{A}:{name}"), A, R, name);
            for (k, v) in attrs.as_object().unwrap() {
                r.attrs.insert(k.clone(), v.clone());
            }
            r
        };
        let resources = vec![
            ls(T::LightsailStaticIp, "ip-free", json!({"attached": false})),
            ls(T::LightsailStaticIp, "ip-used", json!({"attached": true})),
            // Scanned before `attached` was recorded: unknown, not flagged.
            ls(T::LightsailStaticIp, "ip-old", json!({})),
            ls(
                T::LightsailInstance,
                "vm-stopped",
                json!({"state": "stopped", "monthly_price_usd": 24.0}),
            ),
            ls(
                T::LightsailInstance,
                "vm-running",
                json!({"state": "running", "monthly_price_usd": 24.0}),
            ),
            ls(
                T::LightsailInstance,
                "vm-no-price",
                json!({"state": "stopped"}),
            ),
            ls(
                T::LightsailDisk,
                "disk-free",
                json!({"attached": false, "size_gib": 32}),
            ),
            ls(
                T::LightsailDisk,
                "disk-used",
                json!({"attached": true, "size_gib": 32}),
            ),
        ];
        let findings = run(&resources, &[], &HashMap::new(), None);
        let rules: Vec<(&str, Option<f64>)> = findings
            .iter()
            .map(|f| (f.rule, f.monthly_savings))
            .collect();
        assert_eq!(
            rules,
            [
                ("lightsail-static-ip-unattached", Some(3.65)),
                ("lightsail-instance-stopped", Some(24.0)),
                ("lightsail-disk-unattached", Some(3.2)),
            ]
        );
        let stopped = &findings[1];
        assert_eq!(stopped.savings_basis, Some(SavingsBasis::PublicPrice));
        assert_eq!(
            stopped.evidence["price_source"],
            "Lightsail bundle price (public price)"
        );
        // CUR actual cost wins over the list price.
        let actual = HashMap::from([(resources[3].arn.clone(), 19.5)]);
        let with_cur = run(&resources, &[], &actual, None);
        let f = with_cur
            .iter()
            .find(|f| f.rule == "lightsail-instance-stopped")
            .unwrap();
        assert_eq!(
            (f.monthly_savings, f.savings_basis),
            (Some(19.5), Some(SavingsBasis::Actual))
        );
    }

    #[test]
    fn platform_waste_rules() {
        // today is 2026-10-14; 90+ days ago is before 2026-07-16.
        let r = |t: T, name: &str, attrs: serde_json::Value| {
            let mut r = Resource::new(t, format!("arn:{name}"), A, R, name);
            for (k, v) in attrs.as_object().unwrap() {
                r.attrs.insert(k.clone(), v.clone());
            }
            r
        };
        let old = "2026-01-10T00:00:00Z";
        let recent = "2026-10-01T00:00:00Z";
        let resources = vec![
            r(T::SecretsManagerSecret, "s-old", json!({"last_accessed": old, "created": old})),
            r(T::SecretsManagerSecret, "s-never", json!({"created": old})),
            r(T::SecretsManagerSecret, "s-used", json!({"last_accessed": recent, "created": old})),
            r(T::SecretsManagerSecret, "s-rds", json!({"created": old, "owning_service": "rds"})),
            r(T::SecretsManagerSecret, "s-deleting", json!({"created": old, "deleted": recent})),
            r(T::KmsKey, "k-off", json!({"state": "Disabled"})),
            r(T::KmsKey, "k-on", json!({"state": "Enabled"})),
            r(T::EcrRepository, "e-idle", json!({"size_gib": 4.0, "last_pulled": old, "last_pushed": old,
                                               "lifecycle_policy": false, "untagged_size_gib": 2.0})),
            r(T::EcrRepository, "e-messy", json!({"size_gib": 4.0, "last_pulled": recent, "last_pushed": recent,
                                                "lifecycle_policy": false, "untagged_size_gib": 2.0, "untagged_images": 9})),
            r(T::EcrRepository, "e-tidy", json!({"size_gib": 4.0, "last_pulled": recent, "lifecycle_policy": true,
                                               "untagged_size_gib": 2.0})),
            r(T::LogGroup, "l-forever", json!({"stored_gib": 50.0})),
            r(T::LogGroup, "l-small", json!({"stored_gib": 0.2})),
            r(T::LogGroup, "l-30d", json!({"stored_gib": 50.0, "retention_days": 30})),
            r(T::EfsFileSystem, "f-orphan", json!({"state": "available", "mount_targets": 0,
                                                  "standard_gib": 100.0, "ia_gib": 100.0, "size_gib": 200.0})),
            r(T::EfsFileSystem, "f-used", json!({"state": "available", "mount_targets": 2, "standard_gib": 10.0})),
            r(T::Route53HostedZone, "z-empty", json!({"record_sets": 2})),
            r(T::Route53HostedZone, "z-used", json!({"record_sets": 14})),
        ];
        let findings = run(&resources, &[], &HashMap::new(), None);
        let got: Vec<(&str, String, Option<f64>)> = findings
            .iter()
            .map(|f| (f.rule, f.resources[0].label.clone(), f.monthly_savings))
            .collect();
        let want = [
            ("secret-unused", "s-old", Some(0.4)),
            ("secret-unused", "s-never", Some(0.4)),
            ("kms-key-disabled", "k-off", Some(1.0)),
            // Idle wins over untagged images: the whole repository goes.
            ("ecr-repository-idle", "e-idle", Some(0.4)),
            ("ecr-untagged-images", "e-messy", Some(0.2)),
            ("logs-no-retention", "l-forever", Some(1.5)),
            ("efs-no-mount-targets", "f-orphan", Some(31.6)),
            ("route53-zone-empty", "z-empty", Some(0.5)),
        ];
        assert_eq!(
            got,
            want.map(|(rule, label, s)| (rule, label.to_owned(), s))
        );
        // A repository whose images couldn't be listed has no price, not $0.
        assert_eq!(platform_monthly(&r(T::EcrRepository, "e-unknown", json!({}))), None);
        assert_eq!(platform_monthly(&r(T::EcrRepository, "e-empty", json!({"size_gib": 0.0}))), Some(0.0));
    }
}
