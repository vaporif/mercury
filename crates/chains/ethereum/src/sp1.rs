use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::U256;
use alloy::sol_types::{SolCall, SolValue};
use eyre::Context;
use futures::stream::{self, StreamExt, TryStreamExt};
use ibc_client_tendermint::types::ConsensusState as TendermintConsensusState;
use prost::Message;
use sha2::Digest;
use sp1_ics07_tendermint_prover::programs::{SP1Program, UpdateClientProgram};
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
use ibc_eureka_solidity_types::msgs::{ISP1Msgs::SP1Proof, IUpdateClientMsgs::MsgUpdateClient};
use ibc_eureka_solidity_types::sp1_ics07::sp1_ics07_tendermint;
// Renamed ibc-proto 0.51 to match the prover's expected Header type (Mercury uses 0.52 from git).
use ibc_proto_eureka::ibc::lightclients::tendermint::v1::Header as EurekaHeader;

use crate::chain::EthereumChainInner;
use crate::config::{ProverMode, Sp1ProverConfig, ZkAlgorithm};
use crate::contracts::ICS26Router;
use crate::types::{EvmClientId, EvmMessage};

/// Caches SP1 prover setup (keys, program) so proof generation avoids re-running `setup()`.
pub struct Sp1Instance<C: SP1ProverComponents> {
    prover: Sp1Prover<C>,
    program: UpdateClientProgram,
    pkey: SP1ProvingKey,
    vkey: SP1VerifyingKey,
    zk_algorithm: SupportedZkAlgorithm,
    pub proof_timeout: Duration,
    pub max_concurrent_proofs: usize,
}

impl<C: SP1ProverComponents> Sp1Instance<C> {
    pub fn from_prover(prover: Sp1Prover<C>, config: &Sp1ProverConfig) -> eyre::Result<Self> {
        let elf_path = config.elf_dir.join("sp1-ics07-tendermint-update-client");
        let elf_bytes = std::fs::read(&elf_path)
            .wrap_err_with(|| format!("reading SP1 ELF from {}", elf_path.display()))?;

        let elf_hash: [u8; 32] = sha2::Sha256::digest(&elf_bytes).into();
        tracing::info!(elf_hash = %hex::encode(elf_hash), path = %elf_path.display(), "loaded SP1 ELF binary");

        let program = UpdateClientProgram::new(elf_bytes);
        let (pkey, vkey) = prover.setup(program.elf());
        tracing::info!(vkey = %vkey.bytes32(), "SP1 prover setup complete");

        let zk_algorithm = match config.zk_algorithm {
            ZkAlgorithm::Groth16 => SupportedZkAlgorithm::Groth16,
            ZkAlgorithm::Plonk => SupportedZkAlgorithm::Plonk,
        };

        Ok(Self {
            prover,
            program,
            pkey,
            vkey,
            zk_algorithm,
            proof_timeout: Duration::from_secs(config.proof_timeout_secs),
            max_concurrent_proofs: config.max_concurrent_proofs,
        })
    }

    /// Synchronous — must be called from `spawn_blocking`.
    pub fn generate_update_proof(
        &self,
        client_state: &SolClientState,
        consensus_state: &SolConsensusState,
        header: &EurekaHeader,
        time: u128,
    ) -> SP1ProofWithPublicValues {
        let prover = SP1ICS07TendermintProver {
            prover_client: &self.prover,
            pkey: self.pkey.clone(),
            vkey: self.vkey.clone(),
            proof_type: self.zk_algorithm,
            program: &self.program,
        };
        prover.generate_proof(client_state, consensus_state, header, time)
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

/// `tokio::time::timeout` around `spawn_blocking` cancels the *wait*
/// but does not abort the blocking thread.
async fn generate_proof_with_timeout<C: SP1ProverComponents + 'static>(
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
    .map_err(|_| {
        eyre::eyre!(
            "SP1 proof generation timed out after {}s",
            timeout_duration.as_secs()
        )
    })?
    .wrap_err("sp1 proving task panicked")
}

impl EthereumChainInner {
    pub async fn build_update_client_message_sp1<C: SP1ProverComponents + 'static>(
        &self,
        client_id: &EvmClientId,
        headers: Vec<Vec<u8>>,
        trusted_consensus_state: Option<TendermintConsensusState>,
        sp1: &Arc<Sp1Instance<C>>,
    ) -> mercury_core::error::Result<Vec<EvmMessage>> {
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

        let proofs: Vec<SP1ProofWithPublicValues> = stream::iter(decoded_headers)
            .map(|header| {
                let sp1 = Arc::clone(sp1);
                let cs = client_state.clone();
                let cons = consensus_state.clone();
                async move { generate_proof_with_timeout(&sp1, cs, cons, header, time).await }
            })
            .buffered(sp1.max_concurrent_proofs)
            .try_collect()
            .await?;

        let vkey_hex = sp1.vkey.bytes32();
        let messages = proofs
            .into_iter()
            .map(|proof| {
                let update_msg = MsgUpdateClient {
                    sp1Proof: SP1Proof::new(&vkey_hex, proof.bytes(), proof.public_values.to_vec()),
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
            })
            .collect();

        Ok(messages)
    }
}
