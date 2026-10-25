// SPDX-License-Identifier: MIT OR Apache-2.0

//! CloudFormation stacks and the resources they manage, to tell which
//! resources were made as code and with which tool (CDK, Serverless…).
//! Stacks are not resources in the inventory: their members are annotated.

use aws_sdk_cloudformation::types::StackStatus;

use super::{Ctx, aws_err};
use crate::error::Result;
use crate::inventory::Collected;
use crate::inventory::iac::IacTool;

/// A live stack and what it manages.
#[derive(Debug, Clone, PartialEq)]
pub struct Stack {
    pub name: String,
    pub region: String,
    /// The tool that made it: CloudFormation, or CDK, Serverless, Elastic
    /// Beanstalk or Amplify, which deploy through CloudFormation.
    pub tool: IacTool,
    /// Why `tool` (e.g. "the stack has CDK metadata").
    pub reason: Option<String>,
    /// Each managed resource.
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub physical_id: String,
    pub logical_id: String,
    /// CloudFormation type, e.g. `AWS::Lambda::Function`.
    pub cfn_type: String,
}

/// Stack flavour from its name and its resources' (logical ID, type).
/// `root_cdk`: the root stack of a nested stack has CDK metadata.
pub fn classify(name: &str, resources: &[(String, String)], root_cdk: bool) -> (IacTool, Option<String>) {
    if root_cdk || resources.iter().any(|(_, t)| t == "AWS::CDK::Metadata") {
        return (IacTool::Cdk, Some("the stack has CDK metadata".into()));
    }
    if resources.iter().any(|(l, _)| l == "ServerlessDeploymentBucket") {
        return (
            IacTool::Serverless,
            Some("the stack has a ServerlessDeploymentBucket".into()),
        );
    }
    if name.starts_with("awseb-") {
        return (
            IacTool::ElasticBeanstalk,
            Some("Elastic Beanstalk names its stacks awseb-…".into()),
        );
    }
    if name.starts_with("amplify-") {
        return (IacTool::Amplify, Some("Amplify names its stacks amplify-…".into()));
    }
    (IacTool::CloudFormation, None)
}

/// Stack ID, name, root stack ID and (logical ID, physical ID, type) of
/// each resource.
type Listed = (String, String, Option<String>, Vec<(String, String, String)>);

pub async fn collect(ctx: &Ctx<'_>) -> Result<Collected> {
    let client = aws_sdk_cloudformation::Client::new(ctx.config);
    let mut summaries = Vec::new();
    for page in client
        .list_stacks()
        .into_paginator()
        .send()
        .try_collect()
        .await
        .map_err(aws_err)?
    {
        for s in page.stack_summaries() {
            if s.stack_status() == Some(&StackStatus::DeleteComplete) {
                continue;
            }
            let (Some(id), Some(name)) = (s.stack_id(), s.stack_name()) else {
                continue;
            };
            summaries.push((id.to_owned(), name.to_owned(), s.root_id().map(str::to_owned)));
        }
    }

    let mut listed: Vec<Listed> = Vec::new();
    let mut out = Collected::default();
    for (id, name, root) in summaries {
        let pages = client
            .list_stack_resources()
            .stack_name(&id)
            .into_paginator()
            .send()
            .try_collect()
            .await;
        let pages = match pages {
            Ok(p) => p,
            // A stack deleted mid-scan, or one we may not read: skip it.
            Err(e) => {
                out.warnings.push(format!(
                    "cloudformation in {}: stack {name}: {}",
                    ctx.region,
                    aws_err(e)
                ));
                continue;
            }
        };
        let resources = pages
            .iter()
            .flat_map(|p| p.stack_resource_summaries())
            .filter_map(|r| {
                Some((
                    r.logical_resource_id()?.to_owned(),
                    r.physical_resource_id().unwrap_or_default().to_owned(),
                    r.resource_type().unwrap_or_default().to_owned(),
                ))
            })
            .collect();
        listed.push((id, name, root, resources));
    }

    let has_cdk = |stack_id: &str| {
        listed.iter().any(|(id, _, _, res)| {
            id == stack_id && res.iter().any(|(_, _, t)| t == "AWS::CDK::Metadata")
        })
    };
    for (_, name, root, resources) in &listed {
        let root_cdk = root.as_deref().is_some_and(has_cdk);
        let types: Vec<(String, String)> = resources
            .iter()
            .map(|(l, _, t)| (l.clone(), t.clone()))
            .collect();
        let (tool, reason) = classify(name, &types, root_cdk);
        out.stacks.push(Stack {
            name: name.clone(),
            region: ctx.region.to_owned(),
            tool,
            reason,
            members: resources
                .iter()
                .filter(|(_, p, _)| !p.is_empty())
                .map(|(l, p, t)| Member {
                    physical_id: p.clone(),
                    logical_id: l.clone(),
                    cfn_type: t.clone(),
                })
                .collect(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(l, t)| ((*l).to_owned(), (*t).to_owned()))
            .collect()
    }

    #[test]
    fn classifies_stacks_by_their_resources_and_names() {
        let cdk = res(&[("CDKMetadata", "AWS::CDK::Metadata"), ("Bucket83908E77", "AWS::S3::Bucket")]);
        assert_eq!(classify("api", &cdk, false).0, IacTool::Cdk);
        // A nested stack whose root has CDK metadata is CDK too.
        let plain = res(&[("Queue", "AWS::SQS::Queue")]);
        assert_eq!(classify("api-Nested-1", &plain, true).0, IacTool::Cdk);
        let sls = res(&[("ServerlessDeploymentBucket", "AWS::S3::Bucket")]);
        assert_eq!(classify("svc-dev", &sls, false).0, IacTool::Serverless);
        assert_eq!(classify("awseb-e-abc123-stack", &plain, false).0, IacTool::ElasticBeanstalk);
        assert_eq!(classify("amplify-app-dev-123", &plain, false).0, IacTool::Amplify);
        assert_eq!(classify("mkt-stack-blue", &plain, false), (IacTool::CloudFormation, None));
    }
}
