use async_trait::async_trait;

use mercury_chain_traits::builders::{ClientMessageBuilder, ClientPayloadBuilder};
use mercury_chain_traits::inner::HasCore;
use mercury_chain_traits::queries::ClientQuery;
use mercury_chain_traits::relay::{ClientUpdater, Relay, RelayChain};
use mercury_core::error::Result;

use crate::context::RelayContext;

#[async_trait]
impl<Src, Dst> ClientUpdater for RelayContext<Src, Dst>
where
    Src: RelayChain + ClientPayloadBuilder<<Dst as HasCore>::Core>,
    Dst: RelayChain
        + ClientMessageBuilder<
            <Src as HasCore>::Core,
            UpdateClientPayload = <Src as ClientPayloadBuilder<<Dst as HasCore>::Core>>::UpdateClientPayload,
        > + ClientQuery<<Src as HasCore>::Core>,
    Self: Relay<SrcChain = Src, DstChain = Dst>,
{
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
            .build_update_client_payload(&trusted_height, &target_height, &client_state)
            .await?;

        let output = self
            .dst_chain()
            .build_update_client_message(self.dst_client_id(), payload)
            .await?;

        self.dst_chain().send_messages(output.messages).await?;
        Ok(())
    }
}
