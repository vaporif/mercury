use async_trait::async_trait;
use mercury_core::error::Result;

use super::context::Relay;
use crate::types::HasChainTypes;

#[async_trait]
pub trait CanRelayEvents: Relay {
    async fn relay_events(
        &self,
        events: Vec<<Self::SrcChain as HasChainTypes>::Event>,
    ) -> Result<()>;
}
