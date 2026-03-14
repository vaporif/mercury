//! Cross-chain trait implementations for Cosmos → EVM client relay.
//! Gated behind the `cosmos-sp1` feature.

use std::time::Duration;

use alloy::primitives::U256;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use eyre::Context;
use tendermint::block::Height as TmHeight;
use tracing::instrument;

use mercury_chain_traits::builders::ClientMessageBuilder;
use mercury_chain_traits::queries::ClientQuery;
use mercury_core::MerklePrefix;
use mercury_core::error::Result;

use mercury_cosmos::builders::{CosmosCreateClientPayload, CosmosUpdateClientPayload};
use mercury_cosmos::chain::CosmosChain;
use mercury_cosmos::keys::CosmosSigner;

use crate::chain::{EthereumChain, to_sol_merkle_prefix};
use crate::contracts::{ICS26Router, IICS02ClientMsgs, SP1ICS07Tendermint};
use crate::queries::{decode_client_state, resolve_light_client};
use crate::types::{EvmClientId, EvmHeight, EvmMessage};

#[async_trait]
impl<S: CosmosSigner> ClientQuery<CosmosChain<S>> for EthereumChain {
    #[instrument(skip_all, name = "query_client_state", fields(client_id = %client_id))]
    async fn query_client_state(
        &self,
        client_id: &EvmClientId,
        _height: &EvmHeight,
    ) -> Result<Vec<u8>> {
        let lc_address = resolve_light_client(self, client_id).await?;
        let lc = SP1ICS07Tendermint::new(lc_address, &*self.provider);
        let result = lc
            .getClientState()
            .call()
            .await
            .wrap_err("SP1ICS07Tendermint.getClientState() failed")?;
        Ok(result.to_vec())
    }

    #[instrument(skip_all, name = "query_consensus_state", fields(client_id = %client_id, consensus_height = %consensus_height))]
    async fn query_consensus_state(
        &self,
        client_id: &EvmClientId,
        consensus_height: &TmHeight,
        _query_height: &EvmHeight,
    ) -> Result<Vec<u8>> {
        let height_u64 = consensus_height.value();
        let lc_address = resolve_light_client(self, client_id).await?;
        let lc = SP1ICS07Tendermint::new(lc_address, &*self.provider);
        let result = lc
            .getConsensusStateHash(height_u64)
            .call()
            .await
            .wrap_err_with(|| format!("getConsensusStateHash({height_u64}) failed"))?;
        Ok(result.to_vec())
    }

    fn trusting_period(client_state: &Vec<u8>) -> Option<Duration> {
        let cs = decode_client_state(client_state)?;
        Some(Duration::from_secs(u64::from(cs.trustingPeriod)))
    }

    fn client_latest_height(client_state: &Vec<u8>) -> TmHeight {
        decode_client_state(client_state).map_or_else(
            || {
                tracing::warn!("failed to decode client state, defaulting to height 1");
                TmHeight::try_from(1u64).expect("height 1 is valid")
            },
            |cs| {
                TmHeight::try_from(cs.latestHeight.revisionHeight)
                    .unwrap_or_else(|_| TmHeight::try_from(1u64).expect("height 1 is valid"))
            },
        )
    }
}

#[async_trait]
impl<S: CosmosSigner> ClientMessageBuilder<CosmosChain<S>> for EthereumChain {
    type CreateClientPayload = CosmosCreateClientPayload;
    type UpdateClientPayload = CosmosUpdateClientPayload;

    async fn build_create_client_message(
        &self,
        _payload: CosmosCreateClientPayload,
    ) -> Result<EvmMessage> {
        // Cross-chain client creation requires encoding Cosmos client/consensus state
        // for the SP1 contract. This is a one-time manual operation, deferred for now.
        todo!("cross-chain create client not yet implemented")
    }

    async fn build_update_client_message(
        &self,
        client_id: &EvmClientId,
        payload: CosmosUpdateClientPayload,
    ) -> Result<Vec<EvmMessage>> {
        let sp1 = self
            .sp1
            .as_ref()
            .ok_or_else(|| eyre::eyre!("SP1 prover not configured"))?;

        let headers: Vec<Vec<u8>> = payload
            .headers
            .iter()
            .map(prost::Message::encode_to_vec)
            .collect();

        self.build_update_client_message_sp1(
            client_id,
            headers,
            payload.trusted_consensus_state,
            sp1,
        )
        .await
    }

    async fn build_register_counterparty_message(
        &self,
        client_id: &EvmClientId,
        counterparty_client_id: &<CosmosChain<S> as mercury_chain_traits::types::ChainTypes>::ClientId,
        counterparty_merkle_prefix: MerklePrefix,
    ) -> Result<EvmMessage> {
        let call = ICS26Router::migrateClientCall {
            clientId: client_id.0.clone(),
            counterpartyInfo: IICS02ClientMsgs::CounterpartyInfo {
                clientId: counterparty_client_id.to_string(),
                merklePrefix: to_sol_merkle_prefix(&counterparty_merkle_prefix),
            },
            client: self.config.light_client_address()?,
        };

        Ok(EvmMessage {
            to: self.router_address,
            calldata: call.abi_encode(),
            value: U256::ZERO,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mercury_chain_traits::builders::ClientPayloadBuilder;
    use mercury_chain_traits::types::IbcTypes;

    /// Compile-time verification that cross-chain trait bounds are satisfied.
    fn _assert_cross_chain_traits<S: CosmosSigner>()
    where
        EthereumChain:
            IbcTypes + ClientQuery<CosmosChain<S>> + ClientMessageBuilder<CosmosChain<S>>,
        CosmosChain<S>: ClientPayloadBuilder<EthereumChain>,
    {
    }
}
