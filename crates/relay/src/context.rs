use std::sync::Arc;
use std::time::Duration;

use mercury_chain_traits::builders::{
    ClientMessageBuilder, ClientPayloadBuilder, MisbehaviourDetector, MisbehaviourMessageBuilder,
    PacketMessageBuilder,
};
use mercury_chain_traits::events::PacketEvents;
use mercury_chain_traits::queries::{
    ChainStatusQuery, ClientQuery, MisbehaviourQuery, PacketStateQuery,
};
use mercury_chain_traits::relay::Relay;
use mercury_chain_traits::types::{ChainTypes, IbcTypes, MessageSender};
use mercury_core::error::Result;
use mercury_core::worker::spawn_worker;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::filter::PacketFilter;
use crate::workers::clearing_worker::ClearingWorker;
use crate::workers::client_refresh::ClientRefreshWorker;
use crate::workers::event_watcher::EventWatcher;
use crate::workers::misbehaviour_worker::MisbehaviourWorker;
use crate::workers::packet_worker::PacketWorker;
use crate::workers::tx_worker::{SrcTxWorker, TxWorker};

const CHANNEL_BUFFER: usize = 256;

/// Configuration for optional relay workers.
#[derive(Clone, Default)]
pub struct RelayWorkerConfig {
    pub lookback: Option<Duration>,
    pub clearing_interval: Option<Duration>,
    pub misbehaviour_scan_interval: Option<Duration>,
    pub packet_filter: Option<PacketFilter>,
}

/// Unidirectional relay context between a source and destination chain.
pub struct RelayContext<Src: ChainTypes, Dst: ChainTypes> {
    pub src_chain: Src,
    pub dst_chain: Dst,
    pub src_client_id: Src::ClientId,
    pub dst_client_id: Dst::ClientId,
}

impl<Src, Dst> Relay for RelayContext<Src, Dst>
where
    Src: ChainTypes
        + ChainStatusQuery
        + MessageSender
        + ClientPayloadBuilder<Dst>
        + PacketEvents<Dst>
        + IbcTypes<Dst, Packet = <Src as PacketEvents<Dst>>::Packet>,
    Dst: ChainTypes
        + ChainStatusQuery
        + MessageSender
        + IbcTypes<Src>
        + ClientMessageBuilder<
            Src,
            CreateClientPayload = <Src as ClientPayloadBuilder<Dst>>::CreateClientPayload,
            UpdateClientPayload = <Src as ClientPayloadBuilder<Dst>>::UpdateClientPayload,
        > + ClientQuery<Src, ClientState = <Dst as IbcTypes<Src>>::ClientState>,
{
    type SrcChain = Src;
    type DstChain = Dst;

    fn src_chain(&self) -> &Src {
        &self.src_chain
    }

    fn dst_chain(&self) -> &Dst {
        &self.dst_chain
    }

    fn src_client_id(&self) -> &Src::ClientId {
        &self.src_client_id
    }

    fn dst_client_id(&self) -> &Dst::ClientId {
        &self.dst_client_id
    }
}

impl<Src, Dst> RelayContext<Src, Dst>
where
    Src: ChainTypes
        + ChainStatusQuery
        + MessageSender
        + ClientPayloadBuilder<Dst>
        + PacketEvents<Dst>
        + IbcTypes<Dst, Packet = <Src as PacketEvents<Dst>>::Packet>
        + PacketStateQuery<Dst>
        + PacketMessageBuilder<Dst>
        + ClientQuery<Dst, ClientState = <Src as IbcTypes<Dst>>::ClientState>
        + ClientMessageBuilder<
            Dst,
            CreateClientPayload = <Dst as ClientPayloadBuilder<Src>>::CreateClientPayload,
            UpdateClientPayload = <Dst as ClientPayloadBuilder<Src>>::UpdateClientPayload,
        > + MisbehaviourDetector<Dst, CounterpartyClientState = <Dst as ClientQuery<Src>>::ClientState>,
    Dst: ChainTypes
        + ChainStatusQuery
        + MessageSender
        + IbcTypes<Src>
        + ClientMessageBuilder<
            Src,
            CreateClientPayload = <Src as ClientPayloadBuilder<Dst>>::CreateClientPayload,
            UpdateClientPayload = <Src as ClientPayloadBuilder<Dst>>::UpdateClientPayload,
        > + ClientQuery<Src, ClientState = <Dst as IbcTypes<Src>>::ClientState>
        + PacketEvents<Src>
        + PacketStateQuery<Src>
        + PacketMessageBuilder<
            Src,
            CounterpartyPacket = <Src as PacketEvents<Dst>>::Packet,
            CounterpartyAcknowledgement = <Src as PacketEvents<Dst>>::Acknowledgement,
        > + ClientPayloadBuilder<Src>
        + MisbehaviourQuery<
            Src,
            CounterpartyUpdateHeader = <Src as MisbehaviourDetector<Dst>>::UpdateHeader,
        > + MisbehaviourMessageBuilder<
            Src,
            MisbehaviourEvidence = <Src as MisbehaviourDetector<Dst>>::MisbehaviourEvidence,
        >,
    // Payload From bounds
    <Dst as PacketMessageBuilder<Src>>::ReceivePacketPayload: From<(
        <Src as PacketStateQuery<Dst>>::CommitmentProof,
        <Src as ChainTypes>::Height,
        u64,
    )>,
    <Dst as PacketMessageBuilder<Src>>::AckPacketPayload: From<(
        <Src as PacketStateQuery<Dst>>::CommitmentProof,
        <Src as ChainTypes>::Height,
        u64,
    )>,
    <Src as PacketMessageBuilder<Dst>>::TimeoutPacketPayload: From<(
        <Dst as PacketStateQuery<Src>>::CommitmentProof,
        <Dst as ChainTypes>::Height,
        u64,
    )>,
{
    pub async fn run_with_token(
        self: Arc<Self>,
        token: CancellationToken,
        config: RelayWorkerConfig,
    ) -> Result<()> {
        let start_height = if let Some(lookback) = config.lookback {
            let latest = self.src_chain.query_latest_height().await?;
            let block_time = self.src_chain.block_time();
            let blocks_back = (lookback.as_secs() / block_time.as_secs().max(1)).max(1);
            Src::sub_height(&latest, blocks_back)
        } else {
            None
        };

        let (event_tx, event_rx) = mpsc::channel(CHANNEL_BUFFER);
        let (tx_req_tx, tx_req_rx) = mpsc::channel(CHANNEL_BUFFER);
        let (src_tx_req_tx, src_tx_req_rx) = mpsc::channel(CHANNEL_BUFFER);

        let event_watcher = EventWatcher {
            relay: Arc::clone(&self),
            sender: event_tx.clone(),
            token: token.clone(),
            start_height,
            packet_filter: config.packet_filter.clone(),
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

        let clearing_handle = config.clearing_interval.map_or_else(
            || tokio::spawn(futures::future::pending()),
            |interval| {
                let clearing_worker = ClearingWorker {
                    relay: Arc::clone(&self),
                    sender: event_tx,
                    token: token.clone(),
                    interval,
                    packet_filter: config.packet_filter.clone(),
                };
                spawn_worker(clearing_worker)
            },
        );

        let misbehaviour_handle = config.misbehaviour_scan_interval.map_or_else(
            || tokio::spawn(futures::future::pending()),
            |interval| {
                let misbehaviour_worker = MisbehaviourWorker {
                    relay: Arc::clone(&self),
                    token: token.clone(),
                    scan_interval: interval,
                };
                spawn_worker(misbehaviour_worker)
            },
        );

        let result = tokio::select! {
            res = event_watcher_handle => res,
            res = packet_worker_handle => res,
            res = tx_worker_handle => res,
            res = src_tx_worker_handle => res,
            res = client_refresh_handle => res,
            res = clearing_handle => res,
            res = misbehaviour_handle => res,
        };

        token.cancel();

        match result {
            Ok(worker_result) => worker_result,
            Err(join_err) => Err(eyre::eyre!(join_err)),
        }
    }

    pub async fn run(self: Arc<Self>, config: RelayWorkerConfig) -> Result<()> {
        self.run_with_token(CancellationToken::new(), config).await
    }
}
