use solana_message::AddressLookupTableAccount;
use solana_sdk::instruction::Instruction;
use solana_sdk::signature::Signature;
use solana_sdk::signer::Signer;
use solana_sdk::signer::keypair::Keypair;
use solana_transaction::versioned::VersionedTransaction;

use crate::rpc::SolanaRpcClient;

/// Send a single batch of instructions as a V0 transaction.
pub async fn send_transaction(
    rpc: &SolanaRpcClient,
    keypair: &Keypair,
    instructions: Vec<Instruction>,
    alt: Option<&[AddressLookupTableAccount]>,
) -> eyre::Result<Signature> {
    let blockhash = rpc.get_latest_blockhash().await?;
    let alts = alt.unwrap_or(&[]);
    let msg = solana_message::v0::Message::try_compile(
        &keypair.pubkey(),
        &instructions,
        alts,
        blockhash,
    )?;
    let versioned_msg = solana_message::VersionedMessage::V0(msg);
    let tx = VersionedTransaction::try_new(versioned_msg, &[keypair])?;
    rpc.send_and_confirm_versioned_transaction(&tx).await
}
