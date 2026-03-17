use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::U256;
use alloy::sol_types::{SolCall, SolValue};
use eyre::Context;
use futures::stream::{self, StreamExt, TryStreamExt};
use ibc_client_tendermint::types::ConsensusState as TendermintConsensusState;
use ibc_core_commitment_types::merkle::MerkleProof;
use prost::Message;
use sha2::Digest;
use sp1_ics07_tendermint_prover::programs::{
    MisbehaviourProgram, SP1Program, UpdateClientAndMembershipProgram, UpdateClientProgram,
};
use sp1_ics07_tendermint_prover::prover::{
    SP1ICS07TendermintProver, Sp1Prover, SupportedZkAlgorithm,
};
use sp1_prover::components::SP1ProverComponents;
use sp1_sdk::{
    HashableKey, ProverClient, SP1ProofWithPublicValues, SP1ProvingKey, SP1VerifyingKey,
};

use ibc_eureka_solidity_types::msgs::IICS07TendermintMsgs::{
    ClientState as SolClientState, ConsensusState as SolConsensusState,
};
use ibc_eureka_solidity_types::msgs::{
    IMembershipMsgs::{KVPair, MembershipProof, SP1MembershipAndUpdateClientProof},
    ISP1Msgs::SP1Proof,
    IUpdateClientMsgs::MsgUpdateClient,
};
use ibc_eureka_solidity_types::sp1_ics07::sp1_ics07_tendermint;
// Renamed ibc-proto 0.51 to match the prover's expected Header type (Mercury uses 0.52 from git).
use ibc_proto_eureka::ibc::lightclients::tendermint::v1::Header as EurekaHeader;
use ibc_proto_eureka::ibc::lightclients::tendermint::v1::Misbehaviour as EurekaMisbehaviour;

use mercury_core::error::ProofError;
use tracing::{debug, info};

use mercury_chain_traits::builders::UpdateClientOutput;
use mercury_core::MembershipProofs;

use crate::chain::EthereumChain;
use crate::config::{ProverMode, Sp1ProverConfig, ZkAlgorithm};
use crate::contracts::ICS26Router;
use crate::types::{EvmClientId, EvmMessage};

/// Caches SP1 prover setup (keys, program) so proof generation avoids re-running `setup()`.
pub struct Sp1Instance<C: SP1ProverComponents> {
    prover: Sp1Prover<C>,
    update_program: UpdateClientProgram,
    update_pkey: SP1ProvingKey,
    update_vkey: SP1VerifyingKey,
    uc_membership_program: UpdateClientAndMembershipProgram,
    uc_membership_pkey: SP1ProvingKey,
    uc_membership_vkey: SP1VerifyingKey,
    misbehaviour_program: MisbehaviourProgram,
    misbehaviour_pkey: SP1ProvingKey,
    pub misbehaviour_vkey: SP1VerifyingKey,
    zk_algorithm: SupportedZkAlgorithm,
    pub proof_timeout: Duration,
    pub max_concurrent_proofs: usize,
}

impl<C: SP1ProverComponents> Sp1Instance<C> {
    #[allow(clippy::similar_names)]
    pub fn from_prover(prover: Sp1Prover<C>, config: &Sp1ProverConfig) -> eyre::Result<Self> {
        let uc_elf_path = config.elf_dir.join("sp1-ics07-tendermint-update-client");
        let uc_elf_bytes = std::fs::read(&uc_elf_path)
            .wrap_err_with(|| format!("reading SP1 ELF from {}", uc_elf_path.display()))?;

        let uc_elf_hash: [u8; 32] = sha2::Sha256::digest(&uc_elf_bytes).into();
        tracing::info!(elf_hash = %hex::encode(uc_elf_hash), path = %uc_elf_path.display(), "loaded update-client SP1 ELF");

        let update_program = UpdateClientProgram::new(uc_elf_bytes);
        let (update_pkey, update_vkey) = prover.setup(update_program.elf());
        tracing::info!(vkey = %update_vkey.bytes32(), "update-client SP1 prover setup complete");

        let ucm_elf_path = config
            .elf_dir
            .join("sp1-ics07-tendermint-uc-and-membership");
        let ucm_elf_bytes = std::fs::read(&ucm_elf_path)
            .wrap_err_with(|| format!("reading SP1 ELF from {}", ucm_elf_path.display()))?;

        let ucm_elf_hash: [u8; 32] = sha2::Sha256::digest(&ucm_elf_bytes).into();
        tracing::info!(elf_hash = %hex::encode(ucm_elf_hash), path = %ucm_elf_path.display(), "loaded uc-and-membership SP1 ELF");

        let uc_membership_program = UpdateClientAndMembershipProgram::new(ucm_elf_bytes);
        let (uc_membership_pkey, uc_membership_vkey) = prover.setup(uc_membership_program.elf());
        tracing::info!(vkey = %uc_membership_vkey.bytes32(), "uc-and-membership SP1 prover setup complete");

        let misbehavior_elf_path = config.elf_dir.join("sp1-ics07-tendermint-misbehaviour");
        let misbehavior_elf_bytes = std::fs::read(&misbehavior_elf_path)
            .wrap_err_with(|| format!("reading SP1 ELF from {}", misbehavior_elf_path.display()))?;

        let misbehavior_elf_hash: [u8; 32] = sha2::Sha256::digest(&misbehavior_elf_bytes).into();
        tracing::info!(elf_hash = %hex::encode(misbehavior_elf_hash), path = %misbehavior_elf_path.display(), "loaded misbehaviour SP1 ELF");

        let misbehaviour_program = MisbehaviourProgram::new(misbehavior_elf_bytes);
        let (misbehaviour_pkey, misbehaviour_vkey) = prover.setup(misbehaviour_program.elf());
        tracing::info!(vkey = %misbehaviour_vkey.bytes32(), "misbehaviour SP1 prover setup complete");

        let zk_algorithm = match config.zk_algorithm {
            ZkAlgorithm::Groth16 => SupportedZkAlgorithm::Groth16,
            ZkAlgorithm::Plonk => SupportedZkAlgorithm::Plonk,
        };

        Ok(Self {
            prover,
            update_program,
            update_pkey,
            update_vkey,
            uc_membership_program,
            uc_membership_pkey,
            uc_membership_vkey,
            misbehaviour_program,
            misbehaviour_pkey,
            misbehaviour_vkey,
            zk_algorithm,
            proof_timeout: Duration::from_secs(config.proof_timeout_secs),
            max_concurrent_proofs: config.max_concurrent_proofs,
        })
    }

    #[must_use]
    pub fn generate_update_proof(
        &self,
        client_state: &SolClientState,
        consensus_state: &SolConsensusState,
        header: &EurekaHeader,
        time: u128,
    ) -> SP1ProofWithPublicValues {
        let prover = SP1ICS07TendermintProver {
            prover_client: &self.prover,
            pkey: self.update_pkey.clone(),
            vkey: self.update_vkey.clone(),
            proof_type: self.zk_algorithm,
            program: &self.update_program,
        };
        prover.generate_proof(client_state, consensus_state, header, time)
    }

    #[must_use]
    pub fn generate_uc_and_membership_proof(
        &self,
        client_state: &SolClientState,
        consensus_state: &SolConsensusState,
        header: &EurekaHeader,
        time: u128,
        kv_proofs: Vec<(KVPair, MerkleProof)>,
    ) -> SP1ProofWithPublicValues {
        let prover = SP1ICS07TendermintProver {
            prover_client: &self.prover,
            pkey: self.uc_membership_pkey.clone(),
            vkey: self.uc_membership_vkey.clone(),
            proof_type: self.zk_algorithm,
            program: &self.uc_membership_program,
        };
        prover.generate_proof(client_state, consensus_state, header, time, kv_proofs)
    }

    #[must_use]
    pub fn generate_misbehaviour_proof(
        &self,
        client_state: &SolClientState,
        misbehaviour: &EurekaMisbehaviour,
        trusted_consensus_state_1: &SolConsensusState,
        trusted_consensus_state_2: &SolConsensusState,
        time: u128,
    ) -> SP1ProofWithPublicValues {
        let prover = SP1ICS07TendermintProver {
            prover_client: &self.prover,
            pkey: self.misbehaviour_pkey.clone(),
            vkey: self.misbehaviour_vkey.clone(),
            proof_type: self.zk_algorithm,
            program: &self.misbehaviour_program,
        };
        prover.generate_proof(
            client_state,
            misbehaviour,
            trusted_consensus_state_1,
            trusted_consensus_state_2,
            time,
        )
    }
}

pub fn create_sp1_instance(
    config: &Sp1ProverConfig,
) -> eyre::Result<Sp1Instance<sp1_prover::components::CpuProverComponents>> {
    let prover = match config.prover_mode {
        ProverMode::Mock => {
            let client = ProverClient::builder().mock().build();
            Sp1Prover::new_public_cluster(client)
        }
        ProverMode::Cpu => {
            let client = ProverClient::builder().cpu().build();
            Sp1Prover::new_public_cluster(client)
        }
        ProverMode::Network => {
            let client = ProverClient::builder().network().build();
            Sp1Prover::new_private_cluster(client)
        }
    };

    Sp1Instance::from_prover(prover, config)
}

fn decode_header_for_prover(header_bytes: &[u8]) -> eyre::Result<EurekaHeader> {
    EurekaHeader::decode(header_bytes).wrap_err("decoding header for SP1 prover")
}

/// TODO: `tokio::time::timeout` around `spawn_blocking` cancels the *wait*
/// but does not abort the blocking thread.
async fn generate_update_proof_with_timeout<C: SP1ProverComponents + 'static>(
    sp1: &Arc<Sp1Instance<C>>,
    client_state: SolClientState,
    consensus_state: SolConsensusState,
    header: EurekaHeader,
    time: u128,
) -> eyre::Result<SP1ProofWithPublicValues> {
    let timeout_duration = sp1.proof_timeout;
    let sp1 = Arc::clone(sp1);

    tokio::time::timeout(
        timeout_duration,
        tokio::task::spawn_blocking(move || {
            sp1.generate_update_proof(&client_state, &consensus_state, &header, time)
        }),
    )
    .await
    .map_err(|_| ProofError::ZkProvingFailed {
        reason: format!("timed out after {}s", timeout_duration.as_secs()),
    })?
    .map_err(|e| {
        eyre::Report::from(ProofError::ZkProvingFailed {
            reason: format!("proving task panicked: {e}"),
        })
    })
}

async fn generate_uc_and_membership_proof_with_timeout<C: SP1ProverComponents + 'static>(
    sp1: &Arc<Sp1Instance<C>>,
    client_state: SolClientState,
    consensus_state: SolConsensusState,
    header: EurekaHeader,
    time: u128,
    kv_proofs: Vec<(KVPair, MerkleProof)>,
) -> eyre::Result<SP1ProofWithPublicValues> {
    let timeout_duration = sp1.proof_timeout;
    let sp1 = Arc::clone(sp1);

    tokio::time::timeout(
        timeout_duration,
        tokio::task::spawn_blocking(move || {
            sp1.generate_uc_and_membership_proof(
                &client_state,
                &consensus_state,
                &header,
                time,
                kv_proofs,
            )
        }),
    )
    .await
    .map_err(|_| ProofError::ZkProvingFailed {
        reason: format!(
            "combined proof timed out after {}s",
            timeout_duration.as_secs()
        ),
    })?
    .map_err(|e| {
        eyre::Report::from(ProofError::ZkProvingFailed {
            reason: format!("combined proving task panicked: {e}"),
        })
    })
}

pub async fn generate_misbehaviour_proof_with_timeout<C: SP1ProverComponents + 'static>(
    sp1: &Arc<Sp1Instance<C>>,
    client_state: SolClientState,
    misbehaviour: EurekaMisbehaviour,
    trusted_consensus_state_1: SolConsensusState,
    trusted_consensus_state_2: SolConsensusState,
    time: u128,
) -> eyre::Result<SP1ProofWithPublicValues> {
    let timeout_duration = sp1.proof_timeout;
    let sp1 = Arc::clone(sp1);

    tokio::time::timeout(
        timeout_duration,
        tokio::task::spawn_blocking(move || {
            sp1.generate_misbehaviour_proof(
                &client_state,
                &misbehaviour,
                &trusted_consensus_state_1,
                &trusted_consensus_state_2,
                time,
            )
        }),
    )
    .await
    .map_err(|_| ProofError::ZkProvingFailed {
        reason: format!(
            "misbehaviour proof timed out after {}s",
            timeout_duration.as_secs()
        ),
    })?
    .map_err(|e| {
        eyre::Report::from(ProofError::ZkProvingFailed {
            reason: format!("misbehaviour proving task panicked: {e}"),
        })
    })
}

/// Convert membership proof entries into typed `(KVPair, MerkleProof)` pairs
/// for the SP1 prover.
fn convert_membership_proofs(
    proofs: &MembershipProofs,
) -> eyre::Result<Vec<(KVPair, MerkleProof)>> {
    proofs
        .0
        .iter()
        .map(|entry| {
            let kv_pair = KVPair {
                path: entry.path.iter().cloned().map(Into::into).collect(),
                value: entry.value.clone().into(),
            };
            let raw_proof = ibc_proto_eureka::ibc::core::commitment::v1::MerkleProof::decode(
                entry.proof.as_slice(),
            )
            .wrap_err("decoding membership merkle proof")?;
            let merkle_proof = MerkleProof::try_from(raw_proof)
                .map_err(|e| eyre::eyre!("converting merkle proof: {e}"))?;
            Ok((kv_pair, merkle_proof))
        })
        .collect()
}

impl EthereumChain {
    fn proof_to_update_message(
        &self,
        client_id: &EvmClientId,
        vkey_hex: &str,
        proof: &SP1ProofWithPublicValues,
    ) -> EvmMessage {
        let update_msg = MsgUpdateClient {
            sp1Proof: SP1Proof::new(vkey_hex, proof.bytes(), proof.public_values.to_vec()),
        };
        let call = ICS26Router::updateClientCall {
            clientId: client_id.0.clone(),
            updateMsg: update_msg.abi_encode().into(),
        };
        EvmMessage {
            to: self.router_address,
            calldata: call.abi_encode(),
            value: U256::ZERO,
        }
    }

    async fn generate_update_proofs<C: SP1ProverComponents + 'static>(
        sp1: &Arc<Sp1Instance<C>>,
        headers: Vec<EurekaHeader>,
        client_state: &SolClientState,
        consensus_state: &SolConsensusState,
        time: u128,
    ) -> eyre::Result<Vec<SP1ProofWithPublicValues>> {
        stream::iter(headers)
            .map(|header| {
                let sp1 = Arc::clone(sp1);
                let cs = client_state.clone();
                let cons = consensus_state.clone();
                async move {
                    generate_update_proof_with_timeout(&sp1, cs, cons, header, time).await
                }
            })
            .buffered(sp1.max_concurrent_proofs)
            .try_collect()
            .await
    }

    #[allow(clippy::too_many_lines)]
    pub async fn build_update_client_message_sp1<C: SP1ProverComponents + 'static>(
        &self,
        client_id: &EvmClientId,
        headers: Vec<Vec<u8>>,
        trusted_consensus_state: Option<TendermintConsensusState>,
        membership_proofs: MembershipProofs,
        sp1: &Arc<Sp1Instance<C>>,
    ) -> mercury_core::error::Result<UpdateClientOutput<EvmMessage>> {
        let trusted_cs = trusted_consensus_state.ok_or_else(|| {
            eyre::eyre!(
                "SP1 proof generation requires trusted_consensus_state in payload, \
                 but it was None — is the source chain configured correctly?"
            )
        })?;

        let light_client_addr = self.config.light_client_address()?;
        let sp1_contract = sp1_ics07_tendermint::new(light_client_addr, &*self.provider);
        let client_state: SolClientState = sp1_contract.clientState().call().await?.into();

        let consensus_state = SolConsensusState::from(trusted_cs);

        let decoded_headers: Vec<EurekaHeader> = headers
            .into_iter()
            .map(|bytes| {
                let any = ibc_proto::google::protobuf::Any::decode(bytes.as_slice())
                    .wrap_err("decoding header Any")?;
                decode_header_for_prover(&any.value)
            })
            .collect::<eyre::Result<Vec<_>>>()?;

        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .wrap_err("system time before UNIX epoch")?
            .as_nanos();

        if membership_proofs.is_empty() {
            info!(
                header_count = decoded_headers.len(),
                "starting SP1 update-only proof generation"
            );
            let proofs = Self::generate_update_proofs(
                sp1,
                decoded_headers,
                &client_state,
                &consensus_state,
                time,
            )
            .await?;
            info!(
                proof_count = proofs.len(),
                "SP1 update-only proofs complete"
            );

            let vkey_hex = sp1.update_vkey.bytes32();
            let messages: Vec<_> = proofs
                .into_iter()
                .map(|proof| self.proof_to_update_message(client_id, &vkey_hex, &proof))
                .collect();
            info!(
                message_count = messages.len(),
                "update client messages built"
            );

            Ok(UpdateClientOutput::messages_only(messages))
        } else {
            let kv_proofs = convert_membership_proofs(&membership_proofs)?;

            let mut headers_iter = decoded_headers.into_iter();
            let first_header = headers_iter
                .next()
                .ok_or_else(|| eyre::eyre!("no headers provided for combined proof"))?;

            info!(
                membership_proof_count = kv_proofs.len(),
                "starting SP1 combined (update+membership) proof generation"
            );
            let combined_proof = generate_uc_and_membership_proof_with_timeout(
                sp1,
                client_state.clone(),
                consensus_state.clone(),
                first_header,
                time,
                kv_proofs,
            )
            .await?;
            info!("SP1 combined proof generation complete");

            let sp1_membership_proof = SP1MembershipAndUpdateClientProof {
                sp1Proof: SP1Proof::new(
                    &sp1.uc_membership_vkey.bytes32(),
                    combined_proof.bytes(),
                    combined_proof.public_values.to_vec(),
                ),
            };
            let membership_proof_bytes = MembershipProof::from(sp1_membership_proof).abi_encode();

            let remaining_headers: Vec<EurekaHeader> = headers_iter.collect();
            let messages = if remaining_headers.is_empty() {
                debug!("no remaining headers after combined proof");
                Vec::new()
            } else {
                info!(
                    remaining_count = remaining_headers.len(),
                    "generating update proofs for remaining headers"
                );
                let proofs = Self::generate_update_proofs(
                    sp1,
                    remaining_headers,
                    &client_state,
                    &consensus_state,
                    time,
                )
                .await?;
                info!(
                    proof_count = proofs.len(),
                    "remaining header proofs complete"
                );

                let vkey_hex = sp1.update_vkey.bytes32();
                proofs
                    .into_iter()
                    .map(|proof| self.proof_to_update_message(client_id, &vkey_hex, &proof))
                    .collect()
            };

            info!(
                update_messages = messages.len(),
                has_membership_proof = true,
                "combined SP1 output ready"
            );
            Ok(UpdateClientOutput {
                messages,
                membership_proof: Some(membership_proof_bytes),
            })
        }
    }
}
