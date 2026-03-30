use alloy::primitives::{Address, B256, U256};
use alloy::rpc::types::Log;
use eyre::{OptionExt, Result};

use mercury_chain_traits::types::{PacketSequence, Port, TimeoutTimestamp};

use derive_more::{Display, From, Into};

#[derive(Clone, Debug, Display, From, Into, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EvmHeight(pub u64);

#[derive(Clone, Debug, Display, From, Into, PartialEq, Eq, Hash)]
pub struct EvmChainId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EvmTimestamp(pub u64);

#[derive(Clone, Copy, Debug, Display, From, Into, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockNumber(pub u64);

#[derive(Clone, Copy, Debug, Display, From, Into, PartialEq, Eq, Hash)]
pub struct LogIndex(pub u64);

#[derive(Clone, Copy, Debug, Display, From, Into, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProofHeight(pub u64);

#[derive(Clone, Copy, Debug, Display, From, Into, PartialEq, Eq, Hash)]
pub struct GasUsed(pub u64);

#[derive(Clone, Debug)]
pub struct EvmEvent {
    pub address: Address,
    pub topics: Vec<B256>,
    pub data: Vec<u8>,
    pub block_number: BlockNumber,
    pub tx_hash: B256,
    pub log_index: LogIndex,
}

impl EvmEvent {
    pub fn from_alloy_log(log: &Log) -> Result<Self> {
        Ok(Self {
            address: log.address(),
            topics: log.topics().to_vec(),
            data: log.data().data.to_vec(),
            block_number: BlockNumber(log.block_number.ok_or_eyre("missing block_number")?),
            tx_hash: log.transaction_hash.ok_or_eyre("missing tx_hash")?,
            log_index: LogIndex(log.log_index.ok_or_eyre("missing log_index")?),
        })
    }
}

#[derive(Clone, Debug)]
pub struct EvmMessage {
    pub to: Address,
    pub calldata: Vec<u8>,
    pub value: U256,
}

#[derive(Clone, Debug)]
pub struct EvmTxResponse {
    pub tx_hash: B256,
    pub block_number: BlockNumber,
    pub gas_used: GasUsed,
    pub logs: Vec<EvmEvent>,
}

#[derive(Clone, Debug)]
pub struct EvmChainStatus {
    pub height: EvmHeight,
    pub timestamp: EvmTimestamp,
}

#[derive(Clone, Debug, Display, From, Into, PartialEq, Eq, Hash)]
pub struct EvmClientId(pub String);

#[derive(Clone, Debug)]
pub struct EvmPacket {
    pub source_client: String,
    pub dest_client: String,
    pub sequence: PacketSequence,
    pub timeout_timestamp: TimeoutTimestamp,
    pub payloads: Vec<EvmPayload>,
}

#[derive(Clone, Debug)]
pub struct EvmPayload {
    pub source_port: Port,
    pub dest_port: Port,
    pub version: String,
    pub encoding: String,
    pub value: Vec<u8>,
}

/// EIP-1186 storage proof for a single slot, obtained via `eth_getProof`.
#[derive(Clone, Debug)]
pub struct EvmCommitmentProof {
    pub proof_height: ProofHeight,
    pub storage_root: B256,
    pub account_proof: Vec<Vec<u8>>,
    pub storage_key: B256,
    pub storage_value: U256,
    pub storage_proof: Vec<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct EvmClientState(pub Vec<u8>);

#[derive(Clone, Debug)]
pub struct EvmConsensusState(pub Vec<u8>);

#[derive(Clone, Debug)]
pub struct EvmPacketCommitment(pub Vec<u8>);

#[derive(Clone, Debug)]
pub struct EvmPacketReceipt;

#[derive(Clone, Debug)]
pub struct EvmAcknowledgement(pub Vec<u8>);

#[derive(Clone, Debug)]
pub struct EvmSendPacketEvent {
    pub packet: EvmPacket,
    pub block_number: BlockNumber,
}

#[derive(Clone, Debug)]
pub struct EvmWriteAckEvent {
    pub packet: EvmPacket,
    pub ack: EvmAcknowledgement,
    pub block_number: BlockNumber,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evm_height_display() {
        assert_eq!(EvmHeight(42).to_string(), "42");
    }

    #[test]
    fn evm_height_ordering() {
        assert!(EvmHeight(10) < EvmHeight(20));
        assert!(EvmHeight(5) == EvmHeight(5));
    }

    #[test]
    fn evm_chain_id_display() {
        assert_eq!(EvmChainId(1).to_string(), "1");
        assert_eq!(EvmChainId(31337).to_string(), "31337");
    }

    #[test]
    fn evm_client_id_display() {
        let id = EvmClientId("07-tendermint-0".to_string());
        assert_eq!(id.to_string(), "07-tendermint-0");
    }

    #[test]
    fn evm_timestamp_ordering() {
        assert!(EvmTimestamp(100) < EvmTimestamp(200));
    }
}
