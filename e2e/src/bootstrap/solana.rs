use std::path::Path;
use std::process::{Child, Command};
use std::time::Duration;

use borsh::BorshSerialize;
use eyre::{Result, WrapErr};
use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::sysvar;
use solana_sdk::transaction::Transaction;

use mercury_solana::accounts::{self, AccessManager, IbcApp, Ics26Router};
use mercury_solana::instructions;

pub struct SolanaProgramIds {
    pub ics26: Pubkey,
    pub ics07: Pubkey,
    pub access_manager: Pubkey,
    pub ibc_app: Pubkey,
}

pub struct SolanaBootstrap {
    pub rpc_url: String,
    pub keypair: Keypair,
    pub program_ids: SolanaProgramIds,
    validator_process: Child,
    _ledger_dir: tempfile::TempDir,
}

impl Drop for SolanaBootstrap {
    fn drop(&mut self) {
        let _ = self.validator_process.kill();
        let _ = self.validator_process.wait();
    }
}

impl SolanaBootstrap {
    pub fn start(fixtures_dir: &Path) -> Result<Self> {
        let ledger_dir = tempfile::tempdir().wrap_err("failed to create ledger dir")?;

        let ics26_id = Self::read_program_id(&fixtures_dir.join("ics26_router-keypair.json"))?;
        let ics07_id = Self::read_program_id(&fixtures_dir.join("ics07_tendermint-keypair.json"))?;
        let am_id = Self::read_program_id(&fixtures_dir.join("access_manager-keypair.json"))?;
        let app_id = Self::read_program_id(&fixtures_dir.join("test_ibc_app-keypair.json"))?;

        let mut cmd = Command::new("solana-test-validator");
        cmd.arg("--reset")
            .arg("--quiet")
            .arg("--ledger")
            .arg(ledger_dir.path())
            .args(["--bpf-program", &ics26_id.to_string()])
            .arg(fixtures_dir.join("ics26_router.so"))
            .args(["--bpf-program", &ics07_id.to_string()])
            .arg(fixtures_dir.join("ics07_tendermint.so"))
            .args(["--bpf-program", &am_id.to_string()])
            .arg(fixtures_dir.join("access_manager.so"))
            .args(["--bpf-program", &app_id.to_string()])
            .arg(fixtures_dir.join("test_ibc_app.so"));

        let process = cmd
            .spawn()
            .wrap_err("failed to start solana-test-validator")?;

        let rpc_url = "http://127.0.0.1:8899".to_string();

        let program_ids = SolanaProgramIds {
            ics26: ics26_id,
            ics07: ics07_id,
            access_manager: am_id,
            ibc_app: app_id,
        };

        let bootstrap = Self {
            rpc_url,
            keypair: Keypair::new(),
            program_ids,
            validator_process: process,
            _ledger_dir: ledger_dir,
        };

        bootstrap.wait_for_ready()?;
        bootstrap.airdrop()?;
        bootstrap.initialize_programs()?;

        Ok(bootstrap)
    }

    fn read_program_id(keypair_path: &Path) -> Result<Pubkey> {
        let bytes = std::fs::read(keypair_path)
            .wrap_err_with(|| format!("failed to read keypair: {}", keypair_path.display()))?;
        let raw: Vec<u8> = serde_json::from_slice(&bytes)?;
        let kp =
            Keypair::try_from(raw.as_slice()).map_err(|e| eyre::eyre!("invalid keypair: {e}"))?;
        Ok(kp.pubkey())
    }

    fn wait_for_ready(&self) -> Result<()> {
        let client = RpcClient::new_with_timeout(self.rpc_url.clone(), Duration::from_secs(5));
        for i in 0..30 {
            if client.get_health().is_ok() {
                tracing::info!(attempts = i + 1, "solana-test-validator is ready");
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        eyre::bail!("solana-test-validator did not become ready in 15s")
    }

    fn airdrop(&self) -> Result<()> {
        let client = RpcClient::new_with_timeout(self.rpc_url.clone(), Duration::from_secs(10));
        let amount = 100 * solana_sdk::native_token::LAMPORTS_PER_SOL;
        tracing::debug!(amount, pubkey = %self.keypair.pubkey(), "requesting airdrop");
        let sig = client.request_airdrop(&self.keypair.pubkey(), amount)?;
        let now = std::time::Instant::now();
        while now.elapsed() < Duration::from_secs(15) {
            if client.confirm_transaction(&sig).unwrap_or(false) {
                tracing::info!(%sig, elapsed = ?now.elapsed(), "airdrop confirmed");
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        eyre::bail!("airdrop not confirmed in 15s")
    }

    fn rpc_client(&self) -> RpcClient {
        RpcClient::new_with_commitment(self.rpc_url.clone(), CommitmentConfig::confirmed())
    }

    fn send_instruction(&self, ix: Instruction) -> Result<()> {
        let client = self.rpc_client();
        let blockhash = client.get_latest_blockhash()?;
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&self.keypair.pubkey()),
            &[&self.keypair],
            blockhash,
        );
        client
            .send_and_confirm_transaction(&tx)
            .wrap_err("transaction failed")?;
        Ok(())
    }

    fn initialize_programs(&self) -> Result<()> {
        const ID_CUSTOMIZER_ROLE: u64 = 6;

        #[derive(BorshSerialize)]
        struct GrantRoleArgs {
            role_id: u64,
            account: Pubkey,
        }

        let payer = self.keypair.pubkey();
        let ids = &self.program_ids;

        // Initialize access manager
        let (am_pda, _) = AccessManager::pda(&ids.access_manager);
        let am_init_data = accounts::encode_anchor_instruction("initialize", &payer)?;
        self.send_instruction(Instruction {
            program_id: ids.access_manager,
            accounts: vec![
                AccountMeta::new(am_pda, false),
                AccountMeta::new(payer, true),
                AccountMeta::new_readonly(solana_system_interface::program::ID, false),
                AccountMeta::new_readonly(sysvar::instructions::ID, false),
            ],
            data: am_init_data,
        })?;

        // Initialize ICS26 router
        let (router_state, _) = Ics26Router::router_state_pda(&ids.ics26);
        let router_init_data =
            accounts::encode_anchor_instruction("initialize", &ids.access_manager)?;
        self.send_instruction(Instruction {
            program_id: ids.ics26,
            accounts: vec![
                AccountMeta::new(router_state, false),
                AccountMeta::new(payer, true),
                AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            ],
            data: router_init_data,
        })?;

        // Grant ID_CUSTOMIZER_ROLE to test keypair
        let grant_data = accounts::encode_anchor_instruction(
            "grant_role",
            &GrantRoleArgs {
                role_id: ID_CUSTOMIZER_ROLE,
                account: payer,
            },
        )?;
        self.send_instruction(Instruction {
            program_id: ids.access_manager,
            accounts: vec![
                AccountMeta::new(am_pda, false),
                AccountMeta::new(payer, true),
                AccountMeta::new_readonly(sysvar::instructions::ID, false),
            ],
            data: grant_data,
        })?;

        // Initialize IBC app
        let (app_state, _) = IbcApp::state_pda(&ids.ibc_app);
        let app_init_data = accounts::encode_anchor_instruction("initialize", &payer)?;
        self.send_instruction(Instruction {
            program_id: ids.ibc_app,
            accounts: vec![
                AccountMeta::new(app_state, false),
                AccountMeta::new(payer, true),
                AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            ],
            data: app_init_data,
        })?;

        // Register IBC app on ICS26
        let add_app_ix = instructions::client::add_ibc_app(
            &ids.ics26,
            &payer,
            &payer,
            "transfer",
            &ids.ibc_app,
            &ids.access_manager,
        )?;
        self.send_instruction(add_app_ix)?;

        tracing::info!("solana programs initialized and IBC app registered");
        Ok(())
    }
}
