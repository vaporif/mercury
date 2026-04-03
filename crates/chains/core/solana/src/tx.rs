use std::sync::Arc;

use solana_sdk::instruction::Instruction;
use solana_sdk::signature::Signature;
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::rpc::SolanaRpcClient;

pub async fn send_transaction(
    rpc: &SolanaRpcClient,
    keypair: &Keypair,
    instructions: Vec<Instruction>,
) -> eyre::Result<Signature> {
    let blockhash = rpc.get_latest_blockhash().await?;
    let tx = Transaction::new_signed_with_payer(
        &instructions,
        Some(&keypair.pubkey()),
        &[keypair],
        blockhash,
    );
    rpc.send_and_confirm_transaction(&tx).await
}

pub async fn send_transactions_parallel(
    rpc: &SolanaRpcClient,
    keypair: &Arc<Keypair>,
    instruction_batches: Vec<Vec<Instruction>>,
) -> eyre::Result<Vec<Signature>> {
    let blockhash = rpc.get_latest_blockhash().await?;

    let mut handles = Vec::with_capacity(instruction_batches.len());
    for batch in instruction_batches {
        let rpc = rpc.clone();
        let kp = Arc::clone(keypair);
        let handle = tokio::spawn(async move {
            let tx = Transaction::new_signed_with_payer(
                &batch,
                Some(&kp.pubkey()),
                &[&kp],
                blockhash,
            );
            rpc.send_and_confirm_transaction(&tx).await
        });
        handles.push(handle);
    }

    let mut signatures = Vec::with_capacity(handles.len());
    for handle in handles {
        signatures.push(handle.await??);
    }
    Ok(signatures)
}

pub async fn send_chunked_packet(
    rpc: &SolanaRpcClient,
    keypair: &Arc<Keypair>,
    chunk_instructions: Vec<Vec<Instruction>>,
    packet_instructions: Vec<Instruction>,
) -> eyre::Result<Signature> {
    if !chunk_instructions.is_empty() {
        send_transactions_parallel(rpc, keypair, chunk_instructions).await?;
    }
    send_transaction(rpc, keypair, packet_instructions).await
}
