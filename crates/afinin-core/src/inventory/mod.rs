// SPDX-License-Identifier: MIT OR Apache-2.0

//! Inventory and service dependency map (FR-20..FR-25).
//!
//! Collectors list resources with read-only `Describe*`/`List*`/`Get*` calls and
//! record relationships by target ARN (or DNS name). The graph builder resolves
//! them over everything stored locally.

pub mod arn;
pub mod collect;
pub mod graph;
pub mod iac;
pub mod sync;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

macro_rules! resource_types {
    ($($variant:ident => $id:literal, $label:literal, $tier:ident, $service:expr;)*) => {
        /// Kinds of resources Afinín discovers.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum ResourceType {
            $(#[serde(rename = $id)] $variant,)*
        }

        impl ResourceType {
            pub const ALL: &[ResourceType] = &[$(Self::$variant,)*];

            pub fn id(self) -> &'static str {
                match self { $(Self::$variant => $id,)* }
            }

            pub fn from_id(s: &str) -> Option<Self> {
                match s { $($id => Some(Self::$variant),)* _ => None }
            }

            pub fn label(self) -> &'static str {
                match self { $(Self::$variant => $label,)* }
            }

            /// Column in the left-to-right graph layout.
            pub fn tier(self) -> Tier {
                match self { $(Self::$variant => Tier::$tier,)* }
            }

            /// Cost Explorer `SERVICE` value this resource is billed under, used
            /// for the per-service cost overlay when CUR is not available (FR-24).
            pub fn cost_service(self) -> Option<&'static str> {
                match self { $(Self::$variant => $service,)* }
            }
        }
    };
}

const EC2: Option<&str> = Some("Amazon Elastic Compute Cloud - Compute");
const EC2_OTHER: Option<&str> = Some("EC2 - Other");
const LIGHTSAIL: Option<&str> = Some("Amazon Lightsail");

resource_types! {
    CloudFrontDistribution => "cloudfront:distribution", "CloudFront distribution", Edge, Some("Amazon CloudFront");
    ApiGatewayRestApi => "apigateway:restapi", "API Gateway REST API", Edge, Some("Amazon API Gateway");
    ApiGatewayHttpApi => "apigateway:api", "API Gateway HTTP/WebSocket API", Edge, Some("Amazon API Gateway");
    LoadBalancer => "elasticloadbalancing:loadbalancer", "Load balancer", Balancing, Some("Amazon Elastic Load Balancing");
    TargetGroup => "elasticloadbalancing:targetgroup", "Target group", Routing, None;
    Ec2Instance => "ec2:instance", "EC2 instance", Compute, EC2;
    EcsCluster => "ecs:cluster", "ECS cluster", Compute, Some("Amazon Elastic Container Service");
    EcsService => "ecs:service", "ECS service", Compute, Some("Amazon Elastic Container Service");
    EksCluster => "eks:cluster", "EKS cluster", Compute, Some("Amazon Elastic Container Service for Kubernetes");
    LambdaFunction => "lambda:function", "Lambda function", Compute, Some("AWS Lambda");
    EbsVolume => "ec2:volume", "EBS volume", Storage, EC2_OTHER;
    EbsSnapshot => "ec2:snapshot", "EBS snapshot", Storage, EC2_OTHER;
    RdsInstance => "rds:db", "RDS instance", Data, Some("Amazon Relational Database Service");
    RdsCluster => "rds:cluster", "RDS cluster", Data, Some("Amazon Relational Database Service");
    DynamoDbTable => "dynamodb:table", "DynamoDB table", Data, Some("Amazon DynamoDB");
    ElastiCacheCluster => "elasticache:cluster", "ElastiCache cluster", Data, Some("Amazon ElastiCache");
    S3Bucket => "s3:bucket", "S3 bucket", Data, Some("Amazon Simple Storage Service");
    SqsQueue => "sqs:queue", "SQS queue", Messaging, Some("Amazon Simple Queue Service");
    SnsTopic => "sns:topic", "SNS topic", Messaging, Some("Amazon Simple Notification Service");
    NatGateway => "ec2:natgateway", "NAT gateway", Network, EC2_OTHER;
    ElasticIp => "ec2:elastic-ip", "Elastic IP", Network, EC2_OTHER;
    Vpc => "ec2:vpc", "VPC", Network, Some("Amazon Virtual Private Cloud");
    Subnet => "ec2:subnet", "Subnet", Network, None;
    SecurityGroup => "ec2:security-group", "Security group", Network, None;
    NetworkInterface => "ec2:network-interface", "Network interface", Network, None;
    LightsailLoadBalancer => "lightsail:loadbalancer", "Lightsail load balancer", Balancing, LIGHTSAIL;
    LightsailInstance => "lightsail:instance", "Lightsail instance", Compute, LIGHTSAIL;
    LightsailDisk => "lightsail:disk", "Lightsail disk", Storage, LIGHTSAIL;
    LightsailDatabase => "lightsail:database", "Lightsail database", Data, LIGHTSAIL;
    LightsailStaticIp => "lightsail:static-ip", "Lightsail static IP", Network, LIGHTSAIL;
}

impl ResourceType {
    /// Plumbing that connects to almost everything; hidden from the graph by default.
    pub fn is_network_plumbing(self) -> bool {
        matches!(
            self,
            Self::Vpc | Self::Subnet | Self::SecurityGroup | Self::NetworkInterface
        )
    }

    /// Shown in the graph only when explicitly selected by type: there can be
    /// thousands and they connect to nothing running.
    pub fn is_bulk(self) -> bool {
        matches!(self, Self::EbsSnapshot)
    }

    /// Whether AWS lets this resource carry tags. Lightsail static IPs can't,
    /// so they never count as "untagged".
    pub fn is_taggable(self) -> bool {
        !matches!(self, Self::LightsailStaticIp)
    }

    pub fn is_lightsail(self) -> bool {
        matches!(
            self,
            Self::LightsailInstance
                | Self::LightsailDatabase
                | Self::LightsailLoadBalancer
                | Self::LightsailDisk
                | Self::LightsailStaticIp
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Edge,
    Balancing,
    Routing,
    Compute,
    Storage,
    Data,
    Messaging,
    Network,
}

/// A discovered AWS resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    /// ARN; synthesized for resource types whose API does not return one.
    pub arn: String,
    pub resource_type: ResourceType,
    pub account_id: String,
    pub region: String,
    /// Service-native ID, e.g. `i-0abc…` or the bucket name.
    pub native_id: String,
    pub name: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// Type-specific details shown in the UI (state, size, instance type…).
    pub attrs: BTreeMap<String, serde_json::Value>,
}

impl Resource {
    pub fn new(
        resource_type: ResourceType,
        arn: impl Into<String>,
        account_id: &str,
        region: &str,
        native_id: impl Into<String>,
    ) -> Self {
        Self {
            arn: arn.into(),
            resource_type,
            account_id: account_id.to_owned(),
            region: region.to_owned(),
            native_id: native_id.into(),
            name: None,
            tags: BTreeMap::new(),
            attrs: BTreeMap::new(),
        }
    }

    /// Sets `attr` when the value is present.
    pub fn attr(&mut self, key: &str, value: impl Into<Option<serde_json::Value>>) -> &mut Self {
        if let Some(v) = value.into().filter(|v| !v.is_null()) {
            self.attrs.insert(key.to_owned(), v);
        }
        self
    }

    pub fn with_tags<'a>(
        &mut self,
        tags: impl IntoIterator<Item = (Option<&'a str>, Option<&'a str>)>,
    ) -> &mut Self {
        for (k, v) in tags {
            if let Some(k) = k {
                self.tags
                    .insert(k.to_owned(), v.unwrap_or_default().to_owned());
            }
        }
        // The `Name` tag is the conventional display name.
        if self.name.is_none() {
            self.name = self.tags.get("Name").filter(|n| !n.is_empty()).cloned();
        }
        self
    }

    pub fn vpc_id(&self) -> Option<&str> {
        self.attrs.get("vpc_id").and_then(|v| v.as_str())
    }

    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.native_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    /// Traffic flows from → to (CloudFront → origin, LB → target group → target).
    RoutesTo,
    /// Storage or address attached to a resource (instance → volume, Elastic IP).
    Attached,
    /// An event source invokes a function (queue → Lambda).
    Triggers,
    /// Topic subscription (topic → queue/function).
    PublishesTo,
    /// Placement in a subnet or VPC.
    InNetwork,
    SecuredBy,
    /// Membership in a cluster.
    MemberOf,
    /// A network interface belongs to its owner.
    Owns,
}

/// Relationship from one resource to another, by ARN or by DNS name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Relation {
    pub from: String,
    /// An ARN, or `dns:<name>` resolved against resources' `dns_name` attribute.
    pub to: String,
    pub kind: RelationKind,
}

impl Relation {
    pub fn new(from: &str, to: impl Into<String>, kind: RelationKind) -> Self {
        Self {
            from: from.to_owned(),
            to: to.into(),
            kind,
        }
    }

    pub fn to_dns(from: &str, dns_name: &str, kind: RelationKind) -> Self {
        Self::new(from, format!("dns:{}", normalize_dns(dns_name)), kind)
    }
}

pub(crate) fn normalize_dns(name: &str) -> String {
    name.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// What one collector run found.
#[derive(Debug, Default)]
pub struct Collected {
    pub resources: Vec<Resource>,
    pub relations: Vec<Relation>,
    pub warnings: Vec<String>,
    /// Tags from the tagging API, applied once all regions are merged.
    pub tags_by_arn: std::collections::HashMap<String, BTreeMap<String, String>>,
    /// CloudFormation stacks; their members are annotated once all regions
    /// are merged (a stack in one region can manage an S3 bucket listed globally).
    pub stacks: Vec<collect::cloudformation::Stack>,
}

impl Collected {
    pub fn extend(&mut self, other: Collected) {
        self.resources.extend(other.resources);
        self.relations.extend(other.relations);
        self.warnings.extend(other.warnings);
        self.tags_by_arn.extend(other.tags_by_arn);
        self.stacks.extend(other.stacks);
    }

    /// Marks resources that a stack manages (see [`iac`]). A physical ID is
    /// matched against the ARN, the native ID, the name or the queue URL;
    /// stacks in the resource's own region win over others.
    pub fn apply_stacks(&mut self, stacks: &[collect::cloudformation::Stack]) {
        let mut by_id: std::collections::HashMap<&str, Vec<(&collect::cloudformation::Stack, &str)>> =
            std::collections::HashMap::new();
        for st in stacks {
            for (physical, logical) in &st.members {
                by_id.entry(physical.as_str()).or_default().push((st, logical.as_str()));
            }
        }
        for r in &mut self.resources {
            let keys = [
                Some(r.arn.as_str()),
                Some(r.native_id.as_str()),
                r.name.as_deref(),
                r.attrs.get("url").and_then(|v| v.as_str()),
            ];
            let found = keys.into_iter().flatten().find_map(|k| {
                let hits = by_id.get(k)?;
                hits.iter()
                    .find(|(st, _)| st.region == r.region)
                    .or_else(|| (hits.len() == 1).then(|| &hits[0]))
                    .copied()
            });
            if let Some((st, logical)) = found {
                let (stack, tool, reason) = (st.name.clone(), st.tool.id(), st.reason.clone());
                r.attr(iac::ATTR_STACK, Some(serde_json::Value::String(stack)))
                    .attr(iac::ATTR_LOGICAL_ID, Some(serde_json::Value::String(logical.to_owned())))
                    .attr(iac::ATTR_TOOL, Some(serde_json::Value::String(tool.to_owned())))
                    .attr(iac::ATTR_TOOL_REASON, reason.map(serde_json::Value::String));
            }
        }
    }

    /// Fills tags (and the `Name`-based display name) from the tagging API.
    pub fn apply_tags(&mut self) {
        for r in &mut self.resources {
            if let Some(t) = self.tags_by_arn.get(&r.arn) {
                for (k, v) in t {
                    r.tags.entry(k.clone()).or_insert_with(|| v.clone());
                }
                if r.name.is_none() {
                    r.name = r.tags.get("Name").filter(|n| !n.is_empty()).cloned();
                }
            }
        }
    }

    pub(crate) fn relate(&mut self, from: &str, to: impl Into<String>, kind: RelationKind) {
        self.relations.push(Relation::new(from, to, kind));
    }
}

#[cfg(test)]
mod tests {
    use super::collect::cloudformation::Stack;
    use super::iac::IacTool;
    use super::*;

    #[test]
    fn stack_members_are_matched_by_arn_id_name_or_url() {
        let stack = |region: &str, members: &[(&str, &str)]| Stack {
            name: format!("api-{region}"),
            region: region.into(),
            tool: IacTool::Cdk,
            reason: Some("the stack has CDK metadata".into()),
            members: members
                .iter()
                .map(|(p, l)| ((*p).to_owned(), (*l).to_owned()))
                .collect(),
        };
        let mut found = Collected::default();
        let mut queue = Resource::new(ResourceType::SqsQueue, "arn:aws:sqs:us-east-1:1:jobs", "1", "us-east-1", "jobs");
        queue.attr("url", Some("https://sqs.us-east-1.amazonaws.com/1/jobs".into()));
        found.resources = vec![
            queue,
            // Same function name in two regions: the stack in its own region wins.
            Resource::new(ResourceType::LambdaFunction, "arn:fn-eu", "1", "eu-west-1", "handler"),
            Resource::new(ResourceType::S3Bucket, "arn:aws:s3:::assets", "1", "us-east-1", "assets"),
            Resource::new(ResourceType::S3Bucket, "arn:aws:s3:::other", "1", "us-east-1", "other"),
        ];
        found.apply_stacks(&[
            stack(
                "us-east-1",
                &[
                    ("https://sqs.us-east-1.amazonaws.com/1/jobs", "JobsQueueA1B2C3D4"),
                    ("handler", "HandlerUs"),
                    ("assets", "Assets"),
                ],
            ),
            stack("eu-west-1", &[("handler", "HandlerEu")]),
        ]);
        let attr = |i: usize, k: &str| {
            found.resources[i]
                .attrs
                .get(k)
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        };
        assert_eq!(attr(0, iac::ATTR_LOGICAL_ID).as_deref(), Some("JobsQueueA1B2C3D4"));
        assert_eq!(attr(1, iac::ATTR_STACK).as_deref(), Some("api-eu-west-1"));
        assert_eq!(attr(2, iac::ATTR_TOOL).as_deref(), Some("cdk"));
        assert_eq!(attr(3, iac::ATTR_STACK), None);
    }
}
