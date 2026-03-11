use async_trait::async_trait;
use mercury_core::error::Result;

use crate::types::HasMessageTypes;

#[async_trait]
pub trait CanSendMessages: HasMessageTypes {
    async fn send_messages(
        &self,
        messages: Vec<Self::Message>,
    ) -> Result<Vec<Self::MessageResponse>>;
}
