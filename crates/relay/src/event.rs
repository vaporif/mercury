use async_trait::async_trait;

use mercury_chain_traits::relay::event::CanRelayEvents;
use mercury_chain_traits::types::{Chain, HasChainTypes};
use mercury_core::error::Result;

use crate::context::RelayContext;

#[async_trait]
impl<Src, Dst> CanRelayEvents for RelayContext<Src, Dst>
where
    Src: Chain<Dst>,
    Dst: Chain<Src>,
{
    async fn relay_events(&self, _events: Vec<<Src as HasChainTypes>::Event>) -> Result<()> {
        // TODO: extract send_packet events, relay each packet
        todo!("relay events")
    }
}
