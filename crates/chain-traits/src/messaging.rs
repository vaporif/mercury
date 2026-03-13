use async_trait::async_trait;
use mercury_core::error::Result;

use crate::types::HasChainTypes;

/// Sends a batch of messages to the chain.
#[async_trait]
pub trait CanSendMessages: HasChainTypes {
    async fn send_messages(
        &self,
        messages: Vec<Self::Message>,
    ) -> Result<Vec<Self::MessageResponse>>;
}
