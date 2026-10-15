use borsh::{BorshDeserialize, BorshSerialize};
use solana_sdk::pubkey::Pubkey;

use crate::rpc::SolanaRpcClient;

pub const ANCHOR_DISCRIMINATOR_LEN: usize = 8;

#[must_use]
pub fn anchor_discriminator(prefix: &str, name: &str) -> [u8; ANCHOR_DISCRIMINATOR_LEN] {
    let hash = solana_sdk::hash::hash(format!("{prefix}:{name}").as_bytes());
    let mut disc = [0u8; ANCHOR_DISCRIMINATOR_LEN];
    disc.copy_from_slice(&hash.to_bytes()[..ANCHOR_DISCRIMINATOR_LEN]);
    disc
}

#[must_use]
pub fn anchor_account_discriminator(struct_name: &str) -> [u8; ANCHOR_DISCRIMINATOR_LEN] {
    anchor_discriminator("account", struct_name)
}

#[must_use]
pub fn anchor_instruction_discriminator(method_name: &str) -> [u8; ANCHOR_DISCRIMINATOR_LEN] {
    anchor_discriminator("global", method_name)
}

pub fn encode_anchor_instruction(
    method: &str,
    args: &impl BorshSerialize,
) -> eyre::Result<Vec<u8>> {
    let disc = anchor_instruction_discriminator(method);
    let mut data = disc.to_vec();
    args.serialize(&mut data)?;
    Ok(data)
}

pub struct Ics26Router;

impl Ics26Router {
    pub const ROUTER_STATE_SEED: &[u8] = b"router_state";
    pub const CLIENT_SEED: &[u8] = b"client";
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
pub struct AccessManagerState {
    pub access_manager: Pubkey,
    pub pending_access_manager: Option<Pubkey>,
}

#[derive(BorshDeserialize, Debug, Clone)]
pub struct OnChainRouterState {
    pub version: u8,
    pub am_state: AccessManagerState,
    pub paused: bool,
}

#[derive(BorshDeserialize, Debug, Clone)]
pub struct OnChainClient {
    pub version: u8,
    pub client_id: String,
    pub client_program_id: Pubkey,
}

#[derive(BorshDeserialize, Debug, Clone)]
pub struct OnChainCommitment {
    pub value: [u8; 32],
}

impl OnChainCommitment {
    /// The router zeroes a commitment on ack/timeout instead of closing its
    /// PDA, so an existing account does not imply a live commitment.
    #[must_use]
    pub fn is_cleared(&self) -> bool {
        self.value == [0; 32]
    }
}

/// An Anchor `#[account]` type, identified on-chain by the discriminator
/// derived from its Rust struct name.
pub trait AnchorAccount: BorshDeserialize {
    /// The on-chain struct name, which may differ from the local type name.
    const NAME: &'static str;
}

impl AnchorAccount for OnChainClientState {
    const NAME: &'static str = "ClientState";
}

impl AnchorAccount for OnChainRouterState {
    const NAME: &'static str = "RouterState";
}

impl AnchorAccount for OnChainClient {
    const NAME: &'static str = "Client";
}

impl AnchorAccount for OnChainCommitment {
    const NAME: &'static str = "Commitment";
}

pub fn deserialize_anchor_account<T: AnchorAccount>(data: &[u8]) -> eyre::Result<T> {
    let Some((disc, mut body)) = data.split_at_checked(ANCHOR_DISCRIMINATOR_LEN) else {
        return Err(eyre::eyre!(
            "account data too short ({} bytes) to contain anchor discriminator",
            data.len()
        ));
    };
    eyre::ensure!(
        disc == anchor_account_discriminator(T::NAME),
        "anchor discriminator mismatch: account is not a {}",
        T::NAME
    );
    T::deserialize(&mut body).map_err(|e| eyre::eyre!("borsh deserialization failed: {e}"))
}

pub async fn fetch_account<T: AnchorAccount>(
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

    impl AnchorAccount for IbcAppAccount {
        const NAME: &'static str = "IBCApp";
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

    use sha2::{Digest, Sha256};

    use crate::rpc::testing::{FakeRpc, anchor_account};

    fn sha256_prefix(preimage: &str) -> [u8; ANCHOR_DISCRIMINATOR_LEN] {
        Sha256::digest(preimage.as_bytes())[..ANCHOR_DISCRIMINATOR_LEN]
            .try_into()
            .unwrap()
    }

    #[test]
    fn discriminators_follow_anchor_namespacing() {
        assert_eq!(
            anchor_instruction_discriminator("recv_packet"),
            sha256_prefix("global:recv_packet")
        );
        assert_eq!(
            anchor_account_discriminator("RouterState"),
            sha256_prefix("account:RouterState")
        );
        assert_ne!(
            anchor_instruction_discriminator("Client"),
            anchor_account_discriminator("Client")
        );
    }

    #[test]
    fn encode_anchor_instruction_prefixes_borsh_args() {
        let args = (7u64, "abc".to_string());
        let data = encode_anchor_instruction("ack_packet", &args).unwrap();
        let (disc, body) = data.split_at(ANCHOR_DISCRIMINATOR_LEN);
        assert_eq!(disc, anchor_instruction_discriminator("ack_packet"));
        assert_eq!(body, borsh::to_vec(&args).unwrap());
    }

    #[test]
    fn pdas_use_on_chain_seed_layout() {
        // Sequence and height seeds are little-endian on-chain.
        let program = Pubkey::new_unique();
        let payer = Pubkey::new_unique();
        let seq = 0x0102_0304_0506_0708_u64;
        let le = seq.to_le_bytes();
        let pda = |seeds: &[&[u8]]| Pubkey::find_program_address(seeds, &program);

        let cases: Vec<((Pubkey, u8), (Pubkey, u8))> = vec![
            (
                Ics26Router::router_state_pda(&program),
                pda(&[b"router_state"]),
            ),
            (
                Ics26Router::client_pda("c-0", &program),
                pda(&[b"client", b"c-0"]),
            ),
            (
                Ics26Router::ibc_app_pda("transfer", &program),
                pda(&[b"ibc_app", b"transfer"]),
            ),
            (
                Ics26Router::packet_commitment_pda("c-0", seq, &program),
                pda(&[b"packet_commitment", b"c-0", &le]),
            ),
            (
                Ics26Router::packet_receipt_pda("c-0", seq, &program),
                pda(&[b"packet_receipt", b"c-0", &le]),
            ),
            (
                Ics26Router::packet_ack_pda("c-0", seq, &program),
                pda(&[b"packet_ack", b"c-0", &le]),
            ),
            (
                Ics26Router::payload_chunk_pda(&payer, "c-0", seq, 2, 3, &program),
                pda(&[b"payload_chunk", payer.as_ref(), b"c-0", &le, &[2], &[3]]),
            ),
            (
                Ics26Router::proof_chunk_pda(&payer, "c-0", seq, 3, &program),
                pda(&[b"proof_chunk", payer.as_ref(), b"c-0", &le, &[3]]),
            ),
            (
                Ics07Tendermint::client_state_pda(&program),
                pda(&[b"client"]),
            ),
            (
                Ics07Tendermint::consensus_state_pda(seq, &program),
                pda(&[b"consensus_state", &le]),
            ),
            (
                Ics07Tendermint::app_state_pda(&program),
                pda(&[b"app_state"]),
            ),
            (
                Ics07Tendermint::header_chunk_pda(&payer, seq, 4, &program),
                pda(&[b"header_chunk", payer.as_ref(), &le, &[4]]),
            ),
            (
                Ics07Tendermint::sig_verify_pda(&[9; 32], &program),
                pda(&[b"sig_verify", &[9; 32]]),
            ),
            (IbcApp::state_pda(&program), pda(&[b"app_state"])),
            (AccessManager::pda(&program), pda(&[b"access_manager"])),
        ];
        for (i, (got, want)) in cases.into_iter().enumerate() {
            assert_eq!(got, want, "case {i}");
        }
    }

    #[test]
    fn packet_pdas_are_namespaced_by_kind_and_client() {
        let program = Pubkey::new_unique();
        let commitment = Ics26Router::packet_commitment_pda("c-0", 1, &program).0;
        let receipt = Ics26Router::packet_receipt_pda("c-0", 1, &program).0;
        let ack = Ics26Router::packet_ack_pda("c-0", 1, &program).0;
        let other_client = Ics26Router::packet_commitment_pda("c-1", 1, &program).0;
        let other_program = Ics26Router::packet_commitment_pda("c-0", 1, &Pubkey::new_unique()).0;
        let all = [commitment, receipt, ack, other_client, other_program];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn chunk_pdas_are_scoped_to_payer() {
        let program = Pubkey::new_unique();
        let (a, b) = (Pubkey::new_unique(), Pubkey::new_unique());
        assert_ne!(
            Ics26Router::payload_chunk_pda(&a, "c", 1, 0, 0, &program),
            Ics26Router::payload_chunk_pda(&b, "c", 1, 0, 0, &program)
        );
        assert_ne!(
            Ics26Router::payload_chunk_pda(&a, "c", 1, 0, 1, &program),
            Ics26Router::payload_chunk_pda(&a, "c", 1, 1, 0, &program)
        );
    }

    type WireHeight = (u64, u64);
    type WireClientState = (String, u64, u64, u64, u64, u64, WireHeight, WireHeight);

    fn wire_client_state() -> WireClientState {
        (
            "cosmoshub-4".into(),
            1,
            3,
            1_000,
            2_000,
            15,
            (0, 0),
            (4, 123),
        )
    }

    #[test]
    fn deserializes_client_state_skipping_discriminator() {
        let data = anchor_account("ClientState", &wire_client_state());
        let cs: OnChainClientState = deserialize_anchor_account(&data).unwrap();
        assert_eq!(cs.chain_id, "cosmoshub-4");
        assert_eq!(
            (cs.trust_level_numerator, cs.trust_level_denominator),
            (1, 3)
        );
        assert_eq!(cs.trusting_period, 1_000);
        assert_eq!(cs.unbonding_period, 2_000);
        assert_eq!(cs.max_clock_drift, 15);
        assert_eq!(cs.frozen_height.revision_height, 0);
        assert_eq!(cs.latest_height.revision_number, 4);
        assert_eq!(cs.latest_height.revision_height, 123);
    }

    #[test]
    fn deserialize_tolerates_trailing_account_padding() {
        let mut data = anchor_account("Commitment", &[5u8; 32]);
        data.extend([0u8; 64]);
        let c: OnChainCommitment = deserialize_anchor_account(&data).unwrap();
        assert_eq!(c.value, [5u8; 32]);
    }

    #[test]
    fn deserialize_rejects_truncated_body() {
        let data = anchor_account("Commitment", &[5u8; 16]);
        let err = deserialize_anchor_account::<OnChainCommitment>(&data).unwrap_err();
        assert!(err.to_string().contains("borsh deserialization failed"));

        let disc_only = vec![0u8; ANCHOR_DISCRIMINATOR_LEN];
        assert!(deserialize_anchor_account::<OnChainCommitment>(&disc_only).is_err());
    }

    #[test]
    fn deserialize_rejects_wrong_discriminator() {
        // Same 32-byte body, but a different account type's discriminator.
        let data = anchor_account("RouterState", &[5u8; 32]);
        let err = deserialize_anchor_account::<OnChainCommitment>(&data).unwrap_err();
        assert!(err.to_string().contains("discriminator mismatch"), "{err}");

        let zeroed = vec![0u8; ANCHOR_DISCRIMINATOR_LEN + 32];
        assert!(deserialize_anchor_account::<OnChainCommitment>(&zeroed).is_err());
    }

    #[test]
    fn deserializes_router_state_with_optional_pending_manager() {
        let am = Pubkey::new_unique();
        let pending = Pubkey::new_unique();
        let with_pending = (1u8, (am.to_bytes(), Some(pending.to_bytes())), true);
        let rs: OnChainRouterState =
            deserialize_anchor_account(&anchor_account("RouterState", &with_pending)).unwrap();
        assert_eq!(rs.am_state.access_manager, am);
        assert_eq!(rs.am_state.pending_access_manager, Some(pending));
        assert!(rs.paused);

        let without = (1u8, (am.to_bytes(), None::<[u8; 32]>), false);
        let rs: OnChainRouterState =
            deserialize_anchor_account(&anchor_account("RouterState", &without)).unwrap();
        assert_eq!(rs.am_state.pending_access_manager, None);
        assert!(!rs.paused);
    }

    fn client_account(ics07: Pubkey) -> Vec<u8> {
        anchor_account("Client", &(1u8, "c-0".to_string(), ics07.to_bytes()))
    }

    #[tokio::test]
    async fn fetch_account_maps_missing_present_and_malformed() {
        let (good, bad) = (Pubkey::new_unique(), Pubkey::new_unique());
        let rpc = FakeRpc::default()
            .with_account(good, anchor_account("Commitment", &[1u8; 32]))
            .with_account(bad, vec![0u8; 3])
            .into_client();

        let got: Option<OnChainCommitment> = fetch_account(&rpc, &good).await.unwrap();
        assert_eq!(got.unwrap().value, [1u8; 32]);
        let missing: Option<OnChainCommitment> =
            fetch_account(&rpc, &Pubkey::new_unique()).await.unwrap();
        assert!(missing.is_none());
        assert!(
            fetch_account::<OnChainCommitment>(&rpc, &bad)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn resolve_ics07_program_id_reads_client_pda() {
        let ics26 = Pubkey::new_unique();
        let ics07 = Pubkey::new_unique();
        let (pda, _) = Ics26Router::client_pda("c-0", &ics26);
        let rpc = FakeRpc::default()
            .with_account(pda, client_account(ics07))
            .into_client();

        assert_eq!(
            resolve_ics07_program_id(&rpc, "c-0", &ics26).await.unwrap(),
            ics07
        );
        let err = resolve_ics07_program_id(&rpc, "c-9", &ics26)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("client_id=c-9"), "{err}");
    }

    #[tokio::test]
    async fn resolve_app_program_id_reads_ibc_app_pda() {
        let ics26 = Pubkey::new_unique();
        let app = Pubkey::new_unique();
        let (pda, _) = Ics26Router::ibc_app_pda("transfer", &ics26);
        let body = (1u8, "transfer".to_string(), app.to_bytes());
        let rpc = FakeRpc::default()
            .with_account(pda, anchor_account("IBCApp", &body))
            .into_client();

        assert_eq!(
            resolve_app_program_id(&rpc, "transfer", &ics26)
                .await
                .unwrap(),
            app
        );
        let err = resolve_app_program_id(&rpc, "other", &ics26)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("port=other"), "{err}");
    }

    fn fuzz_account<T: AnchorAccount>(with_disc: bool, body: &[u8]) {
        let mut data = if with_disc {
            anchor_account_discriminator(T::NAME).to_vec()
        } else {
            Vec::new()
        };
        data.extend_from_slice(body);
        let _ = deserialize_anchor_account::<T>(&data);
    }

    #[test]
    fn fuzz_deserialize_anchor_account() {
        bolero::check!()
            .with_type::<(u8, Vec<u8>)>()
            .for_each(|(selector, body)| {
                let with_disc = selector & 0x80 == 0;
                match selector % 4 {
                    0 => fuzz_account::<OnChainClientState>(with_disc, body),
                    1 => fuzz_account::<OnChainRouterState>(with_disc, body),
                    2 => fuzz_account::<OnChainClient>(with_disc, body),
                    _ => fuzz_account::<OnChainCommitment>(with_disc, body),
                }
            });
    }
}
