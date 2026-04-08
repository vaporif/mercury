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

        let log_path = ledger_dir.path().join("validator.log");
        let stderr_path = ledger_dir.path().join("validator.stderr");
        let log_file =
            std::fs::File::create(&log_path).wrap_err("failed to create validator log file")?;
        let stderr_file =
            std::fs::File::create(&stderr_path).wrap_err("failed to create stderr log file")?;
        let process = cmd
            .stdout(std::process::Stdio::from(log_file))
            .stderr(std::process::Stdio::from(stderr_file))
            .spawn()
            .wrap_err("failed to start solana-test-validator")?;

        let rpc_url = "http://127.0.0.1:8899".to_string();

        let program_ids = SolanaProgramIds {
            ics26: ics26_id,
            ics07: ics07_id,
            access_manager: am_id,
            ibc_app: app_id,
        };

        let mut bootstrap = Self {
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

    fn wait_for_ready(&mut self) -> Result<()> {
        let client = RpcClient::new_with_timeout(self.rpc_url.clone(), Duration::from_secs(5));
        let ledger = self._ledger_dir.path();
        let read_file = |name: &str| std::fs::read_to_string(ledger.join(name)).unwrap_or_default();
        for i in 0..120 {
            if let Some(status) = self.validator_process.try_wait()? {
                use std::os::unix::process::ExitStatusExt;
                let signal = status.signal();
                eyre::bail!(
                    "solana-test-validator exited with {status} (code={:?}, signal={:?})\n\
                     --- stderr ---\n{}\n\
                     --- validator log (last 50 lines) ---\n{}",
                    status.code(),
                    signal,
                    read_file("validator.stderr"),
                    read_file("validator.log")
                        .lines()
                        .rev()
                        .take(50)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            if client.get_health().is_ok() {
                tracing::info!(attempts = i + 1, "solana-test-validator is ready");
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        eyre::bail!(
            "solana-test-validator did not become ready in 60s\n\
             --- stderr ---\n{}\n\
             --- validator log (last 50 lines) ---\n{}",
            read_file("validator.stderr"),
            read_file("validator.log")
                .lines()
                .rev()
                .take(50)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        )
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

    fn grant_role(&self, role_id: u64, account: Pubkey) -> Result<()> {
        #[derive(BorshSerialize)]
        struct GrantRoleArgs {
            role_id: u64,
            account: Pubkey,
        }

        let (am_pda, _) = AccessManager::pda(&self.program_ids.access_manager);
        let grant_data =
            accounts::encode_anchor_instruction("grant_role", &GrantRoleArgs { role_id, account })?;
        self.send_instruction(Instruction {
            program_id: self.program_ids.access_manager,
            accounts: vec![
                AccountMeta::new(am_pda, false),
                AccountMeta::new(self.keypair.pubkey(), true),
                AccountMeta::new_readonly(sysvar::instructions::ID, false),
            ],
            data: grant_data,
        })
    }

    fn initialize_programs(&self) -> Result<()> {
        const RELAYER_ROLE: u64 = 1;
        const ID_CUSTOMIZER_ROLE: u64 = 6;

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

        // Grant roles to test keypair
        for role_id in [RELAYER_ROLE, ID_CUSTOMIZER_ROLE] {
            self.grant_role(role_id, payer)?;
        }

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
