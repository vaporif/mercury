use std::fmt::Display;

pub fn require_http_url(name: &str, url: &str) -> eyre::Result<()> {
    eyre::ensure!(
        url.starts_with("http://") || url.starts_with("https://"),
        "{name} must start with http:// or https://, got '{url}'"
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
