// SPDX-License-Identifier: MIT OR Apache-2.0

//! Redaction layer (FR-44): replaces account IDs, ARNs, resource IDs, IP
//! addresses, host names and known tag values / resource names with reversible
//! placeholders before anything reaches a cloud LLM, and restores them in the
//! answer shown to the user. One `Redactor` lives for a whole conversation so
//! placeholders stay consistent across turns.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

static ARN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"arn:aws[a-z-]*:[a-z0-9-]+:[a-z0-9-]*:[0-9]{0,12}:[^\s"'`,;)\]}>]+"#).unwrap()
});
static HOST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+(?:amazonaws\.com|cloudfront\.net|awsapps\.com|com|net|org|io|dev|app|co|cloud|internal|local|ai|us|eu|uk|de|es|mx|co\.uk)\b",
    )
    .unwrap()
});
static RESOURCE_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:i|vol|snap|sg|subnet|vpc|eni|nat|eipalloc|eipassoc|igw|rtb|acl|ami|lt|tgw|tgw-attach|vpce|pcx|fs|fsap|elbv2|r|cgw|vgw|dopt)-[0-9a-f]{8,17}\b",
    )
    .unwrap()
});
static IPV4: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:25[0-5]|2[0-4]\d|1?\d?\d)(?:\.(?:25[0-5]|2[0-4]\d|1?\d?\d)){3}\b").unwrap()
});
static ACCOUNT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{12}\b").unwrap());
static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:ACCOUNT|ARN|RESOURCE|IP|HOST|NAME)_\d+\b").unwrap());

#[derive(Debug, Clone, Default)]
pub struct Redactor {
    enabled: bool,
    to_placeholder: HashMap<String, String>,
    to_original: HashMap<String, String>,
    counters: HashMap<&'static str, usize>,
    /// Tag values and resource names, longest first.
    names: Vec<String>,
    names_re: Option<Regex>,
}

impl Redactor {
    /// `names` are workspace-specific strings to hide (tag values, resource names).
    pub fn new(enabled: bool, names: impl IntoIterator<Item = String>) -> Self {
        let mut names: Vec<String> = names
            .into_iter()
            .map(|n| n.trim().to_owned())
            // Very short values ("1", "on") would mask ordinary words.
            .filter(|n| n.chars().count() >= 3 && !n.chars().all(|c| c.is_ascii_digit()))
            .collect();
        names.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        names.dedup();
        let names_re = (!names.is_empty()).then(|| {
            let alternation = names
                .iter()
                .map(|n| regex::escape(n))
                .collect::<Vec<_>>()
                .join("|");
            Regex::new(&format!(
                r"(?:^|\b|[^A-Za-z0-9_-])({alternation})(?:$|\b|[^A-Za-z0-9_-])"
            ))
            .unwrap()
        });
        Self {
            enabled,
            names,
            names_re,
            ..Default::default()
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    fn placeholder(&mut self, kind: &'static str, original: &str) -> String {
        if let Some(p) = self.to_placeholder.get(original) {
            return p.clone();
        }
        let n = self.counters.entry(kind).or_default();
        *n += 1;
        let p = format!("{kind}_{n}");
        self.to_placeholder.insert(original.to_owned(), p.clone());
        self.to_original.insert(p.clone(), original.to_owned());
        p
    }

    fn replace_all(&mut self, text: &str, re: &Regex, kind: &'static str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for m in re.find_iter(text) {
            out.push_str(&text[last..m.start()]);
            out.push_str(&self.placeholder(kind, m.as_str()));
            last = m.end();
        }
        out.push_str(&text[last..]);
        out
    }

    fn replace_names(&mut self, text: &str) -> String {
        let Some(re) = self.names_re.clone() else {
            return text.to_owned();
        };
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for caps in re.captures_iter(text) {
            let m = caps.get(1).expect("group 1 always participates");
            out.push_str(&text[last..m.start()]);
            out.push_str(&self.placeholder("NAME", m.as_str()));
            last = m.end();
        }
        out.push_str(&text[last..]);
        out
    }

    /// Masks identifiers in text sent to the model.
    pub fn mask(&mut self, text: &str) -> String {
        if !self.enabled {
            return text.to_owned();
        }
        // ARNs first: they contain account IDs and resource IDs.
        let t = self.replace_all(text, &ARN, "ARN");
        let t = self.replace_all(&t, &HOST, "HOST");
        let t = self.replace_names(&t);
        let t = self.replace_all(&t, &RESOURCE_ID, "RESOURCE");
        let t = self.replace_all(&t, &IPV4, "IP");
        self.replace_all(&t, &ACCOUNT, "ACCOUNT")
    }

    /// Restores originals in text coming back from the model.
    pub fn unmask(&self, text: &str) -> String {
        if !self.enabled {
            return text.to_owned();
        }
        PLACEHOLDER
            .replace_all(text, |c: &regex::Captures| {
                self.to_original
                    .get(&c[0])
                    .cloned()
                    .unwrap_or_else(|| c[0].to_owned())
            })
            .into_owned()
    }

    /// Masks every string value (not keys) of a JSON document.
    pub fn mask_json(&mut self, v: &serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        match v {
            Value::String(s) => Value::String(self.mask(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.mask_json(x)).collect()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, x)| (k.clone(), self.mask_json(x)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    pub fn unmask_json(&self, v: &serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        match v {
            Value::String(s) => Value::String(self.unmask(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.unmask_json(x)).collect()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, x)| (k.clone(), self.unmask_json(x)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// Number of distinct values masked so far.
    pub fn masked_count(&self) -> usize {
        self.to_original.len()
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_and_restores_identifiers() {
        let mut r = Redactor::new(
            true,
            ["checkout-api".to_owned(), "prod".to_owned(), "x".to_owned()],
        );
        let text = "Account 123456789012 runs i-0abc12345678def90 (checkout-api, env prod) behind \
                    arn:aws:elasticloadbalancing:us-east-1:123456789012:loadbalancer/app/web/50dc6c495c0c9188 \
                    at web-123.us-east-1.elb.amazonaws.com and 10.0.3.17. Production stays.";
        let masked = r.mask(text);
        for secret in [
            "123456789012",
            "i-0abc12345678def90",
            "checkout-api",
            "loadbalancer/app/web",
            "elb.amazonaws.com",
            "10.0.3.17",
        ] {
            assert!(!masked.contains(secret), "{secret} leaked: {masked}");
        }
        assert!(
            masked.contains("Production stays"),
            "names match whole words only: {masked}"
        );
        assert!(
            masked.contains("ACCOUNT_1") && masked.contains("ARN_1") && masked.contains("HOST_1")
        );
        assert_eq!(r.unmask(&masked), text);

        // Same value → same placeholder across calls; services are untouched.
        assert_eq!(
            r.mask("cost of 123456789012 in Amazon EC2"),
            "cost of ACCOUNT_1 in Amazon EC2"
        );

        let json = serde_json::json!({"account_id": "123456789012", "amount": 12.5, "tags": {"app": "checkout-api"}});
        let m = r.mask_json(&json);
        assert_eq!(m["account_id"], "ACCOUNT_1");
        assert_eq!(m["amount"], 12.5);
        assert_eq!(r.unmask_json(&m), json);
    }

    #[test]
    fn disabled_is_a_no_op() {
        let mut r = Redactor::new(false, []);
        assert_eq!(r.mask("123456789012"), "123456789012");
    }
}
