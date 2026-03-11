use async_trait::async_trait;

use mercury_chain_traits::messaging::CanSendMessages;
use mercury_chain_traits::types::{HasChainTypes, HasIbcTypes, HasMessageTypes, HasPacketTypes};
use mercury_chain_traits::relay::event::CanRelayEvents;
use mercury_core::error::Result;

use crate::context::RelayContext;

#[async_trait]
impl<Src, Dst> CanRelayEvents for RelayContext<Src, Dst>
where
    Src: HasMessageTypes + HasIbcTypes<Dst> + HasPacketTypes<Dst> + CanSendMessages,
    Dst: HasMessageTypes + HasIbcTypes<Src> + HasPacketTypes<Src> + CanSendMessages,
{
    async fn relay_events(
        &self,
        _events: Vec<<Src as HasChainTypes>::Event>,
    ) -> Result<()> {
        // TODO: extract send_packet events, relay each packet
        todo!("relay events")
    }
}
