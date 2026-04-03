use solana_address_lookup_table_interface as alt_interface;
use solana_message::AddressLookupTableAccount;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;

use crate::rpc::SolanaRpcClient;

#[must_use]
pub fn create_alt(payer: &Pubkey, recent_slot: u64) -> (Instruction, Pubkey) {
    alt_interface::instruction::create_lookup_table(*payer, *payer, recent_slot)
}

#[must_use]
pub fn extend_alt(alt_address: &Pubkey, payer: &Pubkey, accounts: &[Pubkey]) -> Vec<Instruction> {
    const MAX_PER_EXTEND: usize = 20;
    accounts
        .chunks(MAX_PER_EXTEND)
        .map(|chunk| {
            alt_interface::instruction::extend_lookup_table(
                *alt_address,
                *payer,
                Some(*payer),
                chunk.to_vec(),
            )
        })
        .collect()
}

pub async fn lookup_alt(
    rpc: &SolanaRpcClient,
    alt_address: &Pubkey,
) -> eyre::Result<AddressLookupTableAccount> {
    let account = rpc
        .get_account(alt_address)
        .await?
        .ok_or_else(|| eyre::eyre!("ALT account not found: {alt_address}"))?;
    let table = alt_interface::state::AddressLookupTable::deserialize(&account.data)
        .map_err(|e| eyre::eyre!("failed to deserialize ALT: {e}"))?;
    Ok(AddressLookupTableAccount {
        key: *alt_address,
        addresses: table.addresses.to_vec(),
    })
}
