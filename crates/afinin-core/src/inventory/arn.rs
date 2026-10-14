// SPDX-License-Identifier: MIT OR Apache-2.0

//! ARN helpers.

/// AWS partition for a region.
pub fn partition(region: &str) -> &'static str {
    if region.starts_with("cn-") {
        "aws-cn"
    } else if region.starts_with("us-gov-") {
        "aws-us-gov"
    } else {
        "aws"
    }
}

/// ARN of an EC2 resource, e.g. `instance/i-0abc`.
pub fn ec2(region: &str, account: &str, kind: &str, id: &str) -> String {
    format!(
        "arn:{}:ec2:{region}:{account}:{kind}/{id}",
        partition(region)
    )
}

pub fn s3_bucket(region: &str, bucket: &str) -> String {
    format!("arn:{}:s3:::{bucket}", partition(region))
}

/// `https://sqs.<region>.amazonaws.com/<account>/<name>` → queue ARN.
pub fn sqs_from_url(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1)?;
    let mut parts = rest.split('/');
    let host = parts.next()?;
    let account = parts.next()?;
    let name = parts.next()?;
    let region = host.split('.').nth(1).filter(|r| *r != "amazonaws")?;
    Some(format!(
        "arn:{}:sqs:{region}:{account}:{name}",
        partition(region)
    ))
}

/// DynamoDB stream ARN (`…:table/T/stream/2024…`) → table ARN.
pub fn dynamodb_table_from_stream(stream_arn: &str) -> String {
    match stream_arn.find("/stream/") {
        Some(i) => stream_arn[..i].to_owned(),
        None => stream_arn.to_owned(),
    }
}

/// Lambda ARN inside an API Gateway integration URI such as
/// `arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/<lambda arn>/invocations`.
pub fn lambda_from_integration_uri(uri: &str) -> Option<String> {
    if uri.starts_with("arn:")
        && uri.contains(":lambda:")
        && uri.contains(":function:")
        && !uri.contains("/functions/")
    {
        // Already a function ARN (HTTP APIs accept it directly).
        return Some(strip_lambda_qualifier(uri));
    }
    let start = uri.find("/functions/")? + "/functions/".len();
    let rest = &uri[start..];
    let end = rest.find("/invocations").unwrap_or(rest.len());
    Some(strip_lambda_qualifier(&rest[..end]))
}

/// Drops a version/alias suffix: `…:function:name:prod` → `…:function:name`.
pub fn strip_lambda_qualifier(arn: &str) -> String {
    let parts: Vec<&str> = arn.split(':').collect();
    if parts.len() > 7 {
        parts[..7].join(":")
    } else {
        arn.to_owned()
    }
}

/// Bucket name from an S3 origin domain like `b.s3.amazonaws.com` or
/// `b.s3.eu-west-1.amazonaws.com` (website endpoints included).
pub fn s3_bucket_from_domain(domain: &str) -> Option<&str> {
    let d = domain.trim_end_matches('.');
    for marker in [".s3.", ".s3-website.", ".s3-website-"] {
        if let Some(i) = d.find(marker)
            && d.ends_with(".amazonaws.com")
        {
            return Some(&d[..i]);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_arns_and_domains() {
        assert_eq!(
            ec2("eu-west-1", "1", "instance", "i-1"),
            "arn:aws:ec2:eu-west-1:1:instance/i-1"
        );
        assert_eq!(partition("cn-north-1"), "aws-cn");
        assert_eq!(
            sqs_from_url("https://sqs.us-east-1.amazonaws.com/123/jobs").as_deref(),
            Some("arn:aws:sqs:us-east-1:123:jobs")
        );
        assert_eq!(
            dynamodb_table_from_stream(
                "arn:aws:dynamodb:us-east-1:1:table/T/stream/2026-01-01T00:00:00.000"
            ),
            "arn:aws:dynamodb:us-east-1:1:table/T"
        );
        let f = "arn:aws:lambda:us-east-1:1:function:api";
        assert_eq!(
            lambda_from_integration_uri(&format!(
                "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/{f}:live/invocations"
            ))
            .as_deref(),
            Some(f)
        );
        assert_eq!(lambda_from_integration_uri(f).as_deref(), Some(f));
        assert_eq!(
            s3_bucket_from_domain("assets.s3.amazonaws.com"),
            Some("assets")
        );
        assert_eq!(
            s3_bucket_from_domain("assets.s3.eu-west-1.amazonaws.com."),
            Some("assets")
        );
        assert_eq!(s3_bucket_from_domain("example.com"), None);
    }
}
