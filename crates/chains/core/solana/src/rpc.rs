use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcBlockConfig, RpcTransactionConfig};
use solana_commitment_config::CommitmentConfig;
use solana_sdk::account::Account;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::transaction::Transaction;
use solana_transaction::versioned::VersionedTransaction;
use solana_transaction_status::{
    EncodedConfirmedTransactionWithStatusMeta, TransactionDetails, UiConfirmedBlock,
    UiTransactionEncoding,
};

use mercury_core::rpc_guard::RpcGuard;

use crate::config::SolanaChainConfig;

#[derive(Clone)]
pub struct SolanaRpcClient {
    client: std::sync::Arc<RpcClient>,
    guard: RpcGuard,
}

impl SolanaRpcClient {
    #[must_use]
    pub fn new(config: &SolanaChainConfig) -> Self {
        let rpc_config = config.rpc_config();
        let client = std::sync::Arc::new(RpcClient::new_with_timeout_and_commitment(
            config.rpc_addr.clone(),
            std::time::Duration::from_secs(config.rpc_timeout_secs),
            CommitmentConfig::confirmed(),
        ));
        let guard = RpcGuard::new("solana", rpc_config);
        Self { client, guard }
    }

    pub async fn get_slot(&self) -> eyre::Result<u64> {
        let client = self.client.clone();
        self.guard
            .guarded(|| async move {
                client
                    .get_slot()
                    .await
                    .map_err(|e| eyre::eyre!("get_slot failed: {e}"))
            })
            .await
    }

    pub async fn get_slot_with_commitment(
        &self,
        commitment: CommitmentConfig,
    ) -> eyre::Result<u64> {
        let client = self.client.clone();
        let slot = self
            .guard
            .guarded(|| async move {
                client
                    .get_slot_with_commitment(commitment)
                    .await
                    .map_err(|e| eyre::eyre!("get_slot failed: {e}"))
            })
            .await?;
        tracing::trace!(slot, ?commitment, "get_slot");
        Ok(slot)
    }

    pub async fn get_block_time(&self, slot: u64) -> eyre::Result<i64> {
        let client = self.client.clone();
        self.guard
            .guarded(|| async move {
                client
                    .get_block_time(slot)
                    .await
                    .map_err(|e| eyre::eyre!("get_block_time({slot}) failed: {e}"))
            })
            .await
    }

    pub async fn get_account(&self, pubkey: &Pubkey) -> eyre::Result<Option<Account>> {
        let client = self.client.clone();
        let pk = *pubkey;
        self.guard
            .guarded(|| async move {
                match client
                    .get_account_with_commitment(&pk, CommitmentConfig::confirmed())
                    .await
                {
                    Ok(response) => Ok(response.value),
                    Err(e) => Err(eyre::eyre!("get_account({pk}) failed: {e}")),
                }
            })
            .await
    }

    pub async fn get_block(&self, slot: u64) -> eyre::Result<UiConfirmedBlock> {
        let client = self.client.clone();
        self.guard
            .guarded(|| async move {
                let config = RpcBlockConfig {
                    encoding: Some(UiTransactionEncoding::Base64),
                    transaction_details: Some(TransactionDetails::Full),
                    rewards: Some(false),
                    commitment: Some(CommitmentConfig::confirmed()),
                    max_supported_transaction_version: Some(0),
                };
                client
                    .get_block_with_config(slot, config)
                    .await
                    .map_err(|e| eyre::eyre!("get_block({slot}) failed: {e}"))
            })
            .await
    }

    pub async fn get_transaction(
        &self,
        signature: &Signature,
    ) -> eyre::Result<EncodedConfirmedTransactionWithStatusMeta> {
        let client = self.client.clone();
        let sig = *signature;
        self.guard
            .guarded(|| async move {
                let config = RpcTransactionConfig {
                    encoding: Some(UiTransactionEncoding::Base64),
                    commitment: Some(CommitmentConfig::confirmed()),
                    max_supported_transaction_version: Some(0),
                };
                client
                    .get_transaction_with_config(&sig, config)
                    .await
                    .map_err(|e| eyre::eyre!("get_transaction({sig}) failed: {e}"))
            })
            .await
    }

    pub async fn get_latest_blockhash(&self) -> eyre::Result<solana_sdk::hash::Hash> {
        let client = self.client.clone();
        self.guard
            .guarded(|| async move {
                client
                    .get_latest_blockhash()
                    .await
                    .map_err(|e| eyre::eyre!("get_latest_blockhash failed: {e}"))
            })
            .await
    }

    pub async fn send_and_confirm_transaction(&self, tx: &Transaction) -> eyre::Result<Signature> {
        let client = self.client.clone();
        let tx = tx.clone();
        self.guard
            .guarded(|| async move {
                client
                    .send_and_confirm_transaction(&tx)
                    .await
                    .map_err(|e| eyre::eyre!("send_and_confirm_transaction failed: {e}"))
            })
            .await
    }

    pub async fn send_and_confirm_versioned_transaction(
        &self,
        tx: &VersionedTransaction,
    ) -> eyre::Result<Signature> {
        let client = self.client.clone();
        let tx = tx.clone();
        self.guard
            .guarded(|| async move {
                client
                    .send_and_confirm_transaction(&tx)
                    .await
                    .map_err(|e| eyre::eyre!("send_and_confirm_versioned_transaction failed: {e}"))
            })
            .await
    }

    pub async fn get_signature_statuses(
        &self,
        signatures: &[Signature],
    ) -> eyre::Result<Vec<Option<solana_transaction_status::TransactionStatus>>> {
        let client = self.client.clone();
        let sigs = signatures.to_vec();
        self.guard
            .guarded(|| async move {
                client
                    .get_signature_statuses(&sigs)
                    .await
                    .map(|r| r.value)
                    .map_err(|e| eyre::eyre!("get_signature_statuses failed: {e}"))
            })
            .await
    }
}

impl std::fmt::Debug for SolanaRpcClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SolanaRpcClient")
            .field("guard", &self.guard)
            .finish_non_exhaustive()
    }
}
