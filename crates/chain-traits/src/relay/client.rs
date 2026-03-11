use async_trait::async_trait;
use mercury_core::error::Result;

use super::context::Relay;

#[async_trait]
pub trait CanUpdateClient: Relay {
    async fn update_src_client(&self) -> Result<()>;
    async fn update_dst_client(&self) -> Result<()>;
}
