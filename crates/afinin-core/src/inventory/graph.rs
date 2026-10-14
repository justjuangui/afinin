// SPDX-License-Identifier: MIT OR Apache-2.0

//! Builds the dependency graph from stored resources and relations (FR-22..FR-25).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use super::{Relation, RelationKind, Resource, ResourceType, Tier, normalize_dns};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagFilter {
    pub key: String,
    /// `None` matches any value of the key.
    pub value: Option<String>,
    /// Match resources *without* the key (or with an empty value), the
    /// "(untagged)" group of Applications. `value` is ignored.
    #[serde(default)]
    pub missing: bool,
}

/// Graph filters (FR-23). Empty lists mean "all".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphFilter {
    pub accounts: Vec<String>,
    pub regions: Vec<String>,
    pub vpcs: Vec<String>,
    pub types: Vec<ResourceType>,
    pub tag: Option<TagFilter>,
    /// Show VPCs, subnets, security groups and network interfaces.
    pub include_network: bool,
}

impl GraphFilter {
    fn narrows(&self) -> bool {
        !self.vpcs.is_empty() || self.tag.is_some() || !self.types.is_empty()
    }

    fn matches(&self, r: &Resource) -> bool {
        (self.include_network || !r.resource_type.is_network_plumbing())
            && (!r.resource_type.is_bulk() || self.types.contains(&r.resource_type))
            && (self.accounts.is_empty() || self.accounts.contains(&r.account_id))
            && (self.regions.is_empty() || self.regions.contains(&r.region))
            && (self.vpcs.is_empty()
                || r.vpc_id().is_some_and(|v| self.vpcs.iter().any(|x| x == v)))
            && (self.types.is_empty() || self.types.contains(&r.resource_type))
            && self
                .tag
                .as_ref()
                .is_none_or(|t| match (r.tags.get(&t.key), &t.value) {
                    (v, _) if t.missing => v.is_none_or(|v| v.is_empty()),
                    (Some(_), None) => true,
                    (Some(v), Some(want)) => v == want,
                    (None, _) => false,
                })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphNode {
    pub id: String,
    pub resource_type: ResourceType,
    pub type_label: &'static str,
    pub tier: Tier,
    pub label: String,
    pub native_id: String,
    pub account_id: String,
    pub region: String,
    pub vpc_id: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub attrs: BTreeMap<String, serde_json::Value>,
    /// Monthly cost of this resource's service in its account (no CUR), of
    /// the resource itself (CUR, M3) or its list price; see `cost_basis`.
    pub cost: Option<f64>,
    /// What `cost` is; `None` when there is no cost.
    pub cost_basis: Option<CostBasis>,
    /// Included only as context for a filtered match.
    pub context: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub kind: RelationKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CostBasis {
    /// Cost Explorer: the resource's service total in its account.
    Service,
    /// CUR: the resource's own cost.
    Resource,
    /// The resource's monthly list price (Lightsail bundles).
    ListPrice,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Graph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    /// Resources stored for the workspace before filtering.
    pub total_resources: usize,
    pub cost_basis: CostBasis,
    pub unit: String,
}

/// Monthly cost lookups for the overlay (FR-24).
#[derive(Debug, Clone, Default)]
pub struct CostOverlay {
    /// (account ID, Cost Explorer service) → cost.
    pub by_service: HashMap<(String, String), f64>,
    /// ARN (or native resource ID) → cost, from CUR.
    pub by_resource: HashMap<String, f64>,
    pub unit: String,
}

/// A resource's cost and what it is: its own (CUR), else its list price,
/// else its service total in the account. Lightsail resources never take the
/// service total: one Lightsail bill on every instance and IP misleads.
fn node_cost(r: &Resource, basis: CostBasis, costs: &CostOverlay) -> Option<(f64, CostBasis)> {
    let own = (basis == CostBasis::Resource)
        .then(|| {
            costs
                .by_resource
                .get(&r.arn)
                .or_else(|| costs.by_resource.get(&r.native_id))
        })
        .flatten()
        .map(|c| (*c, CostBasis::Resource));
    let list = || {
        r.attrs
            .get("monthly_price_usd")
            .and_then(serde_json::Value::as_f64)
            .map(|c| (c, CostBasis::ListPrice))
    };
    let service = || {
        if basis == CostBasis::Resource || r.resource_type.is_lightsail() {
            return None;
        }
        r.resource_type
            .cost_service()
            .and_then(|s| costs.by_service.get(&(r.account_id.clone(), s.to_owned())))
            .map(|c| (*c, CostBasis::Service))
    };
    own.or_else(list).or_else(service)
}

/// Resolved, de-duplicated edges between existing resources.
pub(crate) fn resolve(resources: &[Resource], relations: &[Relation]) -> Vec<GraphEdge> {
    let arns: HashSet<&str> = resources.iter().map(|r| r.arn.as_str()).collect();
    let mut dns: HashMap<String, &str> = HashMap::new();
    let mut ips: HashMap<&str, &str> = HashMap::new();
    for r in resources {
        if let Some(d) = r.attrs.get("dns_name").and_then(|v| v.as_str()) {
            dns.insert(normalize_dns(d), &r.arn);
        }
        if let Some(ip) = r.attrs.get("private_ip").and_then(|v| v.as_str()) {
            // An instance wins over its own network interface.
            if r.resource_type == ResourceType::Ec2Instance || !ips.contains_key(ip) {
                ips.insert(ip, &r.arn);
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut edges = Vec::new();
    for rel in relations {
        let target = if let Some(name) = rel.to.strip_prefix("dns:") {
            dns.get(name).copied()
        } else if let Some(ip) = rel.to.strip_prefix("ip:") {
            ips.get(ip).copied()
        } else {
            arns.get(rel.to.as_str()).copied()
        };
        let Some(target) = target else { continue };
        if !arns.contains(rel.from.as_str()) || rel.from == target {
            continue;
        }
        if seen.insert((rel.from.clone(), target.to_owned(), rel.kind)) {
            edges.push(GraphEdge {
                source: rel.from.clone(),
                target: target.to_owned(),
                kind: rel.kind,
            });
        }
    }
    edges
}

pub fn build(
    resources: &[Resource],
    relations: &[Relation],
    filter: &GraphFilter,
    costs: &CostOverlay,
) -> Graph {
    let edges = resolve(resources, relations);
    let by_arn: HashMap<&str, &Resource> = resources.iter().map(|r| (r.arn.as_str(), r)).collect();

    let matched: HashSet<&str> = resources
        .iter()
        .filter(|r| filter.matches(r))
        .map(|r| r.arn.as_str())
        .collect();
    // With narrowing filters, keep direct neighbors as context so a VPC's
    // resources still show what reaches them (CloudFront, S3, Lambda…).
    let mut context: HashSet<&str> = HashSet::new();
    if filter.narrows() {
        for e in &edges {
            for (a, b) in [(&e.source, &e.target), (&e.target, &e.source)] {
                if matched.contains(a.as_str()) && !matched.contains(b.as_str()) {
                    let r = by_arn[b.as_str()];
                    if filter.include_network || !r.resource_type.is_network_plumbing() {
                        context.insert(b.as_str());
                    }
                }
            }
        }
    }

    let cost_basis = if costs.by_resource.is_empty() {
        CostBasis::Service
    } else {
        CostBasis::Resource
    };
    let nodes: Vec<GraphNode> = resources
        .iter()
        .filter(|r| matched.contains(r.arn.as_str()) || context.contains(r.arn.as_str()))
        .map(|r| GraphNode {
            id: r.arn.clone(),
            resource_type: r.resource_type,
            type_label: r.resource_type.label(),
            tier: r.resource_type.tier(),
            label: r.display_name().to_owned(),
            native_id: r.native_id.clone(),
            account_id: r.account_id.clone(),
            region: r.region.clone(),
            vpc_id: r.vpc_id().map(str::to_owned),
            tags: r.tags.clone(),
            attrs: r.attrs.clone(),
            cost: node_cost(r, cost_basis, costs).map(|(c, _)| c),
            cost_basis: node_cost(r, cost_basis, costs).map(|(_, b)| b),
            context: !matched.contains(r.arn.as_str()),
        })
        .collect();
    let visible: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let edges = edges
        .into_iter()
        .filter(|e| visible.contains(e.source.as_str()) && visible.contains(e.target.as_str()))
        .collect();

    Graph {
        nodes,
        edges,
        total_resources: resources.len(),
        cost_basis,
        unit: if costs.unit.is_empty() {
            "USD".into()
        } else {
            costs.unit.clone()
        },
    }
}

/// Resources within `depth` hops of `arn`, ignoring direction (FR-41 "what is
/// connected to this RDS instance?"). Network plumbing is followed only when asked.
pub fn neighbors<'a>(
    resources: &'a [Resource],
    relations: &[Relation],
    arn: &str,
    depth: usize,
    include_network: bool,
) -> Vec<(&'a Resource, usize)> {
    let by_arn: HashMap<&str, &Resource> = resources.iter().map(|r| (r.arn.as_str(), r)).collect();
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for e in resolve(resources, relations) {
        adj.entry(e.source.clone())
            .or_default()
            .push(e.target.clone());
        adj.entry(e.target).or_default().push(e.source);
    }
    let mut out = Vec::new();
    let mut seen = HashSet::from([arn.to_owned()]);
    let mut queue = VecDeque::from([(arn.to_owned(), 0)]);
    while let Some((cur, d)) = queue.pop_front() {
        if d == depth {
            continue;
        }
        for next in adj.get(&cur).into_iter().flatten() {
            let Some(r) = by_arn.get(next.as_str()) else {
                continue;
            };
            if (!include_network && r.resource_type.is_network_plumbing())
                || !seen.insert(next.clone())
            {
                continue;
            }
            out.push((*r, d + 1));
            queue.push_back((next.clone(), d + 1));
        }
    }
    out
}

/// Resources grouped by the value of a tag key (FR-25).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Application {
    /// Tag value; empty for untagged resources.
    pub value: String,
    pub resource_count: usize,
    pub resource_types: BTreeMap<ResourceType, usize>,
    /// Last 30 days, when the tag key is synced from Cost Explorer.
    pub cost: Option<f64>,
}

pub fn applications(resources: &[Resource], tag_key: &str) -> Vec<Application> {
    let mut groups: BTreeMap<String, Application> = BTreeMap::new();
    for r in resources
        .iter()
        .filter(|r| !r.resource_type.is_network_plumbing() && !r.resource_type.is_bulk())
    {
        let value = r.tags.get(tag_key).cloned().unwrap_or_default();
        let app = groups.entry(value.clone()).or_insert_with(|| Application {
            value,
            resource_count: 0,
            resource_types: BTreeMap::new(),
            cost: None,
        });
        app.resource_count += 1;
        *app.resource_types.entry(r.resource_type).or_default() += 1;
    }
    groups.into_values().collect()
}

/// Values to filter the graph by, with resource counts (FR-23).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct InventoryFacets {
    pub accounts: Vec<Facet>,
    pub regions: Vec<Facet>,
    pub vpcs: Vec<Facet>,
    pub types: Vec<Facet>,
    pub tag_keys: Vec<Facet>,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Facet {
    pub key: String,
    pub label: String,
    pub count: usize,
}

pub fn facets(resources: &[Resource]) -> InventoryFacets {
    fn collect(counts: BTreeMap<String, (String, usize)>) -> Vec<Facet> {
        let mut v: Vec<Facet> = counts
            .into_iter()
            .map(|(key, (label, count))| Facet { key, label, count })
            .collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));
        v
    }
    let (mut accounts, mut regions, mut vpcs, mut types, mut tags) = (
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    );
    let vpc_names: HashMap<&str, &str> = resources
        .iter()
        .filter(|r| r.resource_type == ResourceType::Vpc)
        .filter_map(|r| Some((r.native_id.as_str(), r.name.as_deref()?)))
        .collect();
    for r in resources {
        let bump = |m: &mut BTreeMap<String, (String, usize)>, k: &str, label: String| {
            m.entry(k.to_owned()).or_insert((label, 0)).1 += 1;
        };
        bump(&mut accounts, &r.account_id, r.account_id.clone());
        bump(&mut regions, &r.region, r.region.clone());
        bump(
            &mut types,
            r.resource_type.id(),
            r.resource_type.label().to_owned(),
        );
        if let Some(v) = r.vpc_id() {
            let label = vpc_names
                .get(v)
                .map_or_else(|| v.to_owned(), |n| format!("{n} ({v})"));
            bump(&mut vpcs, v, label);
        }
        for k in r
            .tags
            .keys()
            .filter(|k| !k.starts_with("aws:") && *k != "Name")
        {
            bump(&mut tags, k, k.clone());
        }
    }
    InventoryFacets {
        accounts: collect(accounts),
        regions: collect(regions),
        vpcs: collect(vpcs),
        types: collect(types),
        tag_keys: collect(tags),
        total: resources.len(),
    }
}

/// Distinct values of a tag key, for the tag filter.
pub fn tag_values(resources: &[Resource], key: &str) -> Vec<Facet> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for v in resources.iter().filter_map(|r| r.tags.get(key)) {
        *counts.entry(v.clone()).or_default() += 1;
    }
    let mut out: Vec<Facet> = counts
        .into_iter()
        .map(|(k, count)| Facet {
            label: if k.is_empty() {
                "(empty)".into()
            } else {
                k.clone()
            },
            key: k,
            count,
        })
        .collect();
    out.sort_by_key(|f| std::cmp::Reverse(f.count));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{RelationKind as K, ResourceType as T, arn};
    use serde_json::json;

    const A: &str = "111111111111";
    const R: &str = "us-east-1";

    fn res(t: T, arn: &str) -> Resource {
        Resource::new(t, arn, A, R, arn.rsplit(['/', ':']).next().unwrap())
    }

    /// CloudFront → ALB (by DNS) → target group → EC2 (by IP) → EBS, plus network plumbing.
    fn fixture() -> (Vec<Resource>, Vec<Relation>) {
        let alb = "arn:aws:elasticloadbalancing:us-east-1:1:loadbalancer/app/web/abc";
        let tg = "arn:aws:elasticloadbalancing:us-east-1:1:targetgroup/web/def";
        let cf = "arn:aws:cloudfront::1:distribution/E1";
        let i = arn::ec2(R, A, "instance", "i-1");
        let vol = arn::ec2(R, A, "volume", "vol-1");
        let subnet = arn::ec2(R, A, "subnet", "subnet-1");
        let vpc = arn::ec2(R, A, "vpc", "vpc-1");
        let orphan = arn::ec2(R, A, "volume", "vol-2");

        let mut alb_r = res(T::LoadBalancer, alb);
        alb_r
            .attr(
                "dns_name",
                Some(json!("Web-123.us-east-1.elb.amazonaws.com")),
            )
            .attr("vpc_id", Some(json!("vpc-1")));
        let mut inst = res(T::Ec2Instance, &i);
        inst.attr("private_ip", Some(json!("10.0.0.5")))
            .attr("vpc_id", Some(json!("vpc-1")));
        inst.tags.insert("app".into(), "shop".into());
        let mut volume = res(T::EbsVolume, &vol);
        volume.tags.insert("app".into(), "shop".into());

        let resources = vec![
            res(T::CloudFrontDistribution, cf),
            alb_r,
            res(T::TargetGroup, tg),
            inst,
            volume,
            res(T::Subnet, &subnet),
            res(T::Vpc, &vpc),
            res(T::EbsVolume, &orphan),
        ];
        let relations = vec![
            Relation::to_dns(cf, "web-123.us-east-1.elb.amazonaws.com.", K::RoutesTo),
            Relation::new(alb, tg, K::RoutesTo),
            Relation::new(tg, "ip:10.0.0.5", K::RoutesTo),
            Relation::new(&i, &vol, K::Attached),
            Relation::new(&i, &vol, K::Attached), // reported by both sides
            Relation::new(&i, &subnet, K::InNetwork),
            Relation::new(&subnet, &vpc, K::InNetwork),
            Relation::new(
                &i,
                arn::ec2(R, A, "security-group", "sg-gone"),
                K::SecuredBy,
            ),
        ];
        (resources, relations)
    }

    #[test]
    fn resolves_alb_to_ebs_chain() {
        let (resources, relations) = fixture();
        let mut costs = CostOverlay::default();
        costs.by_service.insert(
            (A.into(), "Amazon Elastic Compute Cloud - Compute".into()),
            250.0,
        );
        let g = build(&resources, &relations, &GraphFilter::default(), &costs);

        // Plumbing hidden by default; dangling SG edge dropped; duplicate merged.
        assert_eq!(g.nodes.len(), 6);
        let kinds: Vec<(&str, &str)> = g
            .edges
            .iter()
            .map(|e| {
                (
                    e.source.rsplit(['/', ':']).next().unwrap(),
                    e.target.rsplit(['/', ':']).next().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("E1", "abc"),
                ("abc", "def"),
                ("def", "i-1"),
                ("i-1", "vol-1")
            ]
        );
        let inst = g.nodes.iter().find(|n| n.native_id == "i-1").unwrap();
        assert_eq!(inst.cost, Some(250.0));
        assert_eq!(g.cost_basis, CostBasis::Service);

        let with_net = build(
            &resources,
            &relations,
            &GraphFilter {
                include_network: true,
                ..Default::default()
            },
            &costs,
        );
        assert_eq!(with_net.nodes.len(), 8);
        assert_eq!(with_net.edges.len(), 6);
    }

    #[test]
    fn narrowing_filters_keep_context() {
        let (resources, relations) = fixture();
        let filter = GraphFilter {
            tag: Some(TagFilter {
                key: "app".into(),
                value: Some("shop".into()),
                missing: false,
            }),
            ..Default::default()
        };
        let g = build(&resources, &relations, &filter, &CostOverlay::default());
        let mut ids: Vec<(&str, bool)> = g
            .nodes
            .iter()
            .map(|n| (n.native_id.as_str(), n.context))
            .collect();
        ids.sort();
        assert_eq!(ids, vec![("def", true), ("i-1", false), ("vol-1", false)]);

        let vpc = build(
            &resources,
            &relations,
            &GraphFilter {
                vpcs: vec!["vpc-1".into()],
                ..Default::default()
            },
            &CostOverlay::default(),
        );
        assert!(vpc.nodes.iter().any(|n| n.native_id == "E1" && n.context));
    }

    #[test]
    fn lightsail_uses_list_prices_not_the_service_total() {
        let ls = |t: ResourceType, name: &str, price: Option<f64>| {
            let mut r = Resource::new(
                t,
                format!("arn:aws:lightsail:us-east-1:{A}:{name}"),
                A,
                "us-east-1",
                name,
            );
            r.attr("monthly_price_usd", price.map(|p| serde_json::json!(p)));
            r
        };
        let resources = vec![
            ls(ResourceType::LightsailInstance, "web", Some(10.0)),
            ls(ResourceType::LightsailStaticIp, "ip-attached", Some(0.0)),
            ls(ResourceType::LightsailDisk, "data", None),
            Resource::new(
                ResourceType::Ec2Instance,
                "arn:aws:ec2:us-east-1:1:instance/i-9",
                A,
                "us-east-1",
                "i-9",
            ),
        ];
        let mut costs = CostOverlay::default();
        costs
            .by_service
            .insert((A.into(), "Amazon Lightsail".into()), 554.9);
        costs.by_service.insert(
            (A.into(), "Amazon Elastic Compute Cloud - Compute".into()),
            250.0,
        );
        let g = build(&resources, &[], &GraphFilter::default(), &costs);
        let cost = |id: &str| {
            let n = g.nodes.iter().find(|n| n.native_id == id).unwrap();
            (n.cost, n.cost_basis)
        };
        assert_eq!(cost("web"), (Some(10.0), Some(CostBasis::ListPrice)));
        assert_eq!(cost("ip-attached"), (Some(0.0), Some(CostBasis::ListPrice)));
        assert_eq!(cost("data"), (None, None), "never the whole Lightsail bill");
        assert_eq!(cost("i-9"), (Some(250.0), Some(CostBasis::Service)));
    }

    #[test]
    fn untagged_filter_shows_the_untagged_application() {
        let (resources, relations) = fixture();
        let untagged = applications(&resources, "app")
            .into_iter()
            .find(|a| a.value.is_empty())
            .unwrap();
        let filter = GraphFilter {
            tag: Some(TagFilter {
                key: "app".into(),
                value: None,
                missing: true,
            }),
            ..Default::default()
        };
        let g = build(&resources, &relations, &filter, &CostOverlay::default());
        let matched: Vec<&GraphNode> = g.nodes.iter().filter(|n| !n.context).collect();
        assert_eq!(matched.len(), untagged.resource_count);
        assert!(
            matched
                .iter()
                .all(|n| n.tags.get("app").is_none_or(|v| v.is_empty()))
        );
    }

    #[test]
    fn neighbors_and_applications() {
        let (resources, relations) = fixture();
        let alb = "arn:aws:elasticloadbalancing:us-east-1:1:loadbalancer/app/web/abc";
        let n = neighbors(&resources, &relations, alb, 2, false);
        let mut names: Vec<(&str, usize)> =
            n.iter().map(|(r, d)| (r.native_id.as_str(), *d)).collect();
        names.sort();
        assert_eq!(names, vec![("E1", 1), ("def", 1), ("i-1", 2)]);

        let apps = applications(&resources, "app");
        let shop = apps.iter().find(|a| a.value == "shop").unwrap();
        assert_eq!(shop.resource_count, 2);
        let untagged = apps.iter().find(|a| a.value.is_empty()).unwrap();
        assert_eq!(untagged.resource_count, 4);

        let f = facets(&resources);
        assert_eq!(f.total, 8);
        assert_eq!(f.tag_keys[0].key, "app");
        assert_eq!(f.vpcs[0].count, 2);
        assert_eq!(tag_values(&resources, "app")[0].count, 2);
    }
}
