use borsh::{BorshDeserialize, BorshSerialize};
use solana_sdk::pubkey::Pubkey;

use crate::rpc::SolanaRpcClient;

pub const ANCHOR_DISCRIMINATOR_LEN: usize = 8;

#[must_use]
pub fn anchor_discriminator(prefix: &str, name: &str) -> [u8; 8] {
    let hash = solana_sdk::hash::hash(format!("{prefix}:{name}").as_bytes());
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&hash.to_bytes()[..8]);
    disc
}

#[must_use]
pub fn anchor_account_discriminator(struct_name: &str) -> [u8; 8] {
    anchor_discriminator("account", struct_name)
}

#[must_use]
pub fn anchor_instruction_discriminator(method_name: &str) -> [u8; 8] {
    anchor_discriminator("global", method_name)
}

pub fn encode_anchor_instruction(method: &str, args: &impl BorshSerialize) -> Vec<u8> {
    let disc = anchor_instruction_discriminator(method);
    let mut data = disc.to_vec();
    args.serialize(&mut data).expect("borsh serialize");
    data
}

pub struct Ics26Router;

impl Ics26Router {
    pub const ROUTER_STATE_SEED: &[u8] = b"router_state";
    pub const CLIENT_SEED: &[u8] = b"client";
    pub const CLIENT_SEQ_SEED: &[u8] = b"cseq";
    pub const IBC_APP_SEED: &[u8] = b"ibc_app";
    pub const PACKET_COMMITMENT_SEED: &[u8] = b"packet_commitment";
    pub const PACKET_RECEIPT_SEED: &[u8] = b"packet_receipt";
    pub const PACKET_ACK_SEED: &[u8] = b"packet_ack";
    pub const PAYLOAD_CHUNK_SEED: &[u8] = b"payload_chunk";
    pub const PROOF_CHUNK_SEED: &[u8] = b"proof_chunk";

    #[must_use]
    pub fn router_state_pda(program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[Self::ROUTER_STATE_SEED], program_id)
    }

    #[must_use]
    pub fn client_pda(client_id: &str, program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[Self::CLIENT_SEED, client_id.as_bytes()], program_id)
    }

    #[must_use]
    pub fn client_sequence_pda(client_id: &str, program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[Self::CLIENT_SEQ_SEED, client_id.as_bytes()], program_id)
    }

    #[must_use]
    pub fn ibc_app_pda(port: &str, program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[Self::IBC_APP_SEED, port.as_bytes()], program_id)
    }

    #[must_use]
    pub fn packet_commitment_pda(
        client_id: &str,
        sequence: u64,
        program_id: &Pubkey,
    ) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                Self::PACKET_COMMITMENT_SEED,
                client_id.as_bytes(),
                &sequence.to_le_bytes(),
            ],
            program_id,
        )
    }

    #[must_use]
    pub fn packet_receipt_pda(client_id: &str, sequence: u64, program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                Self::PACKET_RECEIPT_SEED,
                client_id.as_bytes(),
                &sequence.to_le_bytes(),
            ],
            program_id,
        )
    }

    #[must_use]
    pub fn packet_ack_pda(client_id: &str, sequence: u64, program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                Self::PACKET_ACK_SEED,
                client_id.as_bytes(),
                &sequence.to_le_bytes(),
            ],
            program_id,
        )
    }

    #[must_use]
    pub fn payload_chunk_pda(
        payer: &Pubkey,
        client_id: &str,
        sequence: u64,
        payload_index: u8,
        chunk_index: u8,
        program_id: &Pubkey,
    ) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                Self::PAYLOAD_CHUNK_SEED,
                payer.as_ref(),
                client_id.as_bytes(),
                &sequence.to_le_bytes(),
                &[payload_index],
                &[chunk_index],
            ],
            program_id,
        )
    }

    #[must_use]
    pub fn proof_chunk_pda(
        payer: &Pubkey,
        client_id: &str,
        sequence: u64,
        chunk_index: u8,
        program_id: &Pubkey,
    ) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                Self::PROOF_CHUNK_SEED,
                payer.as_ref(),
                client_id.as_bytes(),
                &sequence.to_le_bytes(),
                &[chunk_index],
            ],
            program_id,
        )
    }
}

pub struct Ics07Tendermint;

impl Ics07Tendermint {
    pub const CLIENT_STATE_SEED: &[u8] = b"client";
    pub const CONSENSUS_STATE_SEED: &[u8] = b"consensus_state";
    pub const APP_STATE_SEED: &[u8] = b"app_state";
    pub const HEADER_CHUNK_SEED: &[u8] = b"header_chunk";
    pub const SIG_VERIFY_SEED: &[u8] = b"sig_verify";

    #[must_use]
    pub fn client_state_pda(program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[Self::CLIENT_STATE_SEED], program_id)
    }

    #[must_use]
    pub fn consensus_state_pda(height: u64, program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[Self::CONSENSUS_STATE_SEED, &height.to_le_bytes()],
            program_id,
        )
    }

    #[must_use]
    pub fn app_state_pda(program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[Self::APP_STATE_SEED], program_id)
    }

    #[must_use]
    pub fn header_chunk_pda(
        submitter: &Pubkey,
        height: u64,
        chunk_index: u8,
        program_id: &Pubkey,
    ) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                Self::HEADER_CHUNK_SEED,
                submitter.as_ref(),
                &height.to_le_bytes(),
                &[chunk_index],
            ],
            program_id,
        )
    }

    #[must_use]
    pub fn sig_verify_pda(signature_hash: &[u8], program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[Self::SIG_VERIFY_SEED, signature_hash], program_id)
    }
}

pub struct IbcApp;

impl IbcApp {
    pub const STATE_SEED: &[u8] = b"app_state";

    #[must_use]
    pub fn state_pda(program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[Self::STATE_SEED], program_id)
    }
}

pub struct AccessManager;

impl AccessManager {
    pub const SEED: &[u8] = b"access_manager";

    #[must_use]
    pub fn pda(program_id: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[Self::SEED], program_id)
    }
}

#[derive(BorshDeserialize, Debug, Clone)]
pub struct IbcHeight {
    pub revision_number: u64,
    pub revision_height: u64,
}

#[derive(BorshDeserialize, Debug, Clone)]
pub struct OnChainClientState {
    pub chain_id: String,
    pub trust_level_numerator: u64,
    pub trust_level_denominator: u64,
    pub trusting_period: u64,
    pub unbonding_period: u64,
    pub max_clock_drift: u64,
    pub frozen_height: IbcHeight,
    pub latest_height: IbcHeight,
}

#[derive(BorshDeserialize, Debug, Clone)]
pub struct OnChainConsensusState {
    pub timestamp: u64,
    pub root: [u8; 32],
    pub next_validators_hash: [u8; 32],
}

#[derive(BorshDeserialize, Debug, Clone)]
pub struct OnChainRouterState {
    pub version: u8,
    pub access_manager: Pubkey,
    pub paused: bool,
}

#[derive(BorshDeserialize, Debug, Clone)]
pub struct OnChainClient {
    pub version: u8,
    pub client_id: String,
    pub client_program_id: Pubkey,
}

#[derive(BorshDeserialize, Debug, Clone)]
pub struct OnChainClientSequence {
    pub version: u8,
    pub next_sequence_send: u64,
}

#[derive(BorshDeserialize, Debug, Clone)]
pub struct OnChainCommitment {
    pub value: [u8; 32],
}

pub fn deserialize_anchor_account<T: BorshDeserialize>(data: &[u8]) -> eyre::Result<T> {
    if data.len() < ANCHOR_DISCRIMINATOR_LEN {
        return Err(eyre::eyre!(
            "account data too short ({} bytes) to contain anchor discriminator",
            data.len()
        ));
    }
    let mut slice = &data[ANCHOR_DISCRIMINATOR_LEN..];
    T::deserialize(&mut slice).map_err(|e| eyre::eyre!("borsh deserialization failed: {e}"))
}

pub async fn fetch_account<T: BorshDeserialize>(
    rpc: &SolanaRpcClient,
    address: &Pubkey,
) -> eyre::Result<Option<T>> {
    rpc.get_account(address)
        .await?
        .map(|acc| deserialize_anchor_account::<T>(&acc.data))
        .transpose()
}

pub async fn resolve_ics07_program_id(
    rpc: &SolanaRpcClient,
    client_id: &str,
    ics26_program_id: &Pubkey,
) -> eyre::Result<Pubkey> {
    let (pda, _) = Ics26Router::client_pda(client_id, ics26_program_id);
    let client: OnChainClient = fetch_account(rpc, &pda)
        .await?
        .ok_or_else(|| eyre::eyre!("client PDA not found for client_id={client_id}"))?;
    Ok(client.client_program_id)
}

pub async fn resolve_app_program_id(
    rpc: &SolanaRpcClient,
    port: &str,
    ics26_program_id: &Pubkey,
) -> eyre::Result<Pubkey> {
    #[derive(BorshDeserialize)]
    struct IbcAppAccount {
        _version: u8,
        _port_id: String,
        app_program_id: Pubkey,
    }

    let (pda, _) = Ics26Router::ibc_app_pda(port, ics26_program_id);
    let app: IbcAppAccount = fetch_account(rpc, &pda)
        .await?
        .ok_or_else(|| eyre::eyre!("IbcApp PDA not found for port={port}"))?;
    Ok(app.app_program_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_state_pda_is_deterministic() {
        let program = Pubkey::new_unique();
        let (a, bump_a) = Ics26Router::router_state_pda(&program);
        let (b, bump_b) = Ics26Router::router_state_pda(&program);
        assert_eq!(a, b);
        assert_eq!(bump_a, bump_b);
    }

    #[test]
    fn packet_commitment_pda_varies_by_sequence() {
        let program = Pubkey::new_unique();
        let (a, _) = Ics26Router::packet_commitment_pda("07-tendermint-0", 1, &program);
        let (b, _) = Ics26Router::packet_commitment_pda("07-tendermint-0", 2, &program);
        assert_ne!(a, b);
    }

    #[test]
    fn anchor_discriminator_is_stable() {
        let disc = anchor_account_discriminator("RouterState");
        assert_eq!(disc.len(), 8);
        assert_eq!(disc, anchor_account_discriminator("RouterState"));
    }

    #[test]
    fn deserialize_anchor_account_rejects_short_data() {
        let short = vec![0u8; 4];
        let result = deserialize_anchor_account::<OnChainCommitment>(&short);
        assert!(result.is_err());
    }
}
