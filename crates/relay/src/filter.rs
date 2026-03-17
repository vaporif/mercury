use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FilterPolicy {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PacketFilterConfig {
    pub policy: FilterPolicy,
    pub source_ports: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct PacketFilter {
    policy: FilterPolicy,
    patterns: GlobSet,
}

impl PacketFilter {
    pub fn new(config: &PacketFilterConfig) -> eyre::Result<Self> {
        if config.source_ports.is_empty() {
            eyre::bail!("packet_filter.source_ports must not be empty");
        }

        let mut builder = GlobSetBuilder::new();
        for pattern in &config.source_ports {
            let glob = Glob::new(pattern)
                .map_err(|e| eyre::eyre!("invalid glob pattern '{pattern}': {e}"))?;
            builder.add(glob);
        }
        let patterns = builder
            .build()
            .map_err(|e| eyre::eyre!("failed to compile glob patterns: {e}"))?;

        Ok(Self {
            policy: config.policy,
            patterns,
        })
    }

    /// Returns `true` if a packet with the given source ports should be relayed.
    ///
    /// - **Allow:** all ports must match at least one pattern.
    /// - **Deny:** no port may match any pattern.
    #[must_use]
    pub fn allows(&self, source_ports: &[String]) -> bool {
        match self.policy {
            FilterPolicy::Allow => source_ports.iter().all(|port| self.patterns.is_match(port)),
            FilterPolicy::Deny => !source_ports.iter().any(|port| self.patterns.is_match(port)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(policy: FilterPolicy, ports: &[&str]) -> PacketFilterConfig {
        PacketFilterConfig {
            policy,
            source_ports: ports.iter().map(ToString::to_string).collect(),
        }
    }

    fn ports(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn allow_single_port_matches() {
        let filter = PacketFilter::new(&cfg(FilterPolicy::Allow, &["transfer"])).unwrap();
        assert!(filter.allows(&ports(&["transfer"])));
    }

    #[test]
    fn allow_single_port_rejects_non_match() {
        let filter = PacketFilter::new(&cfg(FilterPolicy::Allow, &["transfer"])).unwrap();
        assert!(!filter.allows(&ports(&["icacontroller-foo"])));
    }

    #[test]
    fn deny_single_port_rejects_match() {
        let filter = PacketFilter::new(&cfg(FilterPolicy::Deny, &["icacontroller-*"])).unwrap();
        assert!(!filter.allows(&ports(&["icacontroller-foo"])));
    }

    #[test]
    fn deny_single_port_allows_non_match() {
        let filter = PacketFilter::new(&cfg(FilterPolicy::Deny, &["icacontroller-*"])).unwrap();
        assert!(filter.allows(&ports(&["transfer"])));
    }

    #[test]
    fn allow_wildcard_matches() {
        let filter = PacketFilter::new(&cfg(FilterPolicy::Allow, &["ica*"])).unwrap();
        assert!(filter.allows(&ports(&["icacontroller-foo"])));
        assert!(filter.allows(&ports(&["icahost"])));
    }

    #[test]
    fn allow_multiple_patterns() {
        let filter =
            PacketFilter::new(&cfg(FilterPolicy::Allow, &["transfer", "oracle-*"])).unwrap();
        assert!(filter.allows(&ports(&["transfer"])));
        assert!(filter.allows(&ports(&["oracle-v1"])));
        assert!(!filter.allows(&ports(&["icacontroller-foo"])));
    }

    #[test]
    fn allow_multi_payload_all_must_match() {
        let filter = PacketFilter::new(&cfg(FilterPolicy::Allow, &["transfer"])).unwrap();
        assert!(filter.allows(&ports(&["transfer", "transfer"])));
        assert!(!filter.allows(&ports(&["transfer", "icacontroller-foo"])));
    }

    #[test]
    fn deny_multi_payload_any_match_rejects() {
        let filter = PacketFilter::new(&cfg(FilterPolicy::Deny, &["icacontroller-*"])).unwrap();
        assert!(!filter.allows(&ports(&["transfer", "icacontroller-foo"])));
        assert!(filter.allows(&ports(&["transfer", "oracle"])));
    }

    #[test]
    fn deny_wildcard_star_rejects_all() {
        let filter = PacketFilter::new(&cfg(FilterPolicy::Deny, &["*"])).unwrap();
        assert!(!filter.allows(&ports(&["transfer"])));
        assert!(!filter.allows(&ports(&["anything"])));
    }

    #[test]
    fn allow_wildcard_star_allows_all() {
        let filter = PacketFilter::new(&cfg(FilterPolicy::Allow, &["*"])).unwrap();
        assert!(filter.allows(&ports(&["transfer"])));
        assert!(filter.allows(&ports(&["anything"])));
    }

    #[test]
    fn empty_source_ports_is_error() {
        let result = PacketFilter::new(&cfg(FilterPolicy::Allow, &[]));
        assert!(result.is_err());
    }

    #[test]
    fn invalid_glob_pattern_is_error() {
        let result = PacketFilter::new(&cfg(FilterPolicy::Allow, &["[invalid"]));
        assert!(result.is_err());
    }

    #[test]
    fn config_deserialization() {
        let allow: PacketFilterConfig = toml::from_str(
            r#"
            policy = "allow"
            source_ports = ["transfer", "oracle-*"]
        "#,
        )
        .unwrap();
        assert_eq!(allow.policy, FilterPolicy::Allow);
        assert_eq!(allow.source_ports, vec!["transfer", "oracle-*"]);

        let deny: PacketFilterConfig = toml::from_str(
            r#"
            policy = "deny"
            source_ports = ["icacontroller-*"]
        "#,
        )
        .unwrap();
        assert_eq!(deny.policy, FilterPolicy::Deny);
    }

    #[test]
    fn config_missing_policy_fails() {
        let toml_str = r#"
            source_ports = ["transfer"]
        "#;
        let result: Result<PacketFilterConfig, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn allow_empty_ports_allows() {
        let filter = PacketFilter::new(&cfg(FilterPolicy::Allow, &["transfer"])).unwrap();
        assert!(filter.allows(&[]));
    }

    #[test]
    fn deny_empty_ports_allows() {
        let filter = PacketFilter::new(&cfg(FilterPolicy::Deny, &["icacontroller-*"])).unwrap();
        assert!(filter.allows(&[]));
    }
}
