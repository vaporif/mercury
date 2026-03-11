use async_trait::async_trait;

use mercury_chain_traits::message_builders::CanBuildUpdateClientMessage;
use mercury_chain_traits::messaging::CanSendMessages;
use mercury_chain_traits::payload_builders::CanBuildUpdateClientPayload;
use mercury_chain_traits::queries::{CanQueryChainStatus, CanQueryClientState};
use mercury_chain_traits::relay::client::CanUpdateClient;
use mercury_chain_traits::types::{
    HasChainStatusType, HasIbcTypes, HasMessageTypes, HasPacketTypes,
};
use mercury_core::error::Result;

use crate::context::RelayContext;

#[async_trait]
impl<Src, Dst> CanUpdateClient for RelayContext<Src, Dst>
where
    Src: HasMessageTypes
        + HasIbcTypes<Dst>
        + HasPacketTypes<Dst>
        + HasChainStatusType
        + CanSendMessages
        + CanQueryChainStatus
        + CanBuildUpdateClientPayload<Dst>
        + CanBuildUpdateClientMessage<Dst>,
    Dst: HasMessageTypes
        + HasIbcTypes<Src>
        + HasPacketTypes<Src>
        + HasChainStatusType
        + CanSendMessages
        + CanQueryChainStatus
        + CanQueryClientState<Src>
        + CanBuildUpdateClientPayload<Src>
        + CanBuildUpdateClientMessage<Src>,
{
    async fn update_src_client(&self) -> Result<()> {
        // TODO: query dst chain status, build update payload, build message, send to src
        todo!("update src client")
    }

    async fn update_dst_client(&self) -> Result<()> {
        // TODO: query src chain status, build update payload, build message, send to dst
        todo!("update dst client")
    }
}
