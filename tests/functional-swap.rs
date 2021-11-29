#[macro_use]
extern crate log;

use bitcoincore_rpc::{Auth, Client, RpcApi};
use std::ffi::OsStr;
use std::io;
use std::process;
use std::{thread, time};
use sysinfo::{ProcessExt, System, SystemExt};

use std::collections::HashMap;
use std::str;
use std::str::FromStr;

const ALLOWED_RETRIES: u32 = 120;

#[tokio::test]
#[ignore]
async fn swap_test_bob_maker() {
    let bitcoin_rpc = bitcoin_setup();
    let (monero_regtest, monero_wallet) = monero_setup().await;

    let (farcasterd_maker, data_dir_maker, farcasterd_taker, data_dir_taker) =
        setup_farcaster_clients().await;

    let (_, bitcoin_address, swap_ids) = make_and_take_offer(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        &bitcoin_rpc,
        &monero_wallet,
    )
    .await;

    run_swap(
        swap_ids[0].clone(),
        data_dir_taker,
        data_dir_maker,
        bitcoin_rpc,
        bitcoin_address,
        monero_regtest,
        monero_wallet,
    )
    .await;

    cleanup_processes(farcasterd_maker, farcasterd_taker);
}

#[tokio::test]
#[ignore]
async fn swap_test_alice_maker() {
    let bitcoin_rpc = bitcoin_setup();
    let (monero_regtest, monero_wallet) = monero_setup().await;

    let (farcasterd_maker, data_dir_maker, farcasterd_taker, data_dir_taker) =
        setup_farcaster_clients().await;

    let (_, bitcoin_address, swap_ids) = make_and_take_offer(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Alice".to_string(),
        &bitcoin_rpc,
        &monero_wallet,
    )
    .await;

    run_swap(
        swap_ids[0].clone(),
        data_dir_maker,
        data_dir_taker,
        bitcoin_rpc,
        bitcoin_address,
        monero_regtest,
        monero_wallet,
    )
    .await;

    cleanup_processes(farcasterd_maker, farcasterd_taker);
}

async fn setup_farcaster_clients() -> (process::Child, Vec<String>, process::Child, Vec<String>) {
    // data directories
    let data_dir_maker = vec!["-d".to_string(), "tests/.farcaster_node_0".to_string()];
    let data_dir_taker = vec!["-d".to_string(), "tests/.farcaster_node_1".to_string()];

    let server_args_maker = vec!["-vvv", "--config", "tests/.farcasterd_1.toml"]
        .iter()
        .map(|i| i.to_string())
        .collect();

    let server_args_taker = vec!["-vvv", "--config", "tests/.farcasterd_2.toml"]
        .iter()
        .map(|i| i.to_string())
        .collect();

    let farcasterd_maker_args = farcasterd_args(data_dir_maker.clone(), server_args_maker);
    let farcasterd_taker_args = farcasterd_args(data_dir_taker.clone(), server_args_taker);

    let farcasterd_maker = launch("../farcasterd", farcasterd_maker_args).unwrap();
    let farcasterd_taker = launch("../farcasterd", farcasterd_taker_args).unwrap();
    (
        farcasterd_maker,
        data_dir_maker,
        farcasterd_taker,
        data_dir_taker,
    )
}

async fn make_and_take_offer(
    data_dir_maker: Vec<String>,
    data_dir_taker: Vec<String>,
    role: String,
    bitcoin_rpc: &bitcoincore_rpc::Client,
    monero_wallet: &monero_rpc::WalletClient,
) -> (monero::Address, bitcoin::Address, Vec<String>) {
    let maker_info_args = info_args(data_dir_maker.clone());

    // test connection to farcasterd and check that swap-cli is in the correct place
    run("../swap-cli", maker_info_args.clone()).unwrap();

    let xmr_address = monero_wallet.get_address(0, None).await.unwrap().address;
    let btc_address = bitcoin_rpc.get_new_address(None, None).unwrap();

    let btc_addr = btc_address.to_string();
    let xmr_addr = xmr_address.to_string();

    let cli_make_args = make_offer_args(
        data_dir_maker.clone(),
        role,
        btc_addr.clone(),
        xmr_addr.clone(),
    );
    let (_stdout, _stderr) = run("../swap-cli", cli_make_args).unwrap();

    // get offer strings
    let offers = retry_until_offer(maker_info_args.clone());

    let cli_take_args = take_offer_args(
        data_dir_taker.clone(),
        btc_addr,
        xmr_addr,
        offers[0].clone(),
    );
    run("../swap-cli", cli_take_args).unwrap();

    let swap_ids = retry_until_swap_ids(maker_info_args.clone());

    (xmr_address, btc_address, swap_ids)
}

async fn run_swap(
    swap_id: String,
    data_dir_alice: Vec<String>,
    data_dir_bob: Vec<String>,
    bitcoin_rpc: bitcoincore_rpc::Client,
    funding_btc_address: bitcoin::Address,
    monero_regtest: monero_rpc::RegtestDaemonClient,
    monero_wallet: monero_rpc::WalletClient,
) {
    // let btc_address = bitcoin_rpc.get_new_address(None, None).unwrap();
    let cli_alice_progress_args: Vec<String> = progress_args(data_dir_alice, swap_id.clone());
    let cli_bob_progress_args: Vec<String> = progress_args(data_dir_bob, swap_id.clone());

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    // run until bob has the btc funding address
    let address = retry_until_bitcoin_funding_address(cli_bob_progress_args.clone());

    // this seems to improve reliability, probably there is a race condition somewhere with the address being funded too early
    // FIXME: change this once https://github.com/farcaster-project/farcaster-node/issues/225 is fixed
    thread::sleep(time::Duration::from_secs(10));

    // fund the bitcoin address
    let amount = bitcoin::Amount::ONE_SAT * 100000150;
    bitcoin_rpc
        .send_to_address(&address, amount, None, None, None, None, None, None)
        .unwrap();
    monero_regtest
        .generate_blocks(11, reusable_xmr_address())
        .await
        .unwrap();

    // run until the alice has the monero funding address
    let monero_address = retry_until_monero_funding_address(cli_alice_progress_args.clone());
    send_monero(&monero_wallet, monero_address, 1000000000000).await;

    // generate some bitcoin blocks for confirmations
    bitcoin_rpc
        .generate_to_address(5, &reusable_btc_address())
        .unwrap();

    // run until the AliceState(Finish) is received
    retry_until_finish_state_transition(
        cli_alice_progress_args.clone(),
        "AliceState(Finish)".to_string(),
    );

    // generate some blocks on bitcoin's side
    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let (_stdout, _stderr) = run("../swap-cli", cli_bob_progress_args.clone()).unwrap();

    // check that btc was received in the destination address
    let balance = bitcoin_rpc
        .get_received_by_address(&funding_btc_address, None)
        .unwrap();
    println!("balance: {:?}", balance.as_sat());
    assert!(balance.as_sat() > 90000000);

    // cache the monero balance before sweeping
    let before_balance = monero_wallet.get_balance(0, None).await.unwrap();

    // generate some blocks on monero's side
    monero_regtest
        .generate_blocks(11, reusable_xmr_address())
        .await
        .unwrap();

    // run until the BobState(Finish) is received
    retry_until_finish_state_transition(
        cli_bob_progress_args.clone(),
        "BobState(Finish)".to_string(),
    );

    monero_regtest
        .generate_blocks(1, reusable_xmr_address())
        .await
        .unwrap();

    monero_wallet.refresh(Some(1)).await.unwrap();
    let after_balance = monero_wallet.get_balance(0, None).await.unwrap();
    let delta_balance = after_balance.balance - before_balance.balance;
    assert!(delta_balance > 999660000000);
}

fn cleanup_processes(mut farcasterd_maker: process::Child, mut farcasterd_taker: process::Child) {
    // clean up processes
    farcasterd_maker
        .kill()
        .expect("Couldn't kill farcasterd maker");
    farcasterd_taker
        .kill()
        .expect("Couldn't kill farcasterd taker");

    let _procs: Vec<_> = System::new_all()
        .get_processes()
        .iter()
        .filter(|(_pid, process)| {
            ["peerd", "swapd", "walletd", "syncerd"].contains(&process.name())
            // && [farcasterd_maker.id(), farcasterd_taker.id()]
            // .contains(&(process.parent().unwrap() as u32))
        })
        .map(|(pid, _process)| {
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(*pid as i32),
                nix::sys::signal::Signal::SIGINT,
            )
            .expect("Sending CTRL-C failed")
        })
        .collect();
}

fn reusable_btc_address() -> bitcoin::Address {
    bitcoin::Address::from_str("bcrt1q3rc4sm3w9fr6a46n08znfjt7eu2yhhel6j8rsa").unwrap()
}

fn reusable_xmr_address() -> monero::Address {
    monero::Address::from_str("44CpGC77Kn6exUWYCUwfaUYmDeKn7MyRcNPikgeHBCz8M6LXUC3fGCWNMW7UACHyTL6QxzqKxvJbu5o2VESLzCaeNHNUkwv").unwrap()
}

fn info_args(data_dir: Vec<String>) -> Vec<String> {
    data_dir
        .into_iter()
        .chain(vec!["info".to_string()])
        .collect()
}

fn farcasterd_args(data_dir: Vec<String>, server_args: Vec<String>) -> Vec<String> {
    data_dir.into_iter().chain(server_args).collect()
}

fn make_offer_args(
    data_dir: Vec<String>,
    role: String,
    btc_addr: String,
    xmr_addr: String,
) -> Vec<String> {
    data_dir
        .into_iter()
        .chain(vec![
            "make".to_string(),
            btc_addr,
            xmr_addr,
            "Local".to_string(),
            "ECDSA".to_string(),
            "Monero".to_string(),
            "1 BTC".to_string(),
            "1 XMR".to_string(),
            role,
            "10".to_string(),
            "30".to_string(),
            "1 satoshi/vByte".to_string(),
            "127.0.0.1".to_string(),
            "0.0.0.0".to_string(),
            "9376".to_string(),
        ])
        .collect()
}

fn take_offer_args(
    data_dir: Vec<String>,
    btc_addr: String,
    xmr_addr: String,
    offer: String,
) -> Vec<String> {
    data_dir
        .into_iter()
        .chain(vec![
            "take".to_string(),
            btc_addr,
            xmr_addr,
            offer,
            "--without-validation".to_string(),
        ])
        .collect()
}

fn progress_args(data_dir: Vec<String>, swap_id: String) -> Vec<String> {
    data_dir
        .into_iter()
        .chain(vec!["progress".to_string(), swap_id])
        .collect()
}

fn retry_until_offer(args: Vec<String>) -> Vec<String> {
    for _ in 0..ALLOWED_RETRIES {
        let (stdout, _stderr) = run("../swap-cli", args.clone()).unwrap();
        let offers: Vec<String> = stdout
            .iter()
            .filter_map(|element| {
                if element.to_string().len() > 5 {
                    let pos = element.find("Offer");
                    if let Some(pos) = pos {
                        let len = element.to_string().len() - 1;
                        let offer = element[pos..len].to_string();
                        return Some(offer);
                    }
                }
                None
            })
            .collect();
        if !offers.is_empty() {
            return offers;
        }
        thread::sleep(time::Duration::from_secs(1));
    }
    panic!("timeout before any offer could be retrieved");
}

fn retry_until_swap_ids(args: Vec<String>) -> Vec<String> {
    for _ in 0..ALLOWED_RETRIES {
        let (stdout, _stderr) = run("../swap-cli", args.clone()).unwrap();
        let swap_ids: Vec<String> = stdout
            .iter()
            .filter_map(|element| {
                if element.to_string().len() > 5 {
                    if let Some(pos) = element.find("\"0x") {
                        let len = element.to_string().len() - 1;
                        let swap_id = element[pos + 1..len].to_string();
                        return Some(swap_id);
                    }
                }
                None
            })
            .collect();
        if !swap_ids.is_empty() {
            return swap_ids;
        }
        thread::sleep(time::Duration::from_secs(1));
    }
    panic!("timeout before any swapid could be retrieved");
}

fn retry_until_bitcoin_funding_address(args: Vec<String>) -> bitcoin::Address {
    for _ in 0..ALLOWED_RETRIES {
        let (stdout, _stderr) = run("../swap-cli", args.clone()).unwrap();
        // get the btc funding address
        let bitcoin_address: Vec<String> = stdout
            .iter()
            .filter_map(|element| {
                if element.to_string().len() > 5 {
                    if let Some(pos) = element.find("bcr") {
                        let len = element.to_string().len();
                        let swap_id = element[pos..len].to_string();
                        return Some(swap_id);
                    }
                }
                None
            })
            .collect();

        if !bitcoin_address.is_empty() {
            return bitcoin::Address::from_str(&bitcoin_address[0]).unwrap();
        }
        thread::sleep(time::Duration::from_secs(1));
    }
    panic!("timeout before any bitcoin funding address could be retrieved");
}

fn retry_until_monero_funding_address(args: Vec<String>) -> monero::Address {
    for _ in 0..ALLOWED_RETRIES {
        let (stdout, _stderr) = run("../swap-cli", args.clone()).unwrap();

        // get the monero funding address
        let monero_addresses: Vec<String> = stdout
            .iter()
            .filter_map(|element| {
                if element.to_string().len() > 5 && element.contains("Success: 4") {
                    let monero_address = element[9..].to_string();
                    Some(monero_address)
                } else {
                    None
                }
            })
            .collect();

        if !monero_addresses.is_empty() {
            return monero::Address::from_str(&monero_addresses[0]).unwrap();
        }
        thread::sleep(time::Duration::from_secs(1));
    }
    panic!("timeout before any monero funding address could be retrieved");
}

fn retry_until_finish_state_transition(args: Vec<String>, finish_state: String) -> Vec<String> {
    for _ in 0..ALLOWED_RETRIES {
        let (stdout, _stderr) = run("../swap-cli", args.clone()).unwrap();

        let alice_finish: Vec<String> = stdout
            .iter()
            .filter_map(|element| {
                if element.to_string().len() > 5 && element.contains(&finish_state) {
                    Some(element.to_string())
                } else {
                    None
                }
            })
            .collect();

        if !alice_finish.is_empty() {
            return alice_finish;
        }

        thread::sleep(time::Duration::from_secs(1));
    }
    panic!(
        "timeout before finish state {:?} could be retrieved",
        finish_state
    );
}

fn bitcoin_setup() -> bitcoincore_rpc::Client {
    let path = std::path::PathBuf::from_str("tests/data_dir/regtest/.cookie").unwrap();
    let bitcoin_rpc = Client::new("http://localhost:18443", Auth::CookieFile(path)).unwrap();

    // make sure a wallet is created and loaded
    if let Err(_e) = bitcoin_rpc.create_wallet("wallet", None, None, None, None) {
        bitcoin_rpc.load_wallet("wallet").unwrap();
    }
    let address = bitcoin_rpc.get_new_address(None, None).unwrap();
    bitcoin_rpc.generate_to_address(200, &address).unwrap();
    bitcoin_rpc
}

async fn monero_setup() -> (monero_rpc::RegtestDaemonClient, monero_rpc::WalletClient) {
    let daemon_client = monero_rpc::RpcClient::new("http://localhost:18081".to_string());
    let daemon = daemon_client.daemon();
    let regtest = daemon.regtest();
    let wallet_client = monero_rpc::RpcClient::new("http://localhost:18083".to_string());
    let wallet = wallet_client.wallet();
    if let Err(_e) = wallet.open_wallet("test".to_string(), None).await {
        wallet
            .create_wallet("test".to_string(), None, "English".to_string())
            .await
            .unwrap();
        wallet.open_wallet("test".to_string(), None).await.unwrap();
    }

    let address = wallet.get_address(0, None).await.unwrap();
    regtest.generate_blocks(200, address.address).await.unwrap();
    regtest
        .generate_blocks(200, reusable_xmr_address())
        .await
        .unwrap();

    (regtest, wallet)
}

pub fn run(
    name: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> io::Result<(Vec<String>, Vec<String>)> {
    let mut bin_path = std::env::current_exe().map_err(|err| {
        error!("Unable to detect binary directory: {}", err);
        err
    })?;
    bin_path.pop();

    bin_path.push(name);
    #[cfg(target_os = "windows")]
    bin_path.set_extension("exe");

    debug!(
        "Launching {} as a separate process using `{}` as binary",
        name,
        bin_path.to_string_lossy()
    );

    let res = process::Command::new(bin_path)
        .args(args)
        .output()
        .expect("failed to run command");

    let stdout = String::from_utf8_lossy(&res.stdout)
        .to_string()
        .lines()
        .map(|line| {
            let plain_bytes = strip_ansi_escapes::strip(&line).unwrap();
            str::from_utf8(&plain_bytes).unwrap().to_string()
        })
        .collect();
    let stderr = String::from_utf8_lossy(&res.stderr)
        .to_string()
        .lines()
        .map(|line| {
            let plain_bytes = strip_ansi_escapes::strip(&line).unwrap();
            str::from_utf8(&plain_bytes).unwrap().to_string()
        })
        .collect();
    Ok((stdout, stderr))
}

pub fn launch(
    name: &str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> io::Result<process::Child> {
    let mut bin_path = std::env::current_exe().map_err(|err| {
        error!("Unable to detect binary directory: {}", err);
        err
    })?;
    bin_path.pop();

    bin_path.push(name);
    #[cfg(target_os = "windows")]
    bin_path.set_extension("exe");

    debug!(
        "Launching {} as a separate process using `{}` as binary",
        name,
        bin_path.to_string_lossy()
    );

    let mut cmd = process::Command::new(bin_path);

    cmd.args(args);

    trace!("Executing `{:?}`", cmd);
    cmd.spawn().map_err(|err| {
        error!("Error launching {}: {}", name, err);
        err
    })
}

async fn send_monero(
    wallet: &monero_rpc::WalletClient,
    address: monero::Address,
    amount: u64,
) -> Vec<u8> {
    let mut destination: HashMap<monero::Address, u64> = HashMap::new();
    destination.insert(address, amount);
    let options = monero_rpc::TransferOptions {
        account_index: None,
        subaddr_indices: None,
        mixin: None,
        ring_size: None,
        unlock_time: None,
        payment_id: None,
        do_not_relay: None,
    };
    let transaction = wallet
        .transfer(
            destination.clone(),
            monero_rpc::TransferPriority::Default,
            options.clone(),
        )
        .await
        .unwrap();
    hex::decode(transaction.tx_hash.to_string()).unwrap()
}
