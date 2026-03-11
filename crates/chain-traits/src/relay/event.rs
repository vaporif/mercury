use async_trait::async_trait;
use mercury_core::error::Result;

use crate::types::HasChainTypes;
use super::context::Relay;

#[async_trait]
pub trait CanRelayEvents: Relay {
    async fn relay_events(
        &self,
        events: Vec<<Self::SrcChain as HasChainTypes>::Event>,
    ) -> Result<()>;
}
