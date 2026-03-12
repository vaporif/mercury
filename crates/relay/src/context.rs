use std::borrow::Borrow;
use std::sync::Arc;

use mercury_chain_traits::events::CanQueryBlockEvents;
use mercury_chain_traits::message_builders::CanBuildUpdateClientMessage;
use mercury_chain_traits::packet_builders::{
    CanBuildAckPacketMessage, CanBuildReceivePacketMessage, CanBuildTimeoutPacketMessage,
};
use mercury_chain_traits::packet_queries::{
    CanQueryPacketAcknowledgement, CanQueryPacketCommitment, CanQueryPacketReceipt,
};
use mercury_chain_traits::payload_builders::CanBuildUpdateClientPayload;
use mercury_chain_traits::queries::{
    CanQueryClientState, HasClientLatestHeight, HasTrustingPeriod,
};
use mercury_chain_traits::relay::Relay;
use mercury_chain_traits::types::{
    Chain, HasChainTypes, HasIbcTypes, HasPacketTypes, HasRevisionNumber,
};
use mercury_core::error::Result;
use mercury_core::worker::spawn_worker;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::workers::client_refresh::ClientRefreshWorker;
use crate::workers::event_watcher::EventWatcher;
use crate::workers::packet_worker::PacketWorker;
use crate::workers::tx_worker::{SrcTxWorker, TxWorker};

const CHANNEL_BUFFER: usize = 256;

/// Unidirectional relay context between a source and destination chain.
pub struct RelayContext<Src, Dst>
where
    Src: Chain<Dst>,
    Dst: Chain<Src>,
{
    pub src_chain: Src,
    pub dst_chain: Dst,
    pub src_client_id: <Src as HasIbcTypes<Dst>>::ClientId,
    pub dst_client_id: <Dst as HasIbcTypes<Src>>::ClientId,
}

impl<Src, Dst> Relay for RelayContext<Src, Dst>
where
    Src: Chain<Dst>,
    Dst: Chain<Src>,
{
    type SrcChain = Src;
    type DstChain = Dst;

    fn src_chain(&self) -> &Src {
        &self.src_chain
    }

    fn dst_chain(&self) -> &Dst {
        &self.dst_chain
    }

    fn src_client_id(&self) -> &<Src as HasIbcTypes<Dst>>::ClientId {
        &self.src_client_id
    }

    fn dst_client_id(&self) -> &<Dst as HasIbcTypes<Src>>::ClientId {
        &self.dst_client_id
    }
}

impl<Src, Dst> RelayContext<Src, Dst>
where
    Src: Chain<Dst>
        + CanQueryBlockEvents
        + CanBuildUpdateClientPayload<Dst>
        + CanQueryPacketCommitment<Dst>
        + CanQueryPacketAcknowledgement<Dst>
        + HasRevisionNumber
        // Timeout: src needs to query its client of dst, build update + timeout msgs
        + CanQueryClientState<Dst>
        + HasClientLatestHeight<Dst>
        + CanBuildUpdateClientMessage<Dst>
        + CanBuildTimeoutPacketMessage<Dst>,
    Dst: Chain<Src>
        + CanQueryClientState<Src>
        + HasClientLatestHeight<Src>
        + HasTrustingPeriod<Src>
        + CanBuildUpdateClientMessage<Src>
        + CanBuildReceivePacketMessage<Src>
        + CanBuildAckPacketMessage<Src>
        + HasRevisionNumber
        // Timeout: dst provides receipt proof + update client payload for src
        + CanQueryPacketReceipt<Src>
        + CanBuildUpdateClientPayload<Src>,
    <Dst as CanBuildReceivePacketMessage<Src>>::ReceivePacketPayload: From<(
        <Src as HasIbcTypes<Dst>>::CommitmentProof,
        <Src as HasChainTypes>::Height,
        u64,
    )>,
    <Dst as CanBuildAckPacketMessage<Src>>::AckPacketPayload: From<(
        <Src as HasIbcTypes<Dst>>::CommitmentProof,
        <Src as HasChainTypes>::Height,
        u64,
    )>,
    <Src as CanBuildTimeoutPacketMessage<Dst>>::TimeoutPacketPayload: From<(
        <Dst as HasIbcTypes<Src>>::CommitmentProof,
        <Dst as HasChainTypes>::Height,
        u64,
    )>,
    <Src as HasPacketTypes<Dst>>::Packet: Borrow<<Dst as HasPacketTypes<Src>>::Packet>,
    <Src as HasPacketTypes<Dst>>::Acknowledgement:
        Borrow<<Dst as HasPacketTypes<Src>>::Acknowledgement>,
    <Dst as HasPacketTypes<Src>>::Acknowledgement:
        Borrow<<Src as HasPacketTypes<Dst>>::Acknowledgement>,
{
    pub async fn run(self: Arc<Self>) -> Result<()> {
        let token = CancellationToken::new();

        let (event_tx, event_rx) = mpsc::channel(CHANNEL_BUFFER);
        let (tx_req_tx, tx_req_rx) = mpsc::channel(CHANNEL_BUFFER);
        let (src_tx_req_tx, src_tx_req_rx) = mpsc::channel(CHANNEL_BUFFER);

        let event_watcher = EventWatcher {
            relay: Arc::clone(&self),
            sender: event_tx,
            token: token.clone(),
        };

        let client_refresh = ClientRefreshWorker {
            relay: Arc::clone(&self),
            sender: tx_req_tx.clone(),
            token: token.clone(),
        };

        let packet_worker = PacketWorker {
            relay: Arc::clone(&self),
            receiver: event_rx,
            sender: tx_req_tx,
            src_sender: src_tx_req_tx,
            token: token.clone(),
        };

        let tx_worker = TxWorker {
            relay: Arc::clone(&self),
            receiver: tx_req_rx,
            token: token.clone(),
        };

        let src_tx_worker = SrcTxWorker {
            relay: Arc::clone(&self),
            receiver: src_tx_req_rx,
            token: token.clone(),
        };

        let event_watcher_handle = spawn_worker(event_watcher);
        let packet_worker_handle = spawn_worker(packet_worker);
        let tx_worker_handle = spawn_worker(tx_worker);
        let src_tx_worker_handle = spawn_worker(src_tx_worker);
        let client_refresh_handle = spawn_worker(client_refresh);

        let result = tokio::select! {
            res = event_watcher_handle => res,
            res = packet_worker_handle => res,
            res = tx_worker_handle => res,
            res = src_tx_worker_handle => res,
            res = client_refresh_handle => res,
        };

        token.cancel();

        match result {
            Ok(worker_result) => worker_result,
            Err(join_err) => Err(mercury_core::error::Error::report(join_err)),
        }
    }
}
