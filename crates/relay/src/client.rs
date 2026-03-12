use async_trait::async_trait;

use mercury_chain_traits::message_builders::CanBuildUpdateClientMessage;
use mercury_chain_traits::payload_builders::CanBuildUpdateClientPayload;
use mercury_chain_traits::queries::{CanQueryClientState, HasClientLatestHeight};
use mercury_chain_traits::relay::client::CanUpdateClient;
use mercury_chain_traits::relay::context::Relay;
use mercury_chain_traits::types::Chain;
use mercury_core::error::Result;

use crate::context::RelayContext;

#[async_trait]
impl<Src, Dst> CanUpdateClient for RelayContext<Src, Dst>
where
    Src: Chain<Dst>
        + CanQueryClientState<Dst>
        + CanBuildUpdateClientPayload<Dst>
        + CanBuildUpdateClientMessage<Dst>
        + HasClientLatestHeight<Dst>,
    Dst: Chain<Src>
        + CanQueryClientState<Src>
        + CanBuildUpdateClientPayload<Src>
        + CanBuildUpdateClientMessage<Src>
        + HasClientLatestHeight<Src>,
{
    async fn update_src_client(&self) -> Result<()> {
        let dst_status = self.dst_chain().query_chain_status().await?;
        let target_height = Dst::chain_status_height(&dst_status).clone();

        let src_status = self.src_chain().query_chain_status().await?;
        let src_height = Src::chain_status_height(&src_status).clone();

        let client_state = self
            .src_chain()
            .query_client_state(self.src_client_id(), &src_height)
            .await?;

        let trusted_height = Src::client_latest_height(&client_state);

        if target_height <= trusted_height {
            return Ok(());
        }

        let payload = self
            .dst_chain()
            .build_update_client_payload(&trusted_height, &target_height)
            .await?;

        let messages = self
            .src_chain()
            .build_update_client_message(self.src_client_id(), payload)
            .await?;

        self.src_chain().send_messages(messages).await?;
        Ok(())
    }

    async fn update_dst_client(&self) -> Result<()> {
        let src_status = self.src_chain().query_chain_status().await?;
        let target_height = Src::chain_status_height(&src_status).clone();

        let dst_status = self.dst_chain().query_chain_status().await?;
        let dst_height = Dst::chain_status_height(&dst_status).clone();

        let client_state = self
            .dst_chain()
            .query_client_state(self.dst_client_id(), &dst_height)
            .await?;

        let trusted_height = Dst::client_latest_height(&client_state);

        if target_height <= trusted_height {
            return Ok(());
        }

        let payload = self
            .src_chain()
            .build_update_client_payload(&trusted_height, &target_height)
            .await?;

        let messages = self
            .dst_chain()
            .build_update_client_message(self.dst_client_id(), payload)
            .await?;

        self.dst_chain().send_messages(messages).await?;
        Ok(())
    }
}
