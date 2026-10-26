// SPDX-License-Identifier: MIT OR Apache-2.0

//! Which tool made a resource (infrastructure as code), from what AWS
//! leaves behind: CloudFormation stack membership (recorded during sync),
//! tags that tools add or teams use by convention, and generated names.
//! Each match says how sure it is and why.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use super::graph::GraphEdge;
use super::{RelationKind, Resource, ResourceType};

/// Attributes set during sync on resources a CloudFormation stack manages.
pub const ATTR_STACK: &str = "cfn_stack";
pub const ATTR_LOGICAL_ID: &str = "cfn_logical_id";
pub const ATTR_TOOL: &str = "cfn_tool";
pub const ATTR_TOOL_REASON: &str = "cfn_tool_reason";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IacTool {
    Cdk,
    CloudFormation,
    Sam,
    Serverless,
    ElasticBeanstalk,
    Copilot,
    Amplify,
    Terraform,
    Pulumi,
}

impl IacTool {
    pub const ALL: &[IacTool] = &[
        Self::Cdk,
        Self::CloudFormation,
        Self::Sam,
        Self::Serverless,
        Self::ElasticBeanstalk,
        Self::Copilot,
        Self::Amplify,
        Self::Terraform,
        Self::Pulumi,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Cdk => "cdk",
            Self::CloudFormation => "cloud_formation",
            Self::Sam => "sam",
            Self::Serverless => "serverless",
            Self::ElasticBeanstalk => "elastic_beanstalk",
            Self::Copilot => "copilot",
            Self::Amplify => "amplify",
            Self::Terraform => "terraform",
            Self::Pulumi => "pulumi",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cdk => "AWS CDK",
            Self::CloudFormation => "CloudFormation",
            Self::Sam => "AWS SAM",
            Self::Serverless => "Serverless Framework",
            Self::ElasticBeanstalk => "Elastic Beanstalk",
            Self::Copilot => "AWS Copilot",
            Self::Amplify => "Amplify",
            Self::Terraform => "Terraform",
            Self::Pulumi => "Pulumi",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| t.id() == id)
    }

    /// Tools that deploy through CloudFormation stacks.
    pub fn deploys_with_cloudformation(self) -> bool {
        !matches!(self, Self::Terraform | Self::Pulumi)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// AWS or the tool recorded it (stack membership, tags it adds), or the
    /// team declared it in a tag.
    Certain,
    /// Strong pattern: a CDK-style logical ID, a Terraform-generated name,
    /// or the resource it is attached to.
    Likely,
    /// Weak pattern, e.g. a `tf-` name prefix.
    Hint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedBy {
    pub tool: IacTool,
    pub confidence: Confidence,
    /// Why, in words: "tag aws:cloudformation:stack-name = api".
    pub evidence: String,
    /// The CloudFormation stack, when there is one.
    pub stack: Option<String>,
}

impl ManagedBy {
    fn new(tool: IacTool, confidence: Confidence, evidence: impl Into<String>) -> Self {
        Self {
            tool,
            confidence,
            evidence: evidence.into(),
            stack: None,
        }
    }

    fn in_stack(mut self, stack: Option<&str>) -> Self {
        self.stack = stack.map(str::to_owned);
        self
    }
}

/// Resource types that are created as a side effect of others (network
/// interfaces of instances, Lambda or load balancers; snapshots taken by
/// backups; log groups that Lambda and other services create on first
/// write): they don't count toward coverage on their own. Network
/// interfaces inherit from their owner (see [`detect_all`]).
pub fn is_derived(t: ResourceType) -> bool {
    matches!(
        t,
        ResourceType::NetworkInterface | ResourceType::EbsSnapshot | ResourceType::LogGroup
    )
}

/// CDK logical IDs end in an 8-character uppercase hex hash of the
/// construct path: `MyBucketF68F3FF0`, `ApiHandler5E7490E8`.
fn cdk_logical_id(id: &str) -> bool {
    let n = id.len();
    n > 8
        && id.is_char_boundary(n - 8)
        && id[n - 8..]
            .chars()
            .all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c))
        && id[..n - 8].chars().any(|c| c.is_ascii_alphabetic())
}

/// A tool named in a convention tag's value, e.g. `ManagedBy = Terraform`.
fn tool_in_value(value: &str) -> Option<IacTool> {
    let v = value.to_ascii_lowercase();
    let has = |w: &str| v.contains(w);
    if has("terraform") || has("opentofu") || v == "tofu" || v == "tf" {
        Some(IacTool::Terraform)
    } else if has("pulumi") {
        Some(IacTool::Pulumi)
    } else if has("cdk") {
        Some(IacTool::Cdk)
    } else if has("cloudformation") || v == "cfn" {
        Some(IacTool::CloudFormation)
    } else if v == "sam" || has("aws-sam") {
        Some(IacTool::Sam)
    } else if has("serverless") {
        Some(IacTool::Serverless)
    } else {
        None
    }
}

/// Tag keys teams use to say how a resource is managed (compared lowercase,
/// without `-`, `_`, `:` or spaces).
const CONVENTION_KEYS: &[&str] = &[
    "managedby",
    "provisionedby",
    "provisioner",
    "createdby",
    "deployedby",
    "iac",
    "iactool",
    "tool",
    "automation",
    "source",
];

fn normalize_key(k: &str) -> String {
    k.chars()
        .filter(|c| !matches!(c, '-' | '_' | ':' | ' ' | '.'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn truthy(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "1" | "y")
}

/// From tags alone.
fn from_tags(tags: &BTreeMap<String, String>) -> Option<ManagedBy> {
    let tag = |k: &str| tags.get(k).map(String::as_str);
    if tag("lambda:createdBy") == Some("SAM") {
        return Some(ManagedBy::new(
            IacTool::Sam,
            Confidence::Certain,
            "tag lambda:createdBy = SAM",
        ));
    }
    if let Some(app) = tag("copilot-application") {
        return Some(ManagedBy::new(
            IacTool::Copilot,
            Confidence::Certain,
            format!("tag copilot-application = {app}"),
        ));
    }
    if let Some(env) = tag("elasticbeanstalk:environment-name") {
        return Some(ManagedBy::new(
            IacTool::ElasticBeanstalk,
            Confidence::Certain,
            format!("tag elasticbeanstalk:environment-name = {env}"),
        ));
    }
    if let Some(stack) = tag("aws:cloudformation:stack-name") {
        let evidence = format!("tag aws:cloudformation:stack-name = {stack}");
        // The team's own tag can name the CloudFormation-based tool for
        // certain (`ManagedBy = cdk`); one naming another kind of tool
        // (`terraform`) loses to the stack AWS recorded.
        if let Some(d) = declared(tags).filter(|d| d.tool.deploys_with_cloudformation()) {
            return Some(
                ManagedBy::new(d.tool, Confidence::Certain, format!("{}; {evidence}", d.evidence))
                    .in_stack(Some(stack)),
            );
        }
        let found = if stack.starts_with("awseb-") {
            ManagedBy::new(IacTool::ElasticBeanstalk, Confidence::Certain, evidence)
        } else if stack.starts_with("amplify-") {
            ManagedBy::new(IacTool::Amplify, Confidence::Likely, evidence)
        } else if let Some(id) = tag("aws:cloudformation:logical-id").filter(|id| cdk_logical_id(id)) {
            ManagedBy::new(
                IacTool::Cdk,
                Confidence::Likely,
                format!("{evidence}; logical ID {id} has a CDK hash suffix"),
            )
        } else {
            ManagedBy::new(IacTool::CloudFormation, Confidence::Certain, evidence)
        };
        return Some(found.in_stack(Some(stack)));
    }
    declared(tags)
}

/// A tool the team declared in a tag: `ManagedBy = cdk`, `terraform = true`,
/// `pulumi:project`.
fn declared(tags: &BTreeMap<String, String>) -> Option<ManagedBy> {
    for (k, v) in tags {
        if k.starts_with("pulumi:") {
            return Some(ManagedBy::new(
                IacTool::Pulumi,
                Confidence::Certain,
                format!("tag {k}"),
            ));
        }
        let key = normalize_key(k);
        // `terraform = true`, `pulumi = yes`.
        let by_key = match key.as_str() {
            "terraform" | "terraformmanaged" | "tf" => Some(IacTool::Terraform),
            "pulumi" | "pulumimanaged" => Some(IacTool::Pulumi),
            _ => None,
        };
        if let Some(tool) = by_key.filter(|_| truthy(v)) {
            return Some(ManagedBy::new(
                tool,
                Confidence::Certain,
                format!("tag {k} = {v}"),
            ));
        }
        if CONVENTION_KEYS.contains(&key.as_str())
            && let Some(tool) = tool_in_value(v)
        {
            return Some(ManagedBy::new(
                tool,
                Confidence::Certain,
                format!("tag {k} = {v}"),
            ));
        }
    }
    None
}

/// Terraform names resources it must name itself `terraform-` plus a
/// timestamp-based ID (`terraform-20240612153045123400000001`).
fn terraform_generated(name: &str) -> bool {
    name.strip_prefix("terraform-").is_some_and(|rest| {
        rest.len() >= 20 && rest.chars().take(20).all(|c| c.is_ascii_digit())
    })
}

/// The tool that made `r`, from its own data (no neighbours).
pub fn detect(r: &Resource) -> Option<ManagedBy> {
    let attr = |k: &str| r.attrs.get(k).and_then(|v| v.as_str());
    // Stack membership, recorded during sync: certain. Tags can still
    // name a tool that deploys through CloudFormation (SAM, Copilot).
    if let Some(stack) = attr(ATTR_STACK) {
        let tool = attr(ATTR_TOOL)
            .and_then(IacTool::from_id)
            .unwrap_or(IacTool::CloudFormation);
        if tool == IacTool::CloudFormation
            && let Some(m) = from_tags(&r.tags).filter(|m| {
                m.tool != IacTool::CloudFormation
                    && m.tool.deploys_with_cloudformation()
                    && m.confidence == Confidence::Certain
            })
        {
            return Some(m.in_stack(Some(stack)));
        }
        let logical = attr(ATTR_LOGICAL_ID);
        let mut evidence = match logical {
            Some(id) => format!("{id} in CloudFormation stack {stack}"),
            None => format!("in CloudFormation stack {stack}"),
        };
        if let Some(reason) = attr(ATTR_TOOL_REASON) {
            evidence.push_str(&format!("; {reason}"));
        }
        // CDK apps can turn off the metadata resource; their hashed logical
        // IDs still give them away (the stack itself is certain).
        if tool == IacTool::CloudFormation
            && let Some(id) = logical.filter(|id| cdk_logical_id(id))
        {
            return Some(
                ManagedBy::new(
                    IacTool::Cdk,
                    Confidence::Likely,
                    format!("{evidence}; logical ID {id} has a CDK hash suffix"),
                )
                .in_stack(Some(stack)),
            );
        }
        return Some(ManagedBy::new(tool, Confidence::Certain, evidence).in_stack(Some(stack)));
    }
    if let Some(m) = from_tags(&r.tags) {
        return Some(m);
    }
    for name in [Some(r.native_id.as_str()), r.name.as_deref()].into_iter().flatten() {
        if terraform_generated(name) {
            return Some(ManagedBy::new(
                IacTool::Terraform,
                Confidence::Likely,
                format!("name {name} is Terraform-generated"),
            ));
        }
    }
    for name in [r.name.as_deref(), Some(r.native_id.as_str())].into_iter().flatten() {
        if name.starts_with("tf-") || name.starts_with("tf_") {
            return Some(ManagedBy::new(
                IacTool::Terraform,
                Confidence::Hint,
                format!("name {name} starts with tf-"),
            ));
        }
    }
    None
}

/// Detection for every resource; derived resources (volumes, network
/// interfaces, snapshots) without their own evidence take it, as Likely,
/// from what they are attached to.
pub fn detect_all<'a>(
    resources: &'a [Resource],
    edges: &[GraphEdge],
) -> HashMap<&'a str, ManagedBy> {
    let mut found: HashMap<&str, ManagedBy> = resources
        .iter()
        .filter_map(|r| Some((r.arn.as_str(), detect(r)?)))
        .collect();
    let by_arn: HashMap<&str, &Resource> = resources.iter().map(|r| (r.arn.as_str(), r)).collect();
    let mut inherited = Vec::new();
    for r in resources {
        // Volumes and network interfaces only. Snapshots don't inherit: they
        // are usually taken by backups, not by the tool that made the volume.
        if found.contains_key(r.arn.as_str())
            || !matches!(
                r.resource_type,
                ResourceType::EbsVolume | ResourceType::NetworkInterface
            )
        {
            continue;
        }
        // instance → volume (Attached), owner → network interface (Owns).
        // A snapshot → volume edge points the other way round: never an owner.
        let owner = edges.iter().find_map(|e| {
            if e.target != r.arn || !matches!(e.kind, RelationKind::Attached | RelationKind::Owns) {
                return None;
            }
            let source = by_arn.get(e.source.as_str())?;
            if source.resource_type == ResourceType::EbsSnapshot {
                return None;
            }
            Some((e.source.as_str(), found.get(e.source.as_str())?))
        });
        if let Some((arn, m)) = owner {
            let label = by_arn.get(arn).map_or(arn, |o| o.display_name());
            inherited.push((
                r.arn.as_str(),
                ManagedBy {
                    tool: m.tool,
                    confidence: Confidence::Likely,
                    evidence: format!("attached to {label} ({})", m.evidence),
                    stack: m.stack.clone(),
                },
            ));
        }
    }
    found.extend(inherited);
    found
}

/// How much of the inventory is managed as code.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct IacSummary {
    /// Resources that count (derived ones excluded).
    pub total: usize,
    /// Per tool, most first; `certain` of them by stack membership or tags.
    pub by_tool: Vec<ToolCount>,
    /// Resources with no evidence of any tool.
    pub unmanaged: usize,
    /// The unmanaged ones by resource type, most first.
    pub unmanaged_by_type: Vec<TypeCount>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TypeCount {
    pub resource_type: ResourceType,
    pub label: &'static str,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolCount {
    pub tool: IacTool,
    pub label: &'static str,
    pub resources: usize,
    pub certain: usize,
}

pub fn summarize(resources: &[Resource], found: &HashMap<&str, ManagedBy>) -> IacSummary {
    let mut s = IacSummary::default();
    let mut tools: BTreeMap<IacTool, (usize, usize)> = BTreeMap::new();
    let mut unmanaged: BTreeMap<ResourceType, usize> = BTreeMap::new();
    for r in resources.iter().filter(|r| !is_derived(r.resource_type)) {
        s.total += 1;
        match found.get(r.arn.as_str()) {
            Some(m) => {
                let e = tools.entry(m.tool).or_default();
                e.0 += 1;
                if m.confidence == Confidence::Certain {
                    e.1 += 1;
                }
            }
            None => {
                s.unmanaged += 1;
                *unmanaged.entry(r.resource_type).or_default() += 1;
            }
        }
    }
    s.by_tool = tools
        .into_iter()
        .map(|(tool, (resources, certain))| ToolCount {
            tool,
            label: tool.label(),
            resources,
            certain,
        })
        .collect();
    s.by_tool.sort_by_key(|t| std::cmp::Reverse(t.resources));
    s.unmanaged_by_type = unmanaged
        .into_iter()
        .map(|(resource_type, count)| TypeCount {
            resource_type,
            label: resource_type.label(),
            count,
        })
        .collect();
    s.unmanaged_by_type.sort_by_key(|t| std::cmp::Reverse(t.count));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::graph::GraphEdge;

    fn res(t: ResourceType, id: &str, tags: &[(&str, &str)]) -> Resource {
        let mut r = Resource::new(t, format!("arn:{id}"), "1", "us-east-1", id);
        for (k, v) in tags {
            r.tags.insert((*k).into(), (*v).into());
        }
        r
    }

    fn tool(r: &Resource) -> Option<(IacTool, Confidence)> {
        detect(r).map(|m| (m.tool, m.confidence))
    }

    #[test]
    fn detects_tools_from_stacks_tags_and_names() {
        use Confidence::*;
        use IacTool::*;
        use ResourceType as T;
        let mut in_stack = res(T::SqsQueue, "q", &[]);
        in_stack.attrs.insert(ATTR_STACK.into(), "api".into());
        in_stack.attrs.insert(ATTR_TOOL.into(), "cdk".into());
        in_stack.attrs.insert(ATTR_LOGICAL_ID.into(), "QueueA1B2C3D4".into());
        assert_eq!(tool(&in_stack), Some((Cdk, Certain)));
        assert_eq!(detect(&in_stack).unwrap().stack.as_deref(), Some("api"));

        // A CloudFormation stack without CDK metadata, but CDK logical IDs.
        let mut quiet_cdk = res(T::SqsQueue, "q2", &[]);
        quiet_cdk.attrs.insert(ATTR_STACK.into(), "api".into());
        quiet_cdk.attrs.insert(ATTR_TOOL.into(), "cloud_formation".into());
        quiet_cdk.attrs.insert(ATTR_LOGICAL_ID.into(), "JobsQueueA1B2C3D4".into());
        assert_eq!(tool(&quiet_cdk), Some((Cdk, Likely)));

        // In a plain CloudFormation stack, the SAM tag names the tool.
        let mut sam = res(T::LambdaFunction, "fn", &[("lambda:createdBy", "SAM")]);
        sam.attrs.insert(ATTR_STACK.into(), "svc".into());
        assert_eq!(tool(&sam), Some((Sam, Certain)));

        let cfn_tags = [
            ("aws:cloudformation:stack-name", "mkt-stack-blue"),
            ("aws:cloudformation:logical-id", "FrontEndAutoScaleGroup"),
        ];
        assert_eq!(tool(&res(T::Ec2Instance, "i-1", &cfn_tags)), Some((CloudFormation, Certain)));
        let cdk_tags = [
            ("aws:cloudformation:stack-name", "Api"),
            ("aws:cloudformation:logical-id", "HandlerF68F3FF0"),
        ];
        assert_eq!(tool(&res(T::LambdaFunction, "f", &cdk_tags)), Some((Cdk, Likely)));
        // The team's ManagedBy tag makes it certain, with both pieces of evidence.
        let declared_cdk = [
            ("ManagedBy", "cdk"),
            ("aws:cloudformation:stack-name", "Jc-Site-JuanCarvajal"),
            ("aws:cloudformation:logical-id", "SiteBucket978D4AEB"),
        ];
        let m = detect(&res(T::S3Bucket, "site", &declared_cdk)).unwrap();
        assert_eq!((m.tool, m.confidence), (Cdk, Certain));
        assert_eq!(
            m.evidence,
            "tag ManagedBy = cdk; tag aws:cloudformation:stack-name = Jc-Site-JuanCarvajal"
        );
        // A tag claiming Terraform doesn't beat the stack AWS recorded.
        let conflicting = [("ManagedBy", "terraform"), ("aws:cloudformation:stack-name", "web")];
        assert_eq!(tool(&res(T::S3Bucket, "b", &conflicting)), Some((CloudFormation, Certain)));
        // In a plain CloudFormation stack, a declared CDK tag names the tool.
        let mut in_plain = res(T::S3Bucket, "b2", &[("ManagedBy", "cdk")]);
        in_plain.attrs.insert(ATTR_STACK.into(), "site".into());
        in_plain.attrs.insert(ATTR_TOOL.into(), "cloud_formation".into());
        assert_eq!(tool(&in_plain), Some((Cdk, Certain)));
        assert_eq!(
            tool(&res(T::S3Bucket, "b", &[("ManagedBy", "Terraform")])),
            Some((Terraform, Certain))
        );
        assert_eq!(tool(&res(T::S3Bucket, "b", &[("terraform", "true")])), Some((Terraform, Certain)));
        assert_eq!(tool(&res(T::S3Bucket, "b", &[("pulumi:project", "x")])), Some((Pulumi, Certain)));
        assert_eq!(
            tool(&res(T::SecurityGroup, "terraform-20240612153045123400000001", &[])),
            Some((Terraform, Likely))
        );
        assert_eq!(tool(&res(T::SnsTopic, "tf-alerts", &[])), Some((Terraform, Hint)));
        assert_eq!(tool(&res(T::S3Bucket, "logs", &[("Name", "logs")])), None);
        // Not a CDK hash: lowercase hex, or nothing before it.
        assert!(!cdk_logical_id("Queue1a2b3c4d") && !cdk_logical_id("F68F3FF0"));
    }

    #[test]
    fn attached_resources_inherit_and_coverage_skips_derived_ones() {
        use ResourceType as T;
        let instance = res(T::Ec2Instance, "i-1", &[("aws:cloudformation:stack-name", "web")]);
        let volume = res(T::EbsVolume, "vol-1", &[]);
        let eni = res(T::NetworkInterface, "eni-1", &[]);
        let bucket = res(T::S3Bucket, "manual", &[]);
        // A tagged snapshot of a hand-made volume says nothing about the volume.
        let snapshot = res(T::EbsSnapshot, "snap-1", &[("ManagedBy", "terraform")]);
        let manual_volume = res(T::EbsVolume, "vol-2", &[]);
        let resources = vec![instance, volume, eni, bucket, snapshot, manual_volume];
        let edges = vec![
            GraphEdge {
                source: "arn:i-1".into(),
                target: "arn:vol-1".into(),
                kind: RelationKind::Attached,
            },
            GraphEdge {
                source: "arn:i-1".into(),
                target: "arn:eni-1".into(),
                kind: RelationKind::Owns,
            },
            GraphEdge {
                source: "arn:snap-1".into(),
                target: "arn:vol-2".into(),
                kind: RelationKind::Attached,
            },
        ];
        let found = detect_all(&resources, &edges);
        let vol = &found["arn:vol-1"];
        assert_eq!((vol.tool, vol.confidence), (IacTool::CloudFormation, Confidence::Likely));
        assert!(vol.evidence.starts_with("attached to i-1"), "{}", vol.evidence);
        assert!(found.contains_key("arn:eni-1"));
        assert!(!found.contains_key("arn:vol-2"), "not from its snapshot");

        let s = summarize(&resources, &found);
        // The network interface and snapshot don't count; the rest do.
        assert_eq!((s.total, s.unmanaged), (4, 2));
        assert_eq!(s.by_tool[0].resources, 2);
        assert_eq!(s.by_tool[0].certain, 1);
        let unmanaged: Vec<_> = s.unmanaged_by_type.iter().map(|t| (t.resource_type, t.count)).collect();
        assert!(unmanaged.contains(&(T::S3Bucket, 1)) && unmanaged.contains(&(T::EbsVolume, 1)));
    }
}
