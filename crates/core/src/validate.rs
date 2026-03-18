use std::fmt::Display;

use serde::Deserialize;

#[derive(Clone, Copy, Debug)]
pub struct GasMultiplier(f64);

impl GasMultiplier {
    #[must_use]
    pub const fn new_unchecked(v: f64) -> Self {
        Self(v)
    }

    #[must_use]
    pub const fn value(self) -> f64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for GasMultiplier {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = f64::deserialize(deserializer)?;
        if v < 1.0 {
            return Err(serde::de::Error::custom(format!(
                "gas multiplier must be >= 1.0, got {v}"
            )));
        }
        Ok(Self(v))
    }
}

pub fn require_http_url(name: &str, url: &str) -> eyre::Result<()> {
    eyre::ensure!(
        url.starts_with("http://") || url.starts_with("https://"),
        "{name} must start with http:// or https://, got '{url}'"
    );
    Ok(())
}

pub fn require_ws_url(name: &str, url: &str) -> eyre::Result<()> {
    eyre::ensure!(
        url.starts_with("ws://") || url.starts_with("wss://"),
        "{name} must start with ws:// or wss://, got '{url}'"
    );
    Ok(())
}

pub fn require_positive<T: Default + PartialOrd + Display>(
    name: &str,
    value: &T,
) -> eyre::Result<()> {
    eyre::ensure!(*value > T::default(), "{name} must be > 0, got {value}");
    Ok(())
}
