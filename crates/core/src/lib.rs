use std::sync::Arc;

pub mod encoding;
pub mod error;
pub mod plugin;
pub mod registry;
/// RPC rate-limiting and timeout guard.
pub mod rpc_guard;
pub mod validate;
pub mod worker;

pub trait ThreadSafe: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> ThreadSafe for T {}

/// Human-readable chain identifier
#[derive(Clone, Debug)]
pub struct ChainLabel {
    name: Arc<str>,
    id: Option<Arc<str>>,
}

impl ChainLabel {
    #[must_use]
    pub fn new(name: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            id: None,
        }
    }

    #[must_use]
    pub fn with_id(name: impl Into<Arc<str>>, id: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            id: Some(id.into()),
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    #[must_use]
    pub fn metric_labels(&self) -> Vec<(&'static str, String)> {
        let mut labels = vec![("chain_name", self.name.to_string())];
        if let Some(id) = &self.id {
            labels.push(("chain_id", id.to_string()));
        }
        labels
    }

    /// Returns metric labels prefixed with `counterparty_`.
    #[must_use]
    pub fn counterparty_metric_labels(&self) -> Vec<(&'static str, String)> {
        let mut labels = vec![("counterparty_chain_name", self.name.to_string())];
        if let Some(id) = &self.id {
            labels.push(("counterparty_chain_id", id.to_string()));
        }
        labels
    }
}

impl std::fmt::Display for ChainLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.id {
            Some(id) => write!(f, "{}/{id}", self.name),
            None => f.write_str(&self.name),
        }
    }
}

/// IBC merkle prefix representing the key path in nested merkle trees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MerklePrefix(pub Vec<Vec<u8>>);

impl MerklePrefix {
    #[must_use]
    pub fn ibc_default() -> Self {
        Self(vec![b"ibc".to_vec(), b"".to_vec()])
    }
}

impl serde::Serialize for MerklePrefix {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = s.serialize_seq(Some(self.0.len()))?;
        for seg in &self.0 {
            let as_str = std::str::from_utf8(seg)
                .map_err(|e| serde::ser::Error::custom(format!("non-utf8 segment: {e}")))?;
            seq.serialize_element(as_str)?;
        }
        seq.end()
    }
}

impl<'de> serde::Deserialize<'de> for MerklePrefix {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let segments: Vec<String> = Vec::deserialize(d)?;
        Ok(Self(segments.into_iter().map(String::into_bytes).collect()))
    }
}

/// A single membership proof entry: IBC merkle path segments, stored value, and proof bytes.
#[derive(Clone, Debug)]
pub struct MembershipProofEntry {
    pub path: Vec<Vec<u8>>,
    pub value: Vec<u8>,
    pub proof: Vec<u8>,
}

/// Collection of membership proof entries for batched proving.
#[derive(Clone, Debug, Default)]
pub struct MembershipProofs(pub Vec<MembershipProofEntry>);

impl MembershipProofs {
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn push(&mut self, entry: MembershipProofEntry) {
        self.0.push(entry);
    }
}

#[cfg(test)]
mod merkle_prefix_serde_tests {
    use super::MerklePrefix;

    #[derive(serde::Serialize, serde::Deserialize)]
    struct TomlWrapper {
        prefix: MerklePrefix,
    }

    #[test]
    fn ibc_default_round_trips_through_toml() {
        let p = MerklePrefix::ibc_default();
        let toml_str = toml::to_string(&TomlWrapper { prefix: p.clone() }).unwrap();
        let parsed: TomlWrapper = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.prefix, p);
    }

    #[test]
    fn ibc_default_deserialises_from_string_list() {
        let toml_str = r#"prefix = ["ibc", ""]"#;
        let parsed: TomlWrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(parsed.prefix, MerklePrefix::ibc_default());
    }
}
