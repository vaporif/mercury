use std::time::Duration;

use alloy::primitives::{Address, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use alloy::sol_types::{SolCall, SolEvent};
use async_trait::async_trait;
use eyre::Context;
use tracing::instrument;

use mercury_chain_traits::queries::{ChainStatusQuery, ClientQuery, PacketStateQuery};
use mercury_chain_traits::types::{ChainTypes, PacketSequence};
use mercury_core::error::{ProofError, QueryError, Result};

use crate::chain::EthereumChain;
use crate::contracts::sp1_ics07;
use crate::contracts::{ICS26Router, SP1ICS07Tendermint};
use crate::ics24;
use crate::types::{
    EvmAcknowledgement, EvmChainStatus, EvmClientId, EvmClientState, EvmCommitmentProof,
    EvmConsensusState, EvmHeight, EvmPacketCommitment, EvmPacketReceipt, EvmTimestamp, ProofHeight,
};

#[async_trait]
impl ChainStatusQuery for EthereumChain {
    async fn query_chain_status(&self) -> Result<EvmChainStatus> {
        let block_number = self
            .rpc_guard
            .guarded(|| async {
                self.provider
                    .get_block_number()
                    .await
                    .wrap_err("querying latest block number")
            })
            .await?;

        let block_info = self
            .rpc_guard
            .guarded(|| async {
                self.provider
                    .get_block_by_number(block_number.into())
                    .await
                    .wrap_err("querying block by number")
            })
            .await?
            .ok_or_else(|| QueryError::StaleState {
                what: format!("block {block_number}"),
            })?;

        Ok(EvmChainStatus {
            height: EvmHeight(block_number),
            timestamp: EvmTimestamp(block_info.header.timestamp),
        })
    }
}

pub async fn resolve_light_client(
    chain: &EthereumChain,
    client_id: &EvmClientId,
) -> Result<Address> {
    let router = ICS26Router::new(chain.router_address, &*chain.provider);
    chain
        .rpc_guard
        .guarded(|| async {
            router
                .getClient(client_id.0.clone())
                .call()
                .await
                .wrap_err_with(|| format!("getClient({client_id}) failed"))
        })
        .await
}

pub type ClientStateReturn = sp1_ics07::SP1ICS07Tendermint::clientStateReturn;

/// Encode a decoded client state back to ABI bytes (matching the format
/// that `clientStateCall::abi_decode_returns` expects).
#[must_use]
pub fn encode_client_state(cs: &ClientStateReturn) -> Vec<u8> {
    sp1_ics07::SP1ICS07Tendermint::clientStateCall::abi_encode_returns(cs)
}

#[must_use]
pub fn decode_client_state(bytes: &[u8]) -> Option<ClientStateReturn> {
    tracing::debug!(
        bytes_len = bytes.len(),
        "decoding client state from ABI bytes"
    );
    let result = sp1_ics07::SP1ICS07Tendermint::clientStateCall::abi_decode_returns(bytes);
    match &result {
        Ok(cs) => {
            tracing::debug!(
                chain_id = %cs.chainId,
                revision_number = cs.latestHeight.revisionNumber,
                revision_height = cs.latestHeight.revisionHeight,
                trusting_period = cs.trustingPeriod,
                "decoded SP1 client state successfully"
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, bytes_len = bytes.len(), "failed to decode SP1 client state");
        }
    }
    result.ok()
}

#[async_trait]
impl ClientQuery<Self> for EthereumChain {
    #[instrument(skip_all, name = "query_client_state", fields(chain = %self.chain_label(), client_id = %client_id))]
    async fn query_client_state(
        &self,
        client_id: &EvmClientId,
        _height: &EvmHeight,
    ) -> Result<EvmClientState> {
        let lc_address = resolve_light_client(self, client_id).await?;
        let lc = SP1ICS07Tendermint::new(lc_address, &*self.provider);
        // Use clientState() directly (struct accessor) instead of getClientState()
        // (bytes wrapper) to avoid ABI decode mismatches. Re-encode using
        // abi_encode_returns so decode_client_state can round-trip.
        let cs = self
            .rpc_guard
            .guarded(|| async {
                lc.clientState()
                    .call()
                    .await
                    .wrap_err("SP1ICS07Tendermint.clientState() failed")
            })
            .await?;
        tracing::debug!(
            chain_id = %cs.chainId,
            revision_height = cs.latestHeight.revisionHeight,
            "queried SP1 client state"
        );
        Ok(EvmClientState(encode_client_state(&cs)))
    }

    #[instrument(skip_all, name = "query_consensus_state", fields(chain = %self.chain_label(), client_id = %client_id, consensus_height = %consensus_height))]
    async fn query_consensus_state(
        &self,
        client_id: &EvmClientId,
        consensus_height: &EvmHeight,
        _query_height: &EvmHeight,
    ) -> Result<EvmConsensusState> {
        let lc_address = resolve_light_client(self, client_id).await?;
        let lc = SP1ICS07Tendermint::new(lc_address, &*self.provider);
        let result = self
            .rpc_guard
            .guarded(|| async {
                lc.getConsensusStateHash(consensus_height.0)
                    .call()
                    .await
                    .wrap_err_with(|| {
                        format!(
                            "getConsensusStateHash({consensus_height}) failed for client {client_id}"
                        )
                    })
            })
            .await?;
        Ok(EvmConsensusState(result.to_vec()))
    }

    fn trusting_period(client_state: &EvmClientState) -> Option<Duration> {
        let cs = decode_client_state(&client_state.0)?;
        Some(Duration::from_secs(u64::from(cs.trustingPeriod)))
    }

    fn client_latest_height(client_state: &EvmClientState) -> EvmHeight {
        decode_client_state(&client_state.0).map_or_else(
            || {
                tracing::warn!("failed to decode client state, defaulting to height 0");
                EvmHeight(0)
            },
            |cs| EvmHeight(cs.latestHeight.revisionHeight),
        )
    }
}

async fn get_storage_proof(
    chain: &EthereumChain,
    storage_slot: U256,
    height: &EvmHeight,
) -> Result<EvmCommitmentProof> {
    let block_id = alloy::eips::BlockId::number(height.0);
    let proof = chain
        .rpc_guard
        .guarded(|| async {
            chain
                .provider
                .get_proof(chain.router_address, vec![storage_slot.into()])
                .block_id(block_id)
                .await
                .wrap_err("eth_getProof failed")
        })
        .await?;

    let sp = proof.storage_proof.first().ok_or(ProofError::Missing)?;

    Ok(EvmCommitmentProof {
        proof_height: ProofHeight(height.0),
        storage_root: proof.storage_hash,
        account_proof: proof.account_proof.iter().map(|b| b.to_vec()).collect(),
        storage_key: sp.key.as_b256(),
        storage_value: sp.value,
        storage_proof: sp.proof.iter().map(|b| b.to_vec()).collect(),
    })
}

async fn get_commitment_at_height(
    chain: &EthereumChain,
    hashed_path: B256,
    height: &EvmHeight,
) -> Result<B256> {
    let router = ICS26Router::new(chain.router_address, &*chain.provider);
    let result = chain
        .rpc_guard
        .guarded(|| async {
            router
                .getCommitment(hashed_path)
                .block(alloy::eips::BlockId::number(height.0))
                .call()
                .await
                .wrap_err("getCommitment failed")
        })
        .await?;
    Ok(result)
}

/// Fetch commitment value + storage proof for a given ICS24 path key.
async fn query_commitment_with_proof(
    chain: &EthereumChain,
    hashed_path: B256,
    height: &EvmHeight,
) -> Result<(B256, EvmCommitmentProof)> {
    let storage_slot = ics24::commitment_storage_slot(hashed_path);
    let commitment_value = get_commitment_at_height(chain, hashed_path, height).await?;
    let proof = get_storage_proof(chain, storage_slot, height).await?;
    Ok((commitment_value, proof))
}

#[async_trait]
impl PacketStateQuery for EthereumChain {
    #[instrument(skip_all, name = "query_packet_commitment", fields(chain = %self.chain_label(), seq = %sequence))]
    async fn query_packet_commitment(
        &self,
        client_id: &EvmClientId,
        sequence: PacketSequence,
        height: &EvmHeight,
    ) -> Result<(Option<EvmPacketCommitment>, EvmCommitmentProof)> {
        let hashed_path = ics24::packet_commitment_key(&client_id.0, sequence.into());
        let (value, proof) = query_commitment_with_proof(self, hashed_path, height).await?;
        let commitment = (!value.is_zero()).then(|| EvmPacketCommitment(value.to_vec()));
        Ok((commitment, proof))
    }

    #[instrument(skip_all, name = "query_packet_receipt", fields(chain = %self.chain_label(), seq = %sequence))]
    async fn query_packet_receipt(
        &self,
        client_id: &EvmClientId,
        sequence: PacketSequence,
        height: &EvmHeight,
    ) -> Result<(Option<EvmPacketReceipt>, EvmCommitmentProof)> {
        let hashed_path = ics24::packet_receipt_key(&client_id.0, sequence.into());
        let (value, proof) = query_commitment_with_proof(self, hashed_path, height).await?;
        let receipt = (!value.is_zero()).then_some(EvmPacketReceipt);
        Ok((receipt, proof))
    }

    #[instrument(skip_all, name = "query_packet_ack", fields(chain = %self.chain_label(), seq = %sequence))]
    async fn query_packet_acknowledgement(
        &self,
        client_id: &EvmClientId,
        sequence: PacketSequence,
        height: &EvmHeight,
    ) -> Result<(Option<EvmAcknowledgement>, EvmCommitmentProof)> {
        let hashed_path = ics24::ack_commitment_key(&client_id.0, sequence.into());
        let (value, proof) = query_commitment_with_proof(self, hashed_path, height).await?;
        let ack = (!value.is_zero()).then(|| EvmAcknowledgement(value.to_vec()));
        Ok((ack, proof))
    }

    #[instrument(skip_all, name = "query_commitment_sequences", fields(chain = %self.chain_label(), client_id = %client_id))]
    async fn query_commitment_sequences(
        &self,
        client_id: &EvmClientId,
        height: &EvmHeight,
    ) -> Result<Vec<PacketSequence>> {
        let filter = Filter::new()
            .address(self.router_address)
            .event_signature(ICS26Router::SendPacket::SIGNATURE_HASH)
            .from_block(self.config.deployment_block)
            .to_block(height.0);

        let logs = self
            .rpc_guard
            .guarded(|| async {
                self.provider
                    .get_logs(&filter)
                    .await
                    .wrap_err("querying SendPacket logs")
            })
            .await?;

        let mut sequences: Vec<PacketSequence> = logs
            .iter()
            .filter_map(|log| {
                let decoded = ICS26Router::SendPacket::decode_log(log.as_ref()).ok()?;
                if decoded.data.packet.sourceClient == client_id.0 {
                    Some(PacketSequence(decoded.data.packet.sequence))
                } else {
                    None
                }
            })
            .collect();

        sequences.sort_unstable();
        sequences.dedup();
        Ok(sequences)
    }

    #[instrument(skip_all, fields(chain = %self.chain_label(), client_id = %client_id))]
    async fn query_ack_sequences(
        &self,
        client_id: &EvmClientId,
        height: &EvmHeight,
    ) -> Result<Vec<PacketSequence>> {
        let filter = Filter::new()
            .address(self.router_address)
            .event_signature(ICS26Router::WriteAcknowledgement::SIGNATURE_HASH)
            .from_block(self.config.deployment_block)
            .to_block(height.0);

        let logs = self
            .rpc_guard
            .guarded(|| async {
                self.provider
                    .get_logs(&filter)
                    .await
                    .wrap_err("querying WriteAcknowledgement logs")
            })
            .await?;

        let mut sequences: Vec<PacketSequence> = logs
            .iter()
            .filter_map(|log| {
                let decoded = ICS26Router::WriteAcknowledgement::decode_log(log.as_ref()).ok()?;
                if decoded.data.packet.destClient == client_id.0 {
                    Some(PacketSequence(decoded.data.packet.sequence))
                } else {
                    None
                }
            })
            .collect();

        sequences.sort_unstable();
        sequences.dedup();
        Ok(sequences)
    }
}

#[cfg(test)]
mod tests {
    use alloy::primitives::{Bytes, keccak256};
    use alloy::rpc::types::{EIP1186AccountProofResponse, EIP1186StorageProof};
    use alloy::serde::JsonStorageKey;
    use serde_json::{Value, json};

    use super::*;
    use crate::chain::test_support::{LIGHT_CLIENT, ROUTER, RpcResult, chain_at, spawn_rpc};

    fn client_state(height: u64, trusting_period: u32) -> ClientStateReturn {
        ClientStateReturn {
            chainId: "cosmoshub-4".to_string(),
            trustLevel: sp1_ics07::IICS07TendermintMsgs::TrustThreshold {
                numerator: 1,
                denominator: 3,
            },
            latestHeight: sp1_ics07::IICS02ClientMsgs::Height {
                revisionNumber: 4,
                revisionHeight: height,
            },
            trustingPeriod: trusting_period,
            unbondingPeriod: trusting_period * 2,
            isFrozen: false,
            zkAlgorithm: 0,
        }
    }

    fn call_target(params: &Value) -> (Address, Bytes) {
        let tx = &params[0];
        let to = tx["to"].as_str().unwrap().parse().unwrap();
        let input = tx
            .get("input")
            .or_else(|| tx.get("data"))
            .and_then(Value::as_str)
            .unwrap()
            .parse()
            .unwrap();
        (to, input)
    }

    fn block_json(number: u64, timestamp: u64) -> Value {
        let mut block = alloy::rpc::types::Block::<alloy::rpc::types::Transaction>::default();
        block.header.inner.number = number;
        block.header.inner.timestamp = timestamp;
        serde_json::to_value(block).unwrap()
    }

    fn proof_json(storage_proofs: Vec<EIP1186StorageProof>) -> Value {
        serde_json::to_value(EIP1186AccountProofResponse {
            address: ROUTER,
            storage_hash: B256::repeat_byte(0x55),
            account_proof: vec![Bytes::from_static(&[1, 2]), Bytes::from_static(&[3])],
            storage_proof: storage_proofs,
            ..Default::default()
        })
        .unwrap()
    }

    fn storage_proof(key: B256, value: u64) -> EIP1186StorageProof {
        EIP1186StorageProof {
            key: JsonStorageKey::Hash(key),
            value: U256::from(value),
            proof: vec![Bytes::from_static(&[9, 9])],
        }
    }

    /// Serves `getCommitment` and `eth_getProof`, asserting that both target
    /// the ICS24 key derived for `expected_path` at block `height`.
    async fn commitment_server(expected_path: B256, value: B256, with_proof: bool) -> String {
        spawn_rpc(move |method, params| -> RpcResult {
            match method {
                "eth_call" => {
                    let (to, input) = call_target(params);
                    assert_eq!(to, ROUTER);
                    let call = ICS26Router::getCommitmentCall::abi_decode(&input).unwrap();
                    assert_eq!(call.hashedPath, expected_path);
                    assert_eq!(params[1], json!("0x64"));
                    Ok(json!(value))
                }
                "eth_getProof" => {
                    assert_eq!(
                        params[0].as_str().unwrap().parse::<Address>().unwrap(),
                        ROUTER
                    );
                    let slot = ics24::commitment_storage_slot(expected_path);
                    let requested: B256 = params[1][0].as_str().unwrap().parse().unwrap();
                    assert_eq!(requested, B256::from(slot));
                    assert_eq!(params[2], json!("0x64"));
                    let proofs = if with_proof {
                        vec![storage_proof(B256::from(slot), 7)]
                    } else {
                        vec![]
                    };
                    Ok(proof_json(proofs))
                }
                other => Err(format!("unexpected {other}")),
            }
        })
        .await
    }

    #[test]
    fn client_state_roundtrip() {
        let cs = client_state(1234, 86_400);
        let decoded = decode_client_state(&encode_client_state(&cs)).unwrap();
        assert_eq!(decoded.chainId, "cosmoshub-4");
        assert_eq!(decoded.latestHeight.revisionHeight, 1234);
        assert_eq!(decoded.trustingPeriod, 86_400);
    }

    #[test]
    fn decode_client_state_rejects_garbage() {
        assert!(decode_client_state(&[]).is_none());
        assert!(decode_client_state(&[0xde, 0xad, 0xbe, 0xef]).is_none());
    }

    #[test]
    fn trusting_period_and_latest_height_from_encoded_state() {
        let cs = EvmClientState(encode_client_state(&client_state(77, 3600)));
        assert_eq!(
            <EthereumChain as ClientQuery<EthereumChain>>::trusting_period(&cs),
            Some(Duration::from_secs(3600))
        );
        assert_eq!(
            <EthereumChain as ClientQuery<EthereumChain>>::client_latest_height(&cs),
            EvmHeight(77)
        );
    }

    #[test]
    fn undecodable_client_state_has_no_trusting_period_and_zero_height() {
        let cs = EvmClientState(vec![1, 2, 3]);
        assert_eq!(
            <EthereumChain as ClientQuery<EthereumChain>>::trusting_period(&cs),
            None
        );
        assert_eq!(
            <EthereumChain as ClientQuery<EthereumChain>>::client_latest_height(&cs),
            EvmHeight(0)
        );
    }

    #[tokio::test]
    async fn chain_status_reads_latest_block() {
        let url = spawn_rpc(|method, params| match method {
            "eth_blockNumber" => Ok(json!("0x2a")),
            "eth_getBlockByNumber" => {
                assert_eq!(params[0], json!("0x2a"));
                Ok(block_json(42, 1_700_000_123))
            }
            other => Err(format!("unexpected {other}")),
        })
        .await;
        let status = chain_at(&url).query_chain_status().await.unwrap();
        assert_eq!(status.height, EvmHeight(42));
        assert_eq!(status.timestamp, EvmTimestamp(1_700_000_123));
    }

    #[tokio::test]
    async fn chain_status_missing_block_is_stale_state() {
        let url = spawn_rpc(|method, _| match method {
            "eth_blockNumber" => Ok(json!("0x5")),
            _ => Ok(Value::Null),
        })
        .await;
        let err = chain_at(&url).query_chain_status().await.unwrap_err();
        let query_err = err.downcast_ref::<QueryError>().unwrap();
        assert!(matches!(query_err, QueryError::StaleState { what } if what == "block 5"));
    }

    fn light_client_server(
        on_lc_call: fn(&Bytes) -> RpcResult,
    ) -> impl Fn(&str, &Value) -> RpcResult {
        move |method, params| {
            assert_eq!(method, "eth_call");
            let (to, input) = call_target(params);
            if to == ROUTER {
                let call = ICS26Router::getClientCall::abi_decode(&input).unwrap();
                assert_eq!(call.clientId, "client-3");
                return Ok(json!(B256::left_padding_from(LIGHT_CLIENT.as_slice())));
            }
            assert_eq!(to, LIGHT_CLIENT);
            on_lc_call(&input)
        }
    }

    #[tokio::test]
    async fn query_client_state_resolves_light_client_and_reencodes() {
        let url = spawn_rpc(light_client_server(|input| {
            SP1ICS07Tendermint::clientStateCall::abi_decode(input).unwrap();
            let encoded = encode_client_state(&client_state(900, 1209));
            Ok(json!(Bytes::from(encoded)))
        }))
        .await;
        let cs = chain_at(&url)
            .query_client_state(&EvmClientId("client-3".to_string()), &EvmHeight(0))
            .await
            .unwrap();
        let decoded = decode_client_state(&cs.0).unwrap();
        assert_eq!(decoded.latestHeight.revisionHeight, 900);
        assert_eq!(decoded.trustingPeriod, 1209);
    }

    #[tokio::test]
    async fn query_consensus_state_returns_hash_bytes() {
        let url = spawn_rpc(light_client_server(|input| {
            let call = SP1ICS07Tendermint::getConsensusStateHashCall::abi_decode(input).unwrap();
            assert_eq!(call.revisionHeight, 55);
            Ok(json!(B256::repeat_byte(0x77)))
        }))
        .await;
        let cs = chain_at(&url)
            .query_consensus_state(
                &EvmClientId("client-3".to_string()),
                &EvmHeight(55),
                &EvmHeight(0),
            )
            .await
            .unwrap();
        assert_eq!(cs.0, vec![0x77; 32]);
    }

    #[tokio::test]
    async fn resolve_light_client_propagates_revert() {
        let url = spawn_rpc(|_, _| Err("execution reverted".to_string())).await;
        let err = resolve_light_client(&chain_at(&url), &EvmClientId("nope".to_string()))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("getClient(nope)"), "{err:#}");
    }

    #[tokio::test]
    async fn packet_commitment_present_with_proof() {
        let path = ics24::packet_commitment_key("client-0", 3);
        let url = commitment_server(path, B256::repeat_byte(0xcc), true).await;
        let (commitment, proof) = chain_at(&url)
            .query_packet_commitment(
                &EvmClientId("client-0".to_string()),
                PacketSequence(3),
                &EvmHeight(100),
            )
            .await
            .unwrap();
        assert_eq!(commitment.unwrap().0, vec![0xcc; 32]);
        assert_eq!(proof.proof_height, ProofHeight(100));
        assert_eq!(proof.storage_root, B256::repeat_byte(0x55));
        assert_eq!(proof.account_proof, vec![vec![1, 2], vec![3]]);
        assert_eq!(
            proof.storage_key,
            B256::from(ics24::commitment_storage_slot(path))
        );
        assert_eq!(proof.storage_value, U256::from(7));
        assert_eq!(proof.storage_proof, vec![vec![9, 9]]);
    }

    #[tokio::test]
    async fn packet_commitment_zero_value_is_absent() {
        let path = ics24::packet_commitment_key("client-0", 3);
        let url = commitment_server(path, B256::ZERO, true).await;
        let (commitment, _) = chain_at(&url)
            .query_packet_commitment(
                &EvmClientId("client-0".to_string()),
                PacketSequence(3),
                &EvmHeight(100),
            )
            .await
            .unwrap();
        assert!(commitment.is_none());
    }

    #[tokio::test]
    async fn packet_commitment_without_storage_proof_errors() {
        let path = ics24::packet_commitment_key("client-0", 3);
        let url = commitment_server(path, B256::repeat_byte(1), false).await;
        let err = chain_at(&url)
            .query_packet_commitment(
                &EvmClientId("client-0".to_string()),
                PacketSequence(3),
                &EvmHeight(100),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err.downcast_ref::<ProofError>(),
            Some(ProofError::Missing)
        ));
    }

    #[tokio::test]
    async fn packet_receipt_uses_receipt_key() {
        let path = ics24::packet_receipt_key("client-1", 8);
        let url = commitment_server(path, B256::repeat_byte(1), true).await;
        let chain = chain_at(&url);
        let (receipt, _) = chain
            .query_packet_receipt(
                &EvmClientId("client-1".to_string()),
                PacketSequence(8),
                &EvmHeight(100),
            )
            .await
            .unwrap();
        assert!(receipt.is_some());

        let url = commitment_server(path, B256::ZERO, true).await;
        let (receipt, _) = chain_at(&url)
            .query_packet_receipt(
                &EvmClientId("client-1".to_string()),
                PacketSequence(8),
                &EvmHeight(100),
            )
            .await
            .unwrap();
        assert!(receipt.is_none());
    }

    #[tokio::test]
    async fn packet_ack_uses_ack_key() {
        let path = ics24::ack_commitment_key("client-1", 8);
        let url = commitment_server(path, B256::repeat_byte(0xab), true).await;
        let (ack, _) = chain_at(&url)
            .query_packet_acknowledgement(
                &EvmClientId("client-1".to_string()),
                PacketSequence(8),
                &EvmHeight(100),
            )
            .await
            .unwrap();
        assert_eq!(ack.unwrap().0, vec![0xab; 32]);

        let url = commitment_server(path, B256::ZERO, true).await;
        let (ack, _) = chain_at(&url)
            .query_packet_acknowledgement(
                &EvmClientId("client-1".to_string()),
                PacketSequence(8),
                &EvmHeight(100),
            )
            .await
            .unwrap();
        assert!(ack.is_none());
    }

    fn packet(source: &str, dest: &str, seq: u64) -> crate::contracts::IICS26RouterMsgs::Packet {
        crate::contracts::IICS26RouterMsgs::Packet {
            sequence: seq,
            sourceClient: source.to_string(),
            destClient: dest.to_string(),
            timeoutTimestamp: 0,
            payloads: vec![],
        }
    }

    fn log_json(data: alloy::primitives::LogData) -> Value {
        serde_json::to_value(alloy::rpc::types::Log {
            inner: alloy::primitives::Log {
                address: ROUTER,
                data,
            },
            block_number: Some(1),
            ..Default::default()
        })
        .unwrap()
    }

    fn send_log(source: &str, seq: u64) -> Value {
        log_json(
            ICS26Router::SendPacket {
                clientId: keccak256(source),
                sequence: U256::from(seq),
                packet: packet(source, "dst", seq),
            }
            .encode_log_data(),
        )
    }

    fn ack_log(dest: &str, seq: u64) -> Value {
        log_json(
            ICS26Router::WriteAcknowledgement {
                clientId: keccak256(dest),
                sequence: U256::from(seq),
                packet: packet("src", dest, seq),
                acknowledgements: vec![],
            }
            .encode_log_data(),
        )
    }

    fn garbage_log() -> Value {
        log_json(alloy::primitives::LogData::new_unchecked(
            vec![ICS26Router::SendPacket::SIGNATURE_HASH],
            Bytes::from_static(&[1]),
        ))
    }

    #[tokio::test]
    async fn commitment_sequences_filtered_sorted_deduped() {
        let logs = json!([
            send_log("client-0", 5),
            send_log("other", 2),
            send_log("client-0", 1),
            send_log("client-0", 5),
            garbage_log(),
        ]);
        let url = spawn_rpc(move |_, params| {
            assert_eq!(params[0]["fromBlock"], json!("0x7"));
            assert_eq!(params[0]["toBlock"], json!("0x64"));
            Ok(logs.clone())
        })
        .await;
        let seqs = chain_at(&url)
            .query_commitment_sequences(&EvmClientId("client-0".to_string()), &EvmHeight(100))
            .await
            .unwrap();
        assert_eq!(seqs, vec![PacketSequence(1), PacketSequence(5)]);
    }

    #[tokio::test]
    async fn ack_sequences_match_dest_client() {
        let logs = json!([
            ack_log("client-1", 9),
            ack_log("client-1", 3),
            ack_log("client-2", 4),
            ack_log("client-1", 3),
        ]);
        let url = spawn_rpc(move |_, params| {
            let topic0: B256 = params[0]["topics"][0].as_str().unwrap().parse().unwrap();
            assert_eq!(topic0, ICS26Router::WriteAcknowledgement::SIGNATURE_HASH);
            Ok(logs.clone())
        })
        .await;
        let seqs = chain_at(&url)
            .query_ack_sequences(&EvmClientId("client-1".to_string()), &EvmHeight(100))
            .await
            .unwrap();
        assert_eq!(seqs, vec![PacketSequence(3), PacketSequence(9)]);
    }

    #[tokio::test]
    async fn sequence_queries_propagate_rpc_errors() {
        let url = spawn_rpc(|_, _| Err("boom".to_string())).await;
        let chain = chain_at(&url);
        let id = EvmClientId("c".to_string());
        assert!(
            chain
                .query_commitment_sequences(&id, &EvmHeight(1))
                .await
                .is_err()
        );
        assert!(chain.query_ack_sequences(&id, &EvmHeight(1)).await.is_err());
    }

    #[test]
    fn fuzz_decode_client_state() {
        bolero::check!().for_each(|bytes: &[u8]| {
            if let Some(cs) = decode_client_state(bytes) {
                let reencoded = encode_client_state(&cs);
                assert!(decode_client_state(&reencoded).is_some());
            }
        });
    }
}
