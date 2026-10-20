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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize)]
    struct Wrapper {
        gas_multiplier: GasMultiplier,
    }

    fn parse_multiplier(raw: &str) -> Result<f64, toml::de::Error> {
        toml::from_str::<Wrapper>(&format!("gas_multiplier = {raw}"))
            .map(|w| w.gas_multiplier.value())
    }

    #[test]
    fn gas_multiplier_accepts_one_and_above() {
        assert!((parse_multiplier("1.0").unwrap() - 1.0).abs() < f64::EPSILON);
        assert!((parse_multiplier("1.3").unwrap() - 1.3).abs() < f64::EPSILON);
    }

    #[test]
    fn gas_multiplier_below_one_is_rejected() {
        let err = parse_multiplier("0.99").unwrap_err().to_string();
        assert!(
            err.contains("gas multiplier must be >= 1.0, got 0.99"),
            "{err}"
        );
    }

    #[test]
    fn gas_multiplier_rejects_non_numbers() {
        assert!(parse_multiplier("\"fast\"").is_err());
    }

    #[test]
    fn gas_multiplier_new_unchecked_skips_validation() {
        assert!((GasMultiplier::new_unchecked(0.5).value() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn require_http_url_accepts_http_and_https_only() {
        require_http_url("rpc_addr", "http://localhost:26657").unwrap();
        require_http_url("rpc_addr", "https://rpc.cosmos.network:443").unwrap();
        for bad in ["ws://localhost", "localhost:26657", "", "HTTP://x"] {
            let err = require_http_url("rpc_addr", bad).unwrap_err().to_string();
            assert_eq!(
                err,
                format!("rpc_addr must start with http:// or https://, got '{bad}'")
            );
        }
    }

    #[test]
    fn require_ws_url_accepts_ws_and_wss_only() {
        require_ws_url("ws_addr", "ws://localhost:26657/websocket").unwrap();
        require_ws_url("ws_addr", "wss://rpc.example.com").unwrap();
        for bad in ["http://localhost", "localhost", ""] {
            let err = require_ws_url("ws_addr", bad).unwrap_err().to_string();
            assert_eq!(
                err,
                format!("ws_addr must start with ws:// or wss://, got '{bad}'")
            );
        }
    }

    #[test]
    fn require_positive_rejects_zero_and_negative() {
        require_positive("max_gas", &1u64).unwrap();
        require_positive("ratio", &0.1f64).unwrap();
        assert_eq!(
            require_positive("max_gas", &0u64).unwrap_err().to_string(),
            "max_gas must be > 0, got 0"
        );
        assert_eq!(
            require_positive("delta", &-1i64).unwrap_err().to_string(),
            "delta must be > 0, got -1"
        );
    }
}
