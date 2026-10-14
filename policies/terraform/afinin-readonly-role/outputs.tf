# SPDX-License-Identifier: MIT OR Apache-2.0

output "role_arn" {
  description = "ARN of the role to put in the AWS profile Afinin uses (role_arn)."
  value       = aws_iam_role.afinin.arn
}
