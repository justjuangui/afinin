# SPDX-License-Identifier: MIT OR Apache-2.0

variable "trusted_principal_arns" {
  description = "Principals allowed to assume the role (IAM Identity Center role, management account root…)."
  type        = list(string)
}

variable "role_name" {
  description = "Name of the read-only role."
  type        = string
  default     = "AfininReadOnly"
}

variable "external_id" {
  description = "Optional external ID required to assume the role."
  type        = string
  default     = null
}

variable "tags" {
  description = "Tags for the role."
  type        = map(string)
  default     = {}
}
