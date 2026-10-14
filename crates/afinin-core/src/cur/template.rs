// SPDX-License-Identifier: MIT OR Apache-2.0

//! Setup material the user applies themselves (Afinín never writes to AWS).

/// CloudFormation template creating the bucket and CUR 2.0 export.
pub const CLOUDFORMATION: &str = include_str!("../../../../policies/cur-export.yaml");

/// Command to deploy [`CLOUDFORMATION`], saved as `afinin-cur-export.yaml`.
pub fn deploy_command(bucket: &str) -> String {
    let bucket = if bucket.trim().is_empty() {
        "<unique-bucket-name>"
    } else {
        bucket.trim()
    };
    format!(
        "aws cloudformation deploy --region us-east-1 --stack-name afinin-cur \\\n  \
         --template-file afinin-cur-export.yaml --parameter-overrides BucketName={bucket}"
    )
}

/// Extra read-only permissions the connected role needs for CUR.
pub fn read_policy(bucket: &str) -> String {
    let bucket = if bucket.trim().is_empty() {
        "<bucket>"
    } else {
        bucket.trim()
    };
    format!(
        r#"{{
  "Version": "2012-10-17",
  "Statement": [
    {{
      "Sid": "AfininReadCur",
      "Effect": "Allow",
      "Action": ["s3:ListBucket", "s3:GetObject"],
      "Resource": ["arn:aws:s3:::{bucket}", "arn:aws:s3:::{bucket}/*"]
    }},
    {{
      "Sid": "AfininCheckExport",
      "Effect": "Allow",
      "Action": ["bcm-data-exports:ListExports", "bcm-data-exports:GetExport"],
      "Resource": "*"
    }}
  ]
}}"#
    )
}
