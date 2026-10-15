use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcBlockConfig, RpcSendTransactionConfig, RpcTransactionConfig};
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
        self.send_and_confirm_versioned_transaction_inner(tx, false)
            .await
    }

    pub async fn send_and_confirm_versioned_transaction_skip_preflight(
        &self,
        tx: &VersionedTransaction,
    ) -> eyre::Result<Signature> {
        self.send_and_confirm_versioned_transaction_inner(tx, true)
            .await
    }

    async fn send_and_confirm_versioned_transaction_inner(
        &self,
        tx: &VersionedTransaction,
        skip_preflight: bool,
    ) -> eyre::Result<Signature> {
        let client = self.client.clone();
        let tx = tx.clone();
        self.guard
            .guarded(|| async move {
                if skip_preflight {
                    let config = RpcSendTransactionConfig {
                        skip_preflight: true,
                        ..Default::default()
                    };
                    let sig = client
                        .send_transaction_with_config(&tx, config)
                        .await
                        .map_err(|e| eyre::eyre!("send_versioned_transaction failed: {e}"))?;
                    tracing::debug!(%sig, "transaction sent (skip_preflight), confirming...");
                    client
                        .confirm_transaction_with_spinner(
                            &sig,
                            tx.message.recent_blockhash(),
                            client.commitment(),
                        )
                        .await
                        .map_err(|e| eyre::eyre!("confirm_versioned_transaction failed: {e}"))?;
                    Ok(sig)
                } else {
                    client.send_and_confirm_transaction(&tx).await.map_err(|e| {
                        eyre::eyre!("send_and_confirm_versioned_transaction failed: {e}")
                    })
                }
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

    pub async fn get_program_accounts_with_filter(
        &self,
        program_id: &Pubkey,
        filters: Vec<solana_client::rpc_filter::RpcFilterType>,
    ) -> eyre::Result<Vec<(Pubkey, Account)>> {
        use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};

        let client = self.client.clone();
        let pid = *program_id;
        let ui_accounts = self
            .guard
            .guarded(|| async move {
                let config = RpcProgramAccountsConfig {
                    filters: Some(filters),
                    account_config: RpcAccountInfoConfig {
                        commitment: Some(CommitmentConfig::confirmed()),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                client
                    .get_program_ui_accounts_with_config(&pid, config)
                    .await
                    .map_err(|e| eyre::eyre!("get_program_accounts failed: {e}"))
            })
            .await?;

        ui_accounts
            .into_iter()
            .map(|(pubkey, ui_account)| {
                let account: Account = ui_account
                    .decode()
                    .ok_or_else(|| eyre::eyre!("failed to decode UiAccount for {pubkey}"))?;
                Ok((pubkey, account))
            })
            .collect()
    }
}

impl std::fmt::Debug for SolanaRpcClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SolanaRpcClient")
            .field("guard", &self.guard)
            .finish_non_exhaustive()
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod testing;

#[cfg(test)]
mod tests {
    use solana_sdk::hash::Hash;
    use solana_sdk::signer::Signer;
    use solana_sdk::signer::keypair::Keypair;

    use super::testing::FakeRpc;
    use super::*;

    #[tokio::test]
    async fn get_account_distinguishes_missing_from_present() {
        let present = Pubkey::new_unique();
        let rpc = FakeRpc::default()
            .with_account(present, vec![1, 2, 3])
            .into_client();

        let account = rpc.get_account(&present).await.unwrap().unwrap();
        assert_eq!(account.data, vec![1, 2, 3]);
        assert!(
            rpc.get_account(&Pubkey::new_unique())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn slot_and_block_time_pass_through() {
        let rpc = FakeRpc {
            slot: Some(42),
            block_time: Some(1_700_000_000),
            ..FakeRpc::default()
        }
        .into_client();

        assert_eq!(rpc.get_slot().await.unwrap(), 42);
        assert_eq!(
            rpc.get_slot_with_commitment(CommitmentConfig::finalized())
                .await
                .unwrap(),
            42
        );
        assert_eq!(rpc.get_block_time(42).await.unwrap(), 1_700_000_000);
    }

    #[tokio::test]
    async fn malformed_responses_surface_as_labelled_errors() {
        let rpc = FakeRpc::default().into_client();

        let err = rpc.get_slot().await.unwrap_err().to_string();
        assert!(err.contains("get_slot failed"), "{err}");
        let err = rpc.get_block_time(9).await.unwrap_err().to_string();
        assert!(err.contains("get_block_time(9) failed"), "{err}");
        let err = rpc.get_block(9).await.unwrap_err().to_string();
        assert!(err.contains("get_block(9) failed"), "{err}");
        let err = rpc
            .get_transaction(&Signature::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("get_transaction("), "{err}");
    }

    #[tokio::test]
    async fn unreachable_endpoint_errors_instead_of_hanging() {
        let config = SolanaChainConfig {
            rpc_addr: "http://127.0.0.1:1".into(),
            ws_addr: None,
            program_id: Pubkey::new_unique().to_string(),
            ics07_program_id: None,
            keypair_path: "/nonexistent".into(),
            block_time: std::time::Duration::from_millis(400),
            rpc_timeout_secs: 5,
            rpc_rate_limit: 10,
            alt_address: None,
            skip_pre_verify_threshold: None,
            attestation: None,
        };
        let rpc = SolanaRpcClient::new(&config);
        assert!(rpc.get_slot().await.is_err());
        assert!(format!("{rpc:?}").starts_with("SolanaRpcClient"));
    }

    #[tokio::test]
    async fn send_and_confirm_returns_the_transaction_signature() {
        let fake = FakeRpc::default();
        let sent = fake.sent_txs.clone();
        let rpc = fake.into_client();

        let payer = Keypair::new();
        let blockhash = rpc.get_latest_blockhash().await.unwrap();
        assert_eq!(blockhash, Hash::new_from_array([7; 32]));

        let ix = solana_system_interface::instruction::transfer(
            &payer.pubkey(),
            &Pubkey::new_unique(),
            1,
        );
        let msg = solana_message::v0::Message::try_compile(&payer.pubkey(), &[ix], &[], blockhash)
            .unwrap();
        let tx =
            VersionedTransaction::try_new(solana_message::VersionedMessage::V0(msg), &[&payer])
                .unwrap();

        let sig = rpc
            .send_and_confirm_versioned_transaction(&tx)
            .await
            .unwrap();
        assert_eq!(sig, tx.signatures[0]);
        assert_eq!(sent.lock().unwrap().len(), 1);

        let statuses = rpc.get_signature_statuses(&[sig, sig]).await.unwrap();
        assert_eq!(statuses.len(), 2);
        assert!(statuses.iter().all(Option::is_some));
    }

    fn transfer_ix(payer: &Keypair) -> solana_sdk::instruction::Instruction {
        solana_system_interface::instruction::transfer(&payer.pubkey(), &Pubkey::new_unique(), 1)
    }

    #[tokio::test]
    async fn get_account_rpc_failure_is_an_error_not_a_missing_account() {
        let pk = Pubkey::new_unique();
        let rpc = FakeRpc {
            failing: vec![solana_client::rpc_request::RpcRequest::GetAccountInfo],
            ..FakeRpc::default()
        }
        .with_account(pk, vec![1])
        .into_client();
        let err = rpc.get_account(&pk).await.unwrap_err().to_string();
        assert!(err.contains(&format!("get_account({pk}) failed")), "{err}");
    }

    #[tokio::test]
    async fn legacy_send_and_confirm_returns_the_transaction_signature() {
        let fake = FakeRpc::default();
        let sent = fake.sent_txs.clone();
        let rpc = fake.into_client();
        let payer = Keypair::new();
        let blockhash = rpc.get_latest_blockhash().await.unwrap();
        let tx = Transaction::new_signed_with_payer(
            &[transfer_ix(&payer)],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );

        let sig = rpc.send_and_confirm_transaction(&tx).await.unwrap();
        assert_eq!(sig, tx.signatures[0]);
        assert_eq!(sent.lock().unwrap().len(), 1);

        let failing = FakeRpc {
            failing: vec![solana_client::rpc_request::RpcRequest::SendTransaction],
            ..FakeRpc::default()
        }
        .into_client();
        let err = failing.send_and_confirm_transaction(&tx).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("send_and_confirm_transaction failed"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn skip_preflight_send_confirms_and_labels_failures() {
        let fake = FakeRpc::default();
        let sent = fake.sent_txs.clone();
        let rpc = fake.into_client();
        let payer = Keypair::new();

        let sig = crate::tx::send_transaction_skip_preflight(
            &rpc,
            &payer,
            vec![transfer_ix(&payer)],
            None,
        )
        .await
        .unwrap();
        {
            let wire = sent.lock().unwrap();
            assert_eq!(wire.len(), 1);
            assert_eq!(&wire[0][1..65], sig.as_ref());
        }

        let unsendable = FakeRpc {
            failing: vec![solana_client::rpc_request::RpcRequest::SendTransaction],
            ..FakeRpc::default()
        }
        .into_client();
        let err = crate::tx::send_transaction_skip_preflight(
            &unsendable,
            &payer,
            vec![transfer_ix(&payer)],
            None,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("send_versioned_transaction failed"),
            "{err}"
        );

        let unconfirmable = FakeRpc {
            failing: vec![solana_client::rpc_request::RpcRequest::GetSignatureStatuses],
            ..FakeRpc::default()
        }
        .into_client();
        let err = crate::tx::send_transaction_skip_preflight(
            &unconfirmable,
            &payer,
            vec![transfer_ix(&payer)],
            None,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("confirm_versioned_transaction failed"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn versioned_send_failure_is_labelled() {
        let payer = Keypair::new();
        let rpc = FakeRpc {
            failing: vec![solana_client::rpc_request::RpcRequest::SendTransaction],
            ..FakeRpc::default()
        }
        .into_client();
        let err = crate::tx::send_transaction(&rpc, &payer, vec![transfer_ix(&payer)], None)
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("send_and_confirm_versioned_transaction failed"),
            "{err}"
        );

        let no_blockhash = FakeRpc {
            failing: vec![solana_client::rpc_request::RpcRequest::GetLatestBlockhash],
            ..FakeRpc::default()
        }
        .into_client();
        let err =
            crate::tx::send_transaction(&no_blockhash, &payer, vec![transfer_ix(&payer)], None)
                .await
                .unwrap_err();
        assert!(
            err.to_string().contains("get_latest_blockhash failed"),
            "{err}"
        );

        let no_status = FakeRpc {
            failing: vec![solana_client::rpc_request::RpcRequest::GetSignatureStatuses],
            ..FakeRpc::default()
        }
        .into_client();
        let err = no_status
            .get_signature_statuses(&[Signature::default()])
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("get_signature_statuses failed"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn program_accounts_apply_memcmp_and_size_filters() {
        use solana_client::rpc_filter::{Memcmp, RpcFilterType};

        let program = Pubkey::new_unique();
        let (a, b, c) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let rpc = FakeRpc::default()
            .with_program_account(program, a, vec![1, 2, 3, 4])
            .with_program_account(program, b, vec![1, 2, 9])
            .with_program_account(program, c, vec![7, 2, 3, 4])
            .with_program_account(Pubkey::new_unique(), Pubkey::new_unique(), vec![1, 2, 3, 4])
            .into_client();

        let prefix = |bytes: &[u8]| RpcFilterType::Memcmp(Memcmp::new_raw_bytes(0, bytes.to_vec()));
        let keys = |accounts: Vec<(Pubkey, Account)>| {
            let mut k: Vec<_> = accounts.into_iter().map(|(pk, _)| pk).collect();
            k.sort();
            k
        };
        let mut ab = vec![a, b];
        ab.sort();

        let found = rpc
            .get_program_accounts_with_filter(&program, vec![prefix(&[1, 2])])
            .await
            .unwrap();
        assert_eq!(keys(found.clone()), ab);
        assert!(
            found
                .iter()
                .any(|(pk, acc)| *pk == a && acc.data == vec![1, 2, 3, 4])
        );

        let found = rpc
            .get_program_accounts_with_filter(
                &program,
                vec![prefix(&[1, 2]), RpcFilterType::DataSize(4)],
            )
            .await
            .unwrap();
        assert_eq!(keys(found), vec![a]);

        let all = rpc
            .get_program_accounts_with_filter(&program, vec![])
            .await
            .unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(
            rpc.get_program_accounts_with_filter(&Pubkey::new_unique(), vec![])
                .await
                .unwrap(),
            Vec::<(Pubkey, Account)>::new()
        );

        let failing = FakeRpc {
            failing: vec![solana_client::rpc_request::RpcRequest::GetProgramAccounts],
            ..FakeRpc::default()
        }
        .into_client();
        let err = failing
            .get_program_accounts_with_filter(&program, vec![])
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("get_program_accounts failed"),
            "{err}"
        );
    }
}
