#[macro_use]
extern crate log;

use bitcoincore_rpc::RpcApi;
use farcaster_core::swap::SwapId;
use farcaster_node::bus::ctl::{BitcoinFundingInfo, FundingInfo, MoneroFundingInfo};
use farcaster_node::bus::info::{FundingInfos, NodeInfo, ProgressEvent, SwapProgress};
use farcaster_node::bus::{CheckpointEntry, StateTransition};
use futures::future::join_all;
use std::collections::HashSet;
use std::sync::Arc;
use std::time;
use sysinfo::{ProcessExt, System, SystemExt};
use tokio::sync::Mutex;

use std::collections::HashMap;
use std::process;
use std::str::FromStr;

use ntest::timeout;

use utils::fc::*;
use utils::setup_logging;

mod utils;

const ALLOWED_RETRIES: u32 = 180;

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_bob_maker_normal() {
    setup_logging();
    let execution_mutex = Arc::new(Mutex::new(0));
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (monero_regtest, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, _, data_dir_taker) = launch_farcasterd_pair().await;

    let (xmr_dest_wallet_name, bitcoin_address, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    run_swap(
        swap_id,
        data_dir_taker,
        data_dir_maker,
        Arc::clone(&bitcoin_rpc),
        bitcoin_address,
        monero_regtest,
        Arc::clone(&monero_wallet),
        xmr_dest_wallet_name,
        execution_mutex,
    )
    .await;

    kill_all();
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_taker_reconnects() {
    setup_logging();
    let (_, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, _, data_dir_taker) = launch_farcasterd_pair().await;

    let (_, _, swap_id) = make_and_take_deal_with_reconnect(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    let cli_bob_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_maker.clone(), "bitcoin".to_string());

    // run until bob has the btc funding address
    let (_, _) =
        retry_until_bitcoin_funding_address(swap_id.clone(), cli_bob_needs_funding_args.clone())
            .await;

    let (_, _, swap_id) = make_and_take_deal_with_reconnect(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Alice".to_string(),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    tokio::time::sleep(time::Duration::from_secs(20)).await;

    let cli_bob_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_taker, "bitcoin".to_string());

    // run until bob has the btc funding address
    let (_, _) =
        retry_until_bitcoin_funding_address(swap_id.clone(), cli_bob_needs_funding_args.clone())
            .await;

    kill_all();
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_bob_funds_incorrect_amount() {
    setup_logging();
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (_monero_regtest, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, _, data_dir_taker) = launch_farcasterd_pair().await;

    let (_xmr_dest_wallet_name, bitcoin_address, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    run_user_funds_incorrect_swap(
        swap_id,
        data_dir_taker,
        data_dir_maker,
        Arc::clone(&bitcoin_rpc),
        bitcoin_address,
    )
    .await;

    kill_all();
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_bob_maker_manual_bitcoin_sweep() {
    setup_logging();
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (_, monero_wallet) = monero_setup().await;

    let (farcasterd_maker, data_dir_maker, farcasterd_taker, data_dir_taker) =
        launch_farcasterd_pair().await;

    let (_, _, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    run_swap_bob_maker_manual_bitcoin_sweep(
        swap_id,
        data_dir_maker,
        Arc::clone(&bitcoin_rpc),
        farcasterd_maker,
        farcasterd_taker,
    )
    .await;
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_bob_maker_manual_monero_sweep() {
    setup_logging();
    let execution_mutex = Arc::new(Mutex::new(0));
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (monero_regtest, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, _, data_dir_taker) = launch_farcasterd_pair().await;

    let (xmr_dest_wallet_name, bitcoin_address, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    run_swap_bob_maker_manual_monero_sweep(
        swap_id,
        data_dir_taker,
        data_dir_maker,
        Arc::clone(&bitcoin_rpc),
        bitcoin_address,
        monero_regtest,
        Arc::clone(&monero_wallet),
        xmr_dest_wallet_name,
        execution_mutex,
    )
    .await;
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_bob_maker_user_abort_sweep_btc() {
    setup_logging();
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (_monero_regtest, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, _, data_dir_taker) = launch_farcasterd_pair().await;

    let (_xmr_dest_wallet_name, bitcoin_address, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    run_user_abort_swap(
        swap_id,
        data_dir_taker,
        data_dir_maker,
        Arc::clone(&bitcoin_rpc),
        bitcoin_address,
    )
    .await;

    kill_all();
}

pub mod farcaster {
    tonic::include_proto!("farcaster");
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_bob_maker_kill_peerd_before_funding_should_reconnect_success() {
    setup_logging();
    let execution_mutex = Arc::new(Mutex::new(0));
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (monero_regtest, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, _, data_dir_taker) = launch_farcasterd_pair().await;

    let (xmr_dest_wallet_name, bitcoin_address, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    tokio::time::sleep(time::Duration::from_secs(20)).await;

    kill_connected_peerd();

    run_swap(
        swap_id,
        data_dir_taker,
        data_dir_maker,
        Arc::clone(&bitcoin_rpc),
        bitcoin_address,
        monero_regtest,
        Arc::clone(&monero_wallet),
        xmr_dest_wallet_name,
        execution_mutex,
    )
    .await;

    kill_all();
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_revoke_deal_bob_maker_normal() {
    setup_logging();
    let execution_mutex = Arc::new(Mutex::new(0));
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (monero_regtest, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, _, data_dir_taker) = launch_farcasterd_pair().await;

    // first make and revoke a deal
    make_and_revoke_deal(
        data_dir_maker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    // then check if we can still swap normally
    let (xmr_dest_wallet_name, bitcoin_address, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    run_swap(
        swap_id,
        data_dir_taker,
        data_dir_maker,
        Arc::clone(&bitcoin_rpc),
        bitcoin_address,
        monero_regtest,
        Arc::clone(&monero_wallet),
        xmr_dest_wallet_name,
        execution_mutex,
    )
    .await;

    kill_all();
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_bob_maker_refund_alice_overfunds() {
    setup_logging();
    let execution_mutex = Arc::new(Mutex::new(0));
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (monero_regtest, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, _, data_dir_taker) = launch_farcasterd_pair().await;

    let (xmr_dest_wallet_name, bitcoin_address, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    run_refund_swap_alice_overfunds(
        swap_id,
        data_dir_taker,
        data_dir_maker,
        Arc::clone(&bitcoin_rpc),
        bitcoin_address,
        monero_regtest,
        Arc::clone(&monero_wallet),
        xmr_dest_wallet_name,
        execution_mutex,
    )
    .await;

    kill_all();
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_bob_maker_refund_race_cancel() {
    setup_logging();
    let execution_mutex = Arc::new(Mutex::new(0));
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (monero_regtest, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, _, data_dir_taker) = launch_farcasterd_pair().await;

    let (xmr_dest_wallet_name, bitcoin_address, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    run_refund_swap_race_cancel(
        swap_id,
        data_dir_taker,
        data_dir_maker,
        Arc::clone(&bitcoin_rpc),
        bitcoin_address,
        monero_regtest,
        Arc::clone(&monero_wallet),
        xmr_dest_wallet_name,
        execution_mutex,
    )
    .await;

    kill_all();
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_bob_maker_refund_kill_alice_after_funding() {
    setup_logging();
    let execution_mutex = Arc::new(Mutex::new(0));
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (_monero_regtest, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, farcasterd_taker, data_dir_taker) = launch_farcasterd_pair().await;

    let (_xmr_dest_wallet_name, bitcoin_address, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    run_refund_swap_kill_alice_after_funding(
        swap_id,
        data_dir_taker,
        data_dir_maker,
        Arc::clone(&bitcoin_rpc),
        bitcoin_address,
        Arc::clone(&monero_wallet),
        execution_mutex,
        farcasterd_taker,
    )
    .await;

    kill_all();
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_bob_maker_refund_alice_does_not_fund() {
    setup_logging();
    let execution_mutex = Arc::new(Mutex::new(0));
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (_monero_regtest, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, _, data_dir_taker) = launch_farcasterd_pair().await;

    let (_xmr_dest_wallet_name, bitcoin_address, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    run_refund_swap_alice_does_not_fund(
        swap_id,
        data_dir_taker,
        data_dir_maker,
        Arc::clone(&bitcoin_rpc),
        bitcoin_address,
        execution_mutex,
    )
    .await;

    kill_all();
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_bob_maker_punish_kill_bob() {
    setup_logging();
    let execution_mutex = Arc::new(Mutex::new(0));
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (monero_regtest, monero_wallet) = monero_setup().await;

    let (farcasterd_maker, data_dir_maker, _, data_dir_taker) = launch_farcasterd_pair().await;

    let (_xmr_dest_wallet_name, bitcoin_address, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    run_punish_swap_kill_bob_before_monero_funding(
        swap_id,
        data_dir_taker,
        data_dir_maker,
        Arc::clone(&bitcoin_rpc),
        bitcoin_address,
        monero_regtest,
        Arc::clone(&monero_wallet),
        execution_mutex,
        farcasterd_maker,
    )
    .await;

    kill_all();
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_bob_maker_restore_checkpoint_bob_pre_buy_alice_pre_lock() {
    setup_logging();
    let execution_mutex = Arc::new(Mutex::new(0));
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (monero_regtest, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, _, data_dir_taker) = launch_farcasterd_pair().await;

    let (xmr_dest_wallet_name, bitcoin_address, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    run_restore_checkpoint_bob_pre_buy_alice_pre_lock(
        swap_id,
        data_dir_taker,
        data_dir_maker,
        Arc::clone(&bitcoin_rpc),
        bitcoin_address,
        monero_regtest,
        Arc::clone(&monero_wallet),
        xmr_dest_wallet_name,
        execution_mutex,
    )
    .await;
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_bob_maker_restore_checkpoint_bob_pre_buy_alice_pre_buy() {
    setup_logging();
    let execution_mutex = Arc::new(Mutex::new(0));
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (monero_regtest, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, _, data_dir_taker) = launch_farcasterd_pair().await;

    let (xmr_dest_wallet_name, bitcoin_address, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    run_restore_checkpoint_bob_pre_buy_alice_pre_buy(
        swap_id,
        data_dir_taker,
        data_dir_maker,
        Arc::clone(&bitcoin_rpc),
        bitcoin_address,
        monero_regtest,
        Arc::clone(&monero_wallet),
        xmr_dest_wallet_name,
        execution_mutex,
    )
    .await;
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_bob_maker_restore_reconnect_alice_pre_lock() {
    setup_logging();
    let execution_mutex = Arc::new(Mutex::new(0));
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (monero_regtest, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, taker_farcasterd, data_dir_taker) = launch_farcasterd_pair().await;

    let (xmr_dest_wallet_name, bitcoin_address, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Bob".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    run_restore_alice_pre_lock(
        swap_id,
        data_dir_taker,
        data_dir_maker,
        Arc::clone(&bitcoin_rpc),
        bitcoin_address,
        monero_regtest,
        Arc::clone(&monero_wallet),
        xmr_dest_wallet_name,
        execution_mutex,
        taker_farcasterd,
    )
    .await;
}

#[tokio::test]
#[timeout(600000)]
#[ignore]
async fn swap_alice_maker() {
    setup_logging();
    let execution_mutex = Arc::new(Mutex::new(0));
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (monero_regtest, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, _, data_dir_taker) = launch_farcasterd_pair().await;

    let (xmr_dest_wallet_name, bitcoin_address, swap_id) = make_and_take_deal(
        data_dir_maker.clone(),
        data_dir_taker.clone(),
        "Alice".to_string(),
        Arc::clone(&bitcoin_rpc),
        Arc::clone(&monero_wallet),
        bitcoin::Amount::from_str("1 BTC").unwrap(),
        monero::Amount::from_str_with_denomination("1 XMR").unwrap(),
    )
    .await;

    run_swap(
        swap_id,
        data_dir_maker,
        data_dir_taker,
        Arc::clone(&bitcoin_rpc),
        bitcoin_address,
        monero_regtest,
        Arc::clone(&monero_wallet),
        xmr_dest_wallet_name,
        execution_mutex,
    )
    .await;

    kill_all();
}

#[derive(Debug, Clone)]
struct SwapParams {
    data_dir_bob: Vec<String>,
    data_dir_alice: Vec<String>,
    xmr_dest_wallet_name: String,
    destination_btc_address: bitcoin::Address,
}

#[tokio::test]
#[timeout(800000)]
#[ignore]
async fn swap_parallel_execution() {
    setup_logging();
    let execution_mutex = Arc::new(Mutex::new(0));
    let bitcoin_rpc = Arc::new(bitcoin_setup());
    let (monero_regtest, monero_wallet) = monero_setup().await;

    let (_, data_dir_maker, _, data_dir_taker) = launch_farcasterd_pair().await;

    let previous_deals: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let previous_swap_ids: Arc<Mutex<HashSet<SwapId>>> = Arc::new(Mutex::new(HashSet::new()));

    let mut res = Vec::new();
    for i in 0..5 {
        let xmr_amount = format!("1.{} XMR", i);
        res.push(make_and_take_deal_parallel(
            data_dir_maker.clone(),
            data_dir_taker.clone(),
            "Bob".to_string(),
            Arc::clone(&bitcoin_rpc),
            Arc::clone(&monero_wallet),
            bitcoin::Amount::from_str("1 BTC").unwrap(),
            monero::Amount::from_str_with_denomination(&xmr_amount).unwrap(),
            Arc::clone(&previous_deals),
            Arc::clone(&previous_swap_ids),
        ));
    }

    let mut results = join_all(res).await;
    let mut swap_info: HashMap<SwapId, SwapParams> = results
        .drain(..)
        .map(|(xmr_dest_wallet_name, destination_btc_address, swap_id)| {
            (
                swap_id,
                SwapParams {
                    data_dir_bob: data_dir_maker.clone(),
                    data_dir_alice: data_dir_taker.clone(),
                    xmr_dest_wallet_name,
                    destination_btc_address,
                },
            )
        })
        .collect();

    let mut res = Vec::new();
    for i in 0..5 {
        let xmr_amount = format!("1.{} XMR", i);
        res.push(make_and_take_deal_parallel(
            data_dir_maker.clone(),
            data_dir_taker.clone(),
            "Alice".to_string(),
            Arc::clone(&bitcoin_rpc),
            Arc::clone(&monero_wallet),
            bitcoin::Amount::from_str("1 BTC").unwrap(),
            monero::Amount::from_str_with_denomination(&xmr_amount).unwrap(),
            Arc::clone(&previous_deals),
            Arc::clone(&previous_swap_ids),
        ));
    }

    let mut results = join_all(res).await;
    let swap_info_alice: HashMap<SwapId, SwapParams> = results
        .drain(..)
        .map(|(xmr_dest_wallet_name, destination_btc_address, swap_id)| {
            (
                swap_id,
                SwapParams {
                    data_dir_bob: data_dir_taker.clone(),
                    data_dir_alice: data_dir_maker.clone(),
                    xmr_dest_wallet_name,
                    destination_btc_address,
                },
            )
        })
        .collect();

    swap_info.extend(swap_info_alice);

    run_swaps_parallel(
        swap_info,
        Arc::clone(&bitcoin_rpc),
        monero_regtest.clone(),
        Arc::clone(&monero_wallet.clone()),
        Arc::clone(&execution_mutex),
    )
    .await;

    kill_all();
}

#[allow(clippy::too_many_arguments)]
async fn run_restore_alice_pre_lock(
    swap_id: SwapId,
    data_dir_alice: Vec<String>,
    data_dir_bob: Vec<String>,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    funding_btc_address: bitcoin::Address,
    monero_regtest: monero_rpc::RegtestDaemonJsonRpcClient,
    monero_wallet: Arc<Mutex<monero_rpc::WalletClient>>,
    monero_dest_wallet_name: String,
    execution_mutex: Arc<Mutex<u8>>,
    alice_farcasterd: std::process::Child,
) {
    let cli_alice_progress_args: Vec<String> = progress_args(data_dir_alice.clone(), swap_id);
    let cli_bob_progress_args: Vec<String> = progress_args(data_dir_bob.clone(), swap_id);
    let cli_bob_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_bob.clone(), "bitcoin".to_string());
    let cli_alice_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_alice.clone(), "monero".to_string());

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let lock = execution_mutex.lock().await;

    // run until bob has the btc funding address
    let (address, amount) =
        retry_until_bitcoin_funding_address(swap_id, cli_bob_needs_funding_args.clone()).await;

    // fund the bitcoin address
    bitcoin_rpc
        .send_to_address(&address, amount, None, None, None, None, None, None)
        .unwrap();

    info!("waiting for Alice Core Arbitrating Setup");
    retry_until_state_transition(
        cli_alice_progress_args.clone(),
        "Alice Core Arbitrating Setup".to_string(),
    )
    .await;

    // run until Bob Refund Procedure Signatures is received
    info!("waiting for Bob Refund Procedure Signatures");
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Refund Procedure Signatures".to_string(),
    )
    .await;

    // run until the funding infos are cleared again
    info!("waiting for the bitcoin funding info to clear");
    retry_until_funding_info_cleared(swap_id, cli_bob_needs_funding_args.clone()).await;

    // wait a bit to ensure the checkpoints are written
    tokio::time::sleep(time::Duration::from_secs(1)).await;

    // kill the taker daemon and start again
    cleanup_processes(vec![alice_farcasterd]);

    let (_, data_dir_alice) = launch_farcasterd_taker();

    // wait a bit for all the daemons to start
    tokio::time::sleep(time::Duration::from_secs(1)).await;

    // restore the saved checkpoints for each alice and bob
    restore_checkpoint(swap_id, data_dir_alice.clone());

    // generate some bitcoin blocks to finalize the bitcoin arb lock tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    // run until the alice has the monero funding address
    let (monero_address, monero_amount) =
        retry_until_monero_funding_address(swap_id, cli_alice_needs_funding_args.clone()).await;
    send_monero(Arc::clone(&monero_wallet), monero_address, monero_amount).await;

    // run until the funding infos are cleared again
    info!("waiting for the monero funding info to clear");
    retry_until_funding_info_cleared(swap_id.clone(), cli_alice_needs_funding_args.clone()).await;

    // generate some monero blocks to finalize the monero acc lock tx
    monero_regtest
        .generate_blocks(6, reusable_xmr_address())
        .await
        .unwrap();

    // run until Bob Accordant Lock Final is received
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Accordant Lock Final".to_string(),
    )
    .await;

    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some bitcoin blocks to make the buy tx final
    bitcoin_rpc
        .generate_to_address(5, &reusable_btc_address())
        .unwrap();

    // run until SuccessSwap is received
    retry_until_bob_finish_state_transition(
        cli_alice_progress_args.clone(),
        "Success Swap".to_string(),
        monero_regtest.clone(),
    )
    .await;

    // generate some blocks on bitcoin's side
    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let (_stdout, _stderr) = run("../swap-cli", cli_bob_progress_args.clone()).unwrap();

    // check that btc was received in the destination address
    let balance = bitcoin_rpc
        .get_received_by_address(&funding_btc_address, None)
        .unwrap();
    assert!(balance.as_sat() > 90000000);

    // cache the monero balance before sweeping
    let monero_wallet_lock = monero_wallet.lock().await;
    monero_wallet_lock
        .open_wallet(monero_dest_wallet_name.clone(), None)
        .await
        .unwrap();
    let before_balance = monero_wallet_lock.get_balance(0, None).await.unwrap();
    drop(monero_wallet_lock);

    // Sleep here to work around a race condition between pending
    // SweepXmrAddress requests and tx Acc Lock confirmations. If Acc Lock
    // confirmations are produced before the pending request is queued, no
    // action will take place after this point.
    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some blocks on monero's side
    monero_regtest
        .generate_blocks(10, reusable_xmr_address())
        .await
        .unwrap();

    // run until SuccessSwap is received
    retry_until_bob_finish_state_transition(
        cli_bob_progress_args.clone(),
        "Success Swap".to_string(),
        monero_regtest.clone(),
    )
    .await;

    monero_regtest
        .generate_blocks(1, reusable_xmr_address())
        .await
        .unwrap();

    let monero_wallet_lock = monero_wallet.lock().await;
    monero_wallet_lock
        .open_wallet(monero_dest_wallet_name, None)
        .await
        .unwrap();
    monero_wallet_lock.refresh(Some(1)).await.unwrap();
    let after_balance = monero_wallet_lock.get_balance(0, None).await.unwrap();
    drop(monero_wallet_lock);
    drop(lock);
    let delta_balance = after_balance.balance - before_balance.balance;
    assert!(delta_balance > monero::Amount::from_pico(998000000000));

    kill_all();
}

#[allow(clippy::too_many_arguments)]
async fn run_restore_checkpoint_bob_pre_buy_alice_pre_buy(
    swap_id: SwapId,
    data_dir_alice: Vec<String>,
    data_dir_bob: Vec<String>,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    funding_btc_address: bitcoin::Address,
    monero_regtest: monero_rpc::RegtestDaemonJsonRpcClient,
    monero_wallet: Arc<Mutex<monero_rpc::WalletClient>>,
    monero_dest_wallet_name: String,
    execution_mutex: Arc<Mutex<u8>>,
) {
    let cli_alice_progress_args: Vec<String> = progress_args(data_dir_alice.clone(), swap_id);
    let cli_bob_progress_args: Vec<String> = progress_args(data_dir_bob.clone(), swap_id);
    let cli_bob_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_bob.clone(), "bitcoin".to_string());
    let cli_alice_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_alice.clone(), "monero".to_string());

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let lock = execution_mutex.lock().await;

    // run until bob has the btc funding address
    let (address, amount) =
        retry_until_bitcoin_funding_address(swap_id, cli_bob_needs_funding_args.clone()).await;

    // fund the bitcoin address
    bitcoin_rpc
        .send_to_address(&address, amount, None, None, None, None, None, None)
        .unwrap();

    info!("waiting for Alice Core Arbitrating Setup");
    retry_until_state_transition(
        cli_alice_progress_args.clone(),
        "Alice Core Arbitrating Setup".to_string(),
    )
    .await;

    // run until Bob Refund Procedure Signatures is received
    info!("waiting for Bob Refund Procedure Signatures");
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Refund Procedure Signatures".to_string(),
    )
    .await;

    // run until the funding infos are cleared again
    info!("waiting for the bitcoin funding info to clear");
    retry_until_funding_info_cleared(swap_id, cli_bob_needs_funding_args.clone()).await;

    // sleep a bit to ensure arb lock is broadcasted
    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some bitcoin blocks to finalize the bitcoin arb lock tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    // run until the alice has the monero funding address
    let (monero_address, monero_amount) =
        retry_until_monero_funding_address(swap_id, cli_alice_needs_funding_args.clone()).await;
    send_monero(Arc::clone(&monero_wallet), monero_address, monero_amount).await;

    // run until the funding infos are cleared again
    info!("waiting for the monero funding info to clear");
    retry_until_funding_info_cleared(swap_id, cli_alice_needs_funding_args.clone()).await;

    info!("Waiting for Bob Accordant Lock");
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Accordant Lock".to_string(),
    )
    .await;

    // generate some monero blocks to finalize the monero acc lock tx
    monero_regtest
        .generate_blocks(6, reusable_xmr_address())
        .await
        .unwrap();

    // run until Bob Accordant Lock Final is received
    info!("Waiting for Bob Accordant Lock Final");
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Accordant Lock Final".to_string(),
    )
    .await;

    // run until Alice Buy Procedure Signature is received
    info!("Waiting for Alice Buy Procedure Signature");
    retry_until_state_transition(
        cli_alice_progress_args.clone(),
        "Alice Buy Procedure Signature".to_string(),
    )
    .await;

    // kill all the daemons,  and start them again
    kill_all();
    let _ = launch_farcasterd_pair().await;

    // wait a bit for all the daemons to start
    tokio::time::sleep(time::Duration::from_secs(1)).await;

    // restore the saved checkpoints for each alice and bob
    restore_checkpoint(swap_id, data_dir_bob.clone());
    restore_checkpoint(swap_id, data_dir_alice.clone());

    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some bitcoin blocks to make the buy tx final
    bitcoin_rpc
        .generate_to_address(5, &reusable_btc_address())
        .unwrap();

    // run until the SuccessSwap outcome is received
    retry_until_finish_transition(cli_alice_progress_args.clone(), "Success Swap".to_string())
        .await;

    // generate some blocks on bitcoin's side
    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let (_stdout, _stderr) = run("../swap-cli", cli_bob_progress_args.clone()).unwrap();

    // check that btc was received in the destination address
    let balance = bitcoin_rpc
        .get_received_by_address(&funding_btc_address, None)
        .unwrap();
    assert!(balance.as_sat() > 90000000);

    // cache the monero balance before sweeping
    let monero_wallet_lock = monero_wallet.lock().await;
    monero_wallet_lock
        .open_wallet(monero_dest_wallet_name.clone(), None)
        .await
        .unwrap();
    let before_balance = monero_wallet_lock.get_balance(0, None).await.unwrap();
    drop(monero_wallet_lock);

    // Sleep here to work around a race condition between pending
    // SweepXmrAddress requests and tx Acc Lock confirmations. If Acc Lock
    // confirmations are produced before the pending request is queued, no
    // action will take place after this point.
    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some blocks on monero's side
    monero_regtest
        .generate_blocks(10, reusable_xmr_address())
        .await
        .unwrap();

    // run until SuccessSwap is received
    retry_until_bob_finish_state_transition(
        cli_bob_progress_args.clone(),
        "Success Swap".to_string(),
        monero_regtest.clone(),
    )
    .await;

    monero_regtest
        .generate_blocks(1, reusable_xmr_address())
        .await
        .unwrap();

    let monero_wallet_lock = monero_wallet.lock().await;
    monero_wallet_lock
        .open_wallet(monero_dest_wallet_name, None)
        .await
        .unwrap();
    monero_wallet_lock.refresh(Some(1)).await.unwrap();
    let after_balance = monero_wallet_lock.get_balance(0, None).await.unwrap();
    drop(monero_wallet_lock);
    drop(lock);
    let delta_balance = after_balance.balance - before_balance.balance;
    assert!(delta_balance > monero::Amount::from_pico(998000000000));

    kill_all();
}

#[allow(clippy::too_many_arguments)]
async fn run_restore_checkpoint_bob_pre_buy_alice_pre_lock(
    swap_id: SwapId,
    data_dir_alice: Vec<String>,
    data_dir_bob: Vec<String>,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    funding_btc_address: bitcoin::Address,
    monero_regtest: monero_rpc::RegtestDaemonJsonRpcClient,
    monero_wallet: Arc<Mutex<monero_rpc::WalletClient>>,
    monero_dest_wallet_name: String,
    execution_mutex: Arc<Mutex<u8>>,
) {
    let cli_alice_progress_args: Vec<String> = progress_args(data_dir_alice.clone(), swap_id);
    let cli_bob_progress_args: Vec<String> = progress_args(data_dir_bob.clone(), swap_id);
    let cli_bob_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_bob.clone(), "bitcoin".to_string());
    let cli_alice_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_alice.clone(), "monero".to_string());

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let lock = execution_mutex.lock().await;

    // run until bob has the btc funding address
    let (address, amount) =
        retry_until_bitcoin_funding_address(swap_id, cli_bob_needs_funding_args.clone()).await;

    // fund the bitcoin address
    bitcoin_rpc
        .send_to_address(&address, amount, None, None, None, None, None, None)
        .unwrap();

    info!("waiting for Alice Core Arbitrating Setup");
    retry_until_state_transition(
        cli_alice_progress_args.clone(),
        "Alice Core Arbitrating Setup".to_string(),
    )
    .await;

    // run until Bob Refund Procedure Signatures is received
    info!("waiting for Bob Refund Procedure Signatures");
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Refund Procedure Signatures".to_string(),
    )
    .await;

    // run until the funding infos are cleared again
    info!("waiting for the bitcoin funding info to clear");
    retry_until_funding_info_cleared(swap_id, cli_bob_needs_funding_args.clone()).await;

    // wait a bit to ensure the checkpoints are set
    tokio::time::sleep(time::Duration::from_secs(2)).await;

    // kill all the daemons and start them again
    kill_all();
    let _ = launch_farcasterd_pair().await;

    // wait a bit for all the daemons to start
    tokio::time::sleep(time::Duration::from_secs(1)).await;

    // restore the saved checkpoints for each alice and bob
    restore_checkpoint(swap_id, data_dir_bob.clone());
    restore_checkpoint(swap_id, data_dir_alice.clone());

    // the rest of the swap execution should be like a more usual refund swap
    tokio::time::sleep(time::Duration::from_secs(10)).await;
    // generate some bitcoin blocks to finalize the bitcoin arb lock tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    // run until the alice has the monero funding address and fund it
    let (monero_address, monero_amount) =
        retry_until_monero_funding_address(swap_id, cli_alice_needs_funding_args.clone()).await;
    send_monero(Arc::clone(&monero_wallet), monero_address, monero_amount).await;

    retry_until_state_transition(
        cli_alice_progress_args.clone(),
        "Alice Accordant Lock".to_string(),
    )
    .await;

    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Accordant Lock".to_string(),
    )
    .await;

    // generate some bitcoin blocks for confirmations
    bitcoin_rpc
        .generate_to_address(20, &reusable_btc_address())
        .unwrap();

    retry_until_state_transition(cli_bob_progress_args.clone(), "Bob Cancel".to_string()).await;

    // generate some bitcoin blocks to finalize the bitcoin cancel tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Cancel Final".to_string(),
    )
    .await;

    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some bitcoin blocks to finalize the bitcoin refund tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    // run until FailureRefund is received
    retry_until_finish_transition(cli_bob_progress_args.clone(), "Failure Refund".to_string())
        .await;

    // generate some blocks on bitcoin's side
    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let (_stdout, _stderr) = run("../swap-cli", cli_bob_progress_args.clone()).unwrap();

    // check that btc was received in the destination address
    let balance = bitcoin_rpc
        .get_received_by_address(&funding_btc_address, None)
        .unwrap();
    assert!(balance.as_sat() > 90000000);

    // cache the monero balance before sweeping
    let monero_wallet_lock = monero_wallet.lock().await;
    monero_wallet_lock
        .open_wallet(monero_dest_wallet_name.clone(), None)
        .await
        .unwrap();
    let before_balance = monero_wallet_lock.get_balance(0, None).await.unwrap();
    drop(monero_wallet_lock);

    // Sleep here to work around a race condition between pending
    // SweepXmrAddress requests and tx Acc Lock confirmations. If Acc Lock
    // confirmations are produced before the pending request is queued, no
    // action will take place after this point.
    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some blocks on monero's side
    monero_regtest
        .generate_blocks(10, reusable_xmr_address())
        .await
        .unwrap();

    // run until FailureRefund is received
    retry_until_finish_transition(
        cli_alice_progress_args.clone(),
        "Failure Refund".to_string(),
    )
    .await;

    monero_regtest
        .generate_blocks(1, reusable_xmr_address())
        .await
        .unwrap();

    let monero_wallet_lock = monero_wallet.lock().await;
    monero_wallet_lock
        .open_wallet(monero_dest_wallet_name, None)
        .await
        .unwrap();
    monero_wallet_lock.refresh(Some(1)).await.unwrap();
    let after_balance = monero_wallet_lock.get_balance(0, None).await.unwrap();
    drop(monero_wallet_lock);
    info!(
        "after balance: {}, before balance: {}",
        after_balance.balance, before_balance.balance
    );
    let delta_balance = after_balance.balance - before_balance.balance;
    assert!(delta_balance > monero::Amount::from_pico(998000000000));
    drop(lock);

    kill_all();
}

#[allow(clippy::too_many_arguments)]
async fn run_refund_swap_alice_overfunds(
    swap_id: SwapId,
    data_dir_alice: Vec<String>,
    data_dir_bob: Vec<String>,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    funding_btc_address: bitcoin::Address,
    monero_regtest: monero_rpc::RegtestDaemonJsonRpcClient,
    monero_wallet: Arc<Mutex<monero_rpc::WalletClient>>,
    monero_dest_wallet_name: String,
    execution_mutex: Arc<Mutex<u8>>,
) {
    let cli_bob_progress_args: Vec<String> = progress_args(data_dir_bob.clone(), swap_id);
    let cli_alice_progress_args: Vec<String> = progress_args(data_dir_alice.clone(), swap_id);
    let cli_bob_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_bob, "bitcoin".to_string());
    let cli_alice_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_alice, "monero".to_string());

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    // run until bob has the btc funding address
    let (address, amount) =
        retry_until_bitcoin_funding_address(swap_id, cli_bob_needs_funding_args.clone()).await;

    // fund the bitcoin address
    let lock = execution_mutex.lock().await;
    bitcoin_rpc
        .send_to_address(&address, amount, None, None, None, None, None, None)
        .unwrap();

    info!("waiting for Alice Core Arbitrating Setup");
    retry_until_state_transition(
        cli_alice_progress_args.clone(),
        "Alice Core Arbitrating Setup".to_string(),
    )
    .await;

    // run until Bob Refund Procedure Signatures is received
    info!("waiting for Bob Refund Procedure Signatures");
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Refund Procedure Signatures".to_string(),
    )
    .await;

    // run until the funding infos are cleared again
    info!("waiting for the bitcoin funding info to clear");
    retry_until_funding_info_cleared(swap_id, cli_bob_needs_funding_args.clone()).await;

    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some bitcoin blocks to finalize the bitcoin arb lock tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    // run until the alice has the monero funding address and fund it
    let (monero_address, monero_amount) =
        retry_until_monero_funding_address(swap_id, cli_alice_needs_funding_args.clone()).await;
    send_monero(
        Arc::clone(&monero_wallet),
        monero_address,
        monero::Amount::from_pico(monero_amount.as_pico() + 1),
    )
    .await;

    // run until the funding infos are cleared again
    info!("waiting for the monero funding info to clear");
    retry_until_funding_info_cleared(swap_id, cli_alice_needs_funding_args.clone()).await;

    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some monero blocks to finalize the monero acc lock tx
    monero_regtest
        .generate_blocks(10, reusable_xmr_address())
        .await
        .unwrap();

    // run until Bob Accordant Lock Final is received
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Accordant Lock Final".to_string(),
    )
    .await;

    // generate some bitcoin blocks for confirmations and triggering cancel
    bitcoin_rpc
        .generate_to_address(20, &reusable_btc_address())
        .unwrap();

    // run until Bob Cancel is received
    retry_until_state_transition(cli_bob_progress_args.clone(), "Bob Cancel".to_string()).await;

    // generate some bitcoin blocks to finalize the bitcoin cancel tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    // run until Bob Cancel Final is received
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Cancel Final".to_string(),
    )
    .await;

    // Wait a bit for refund to be broadcasted
    tokio::time::sleep(time::Duration::from_secs(20)).await;

    // generate some bitcoin blocks to finalize the bitcoin refund tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    // run until FailureRefund is received
    retry_until_finish_transition(cli_bob_progress_args.clone(), "Failure Refund".to_string())
        .await;

    // generate some blocks on bitcoin's side
    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let (_stdout, _stderr) = run("../swap-cli", cli_bob_progress_args.clone()).unwrap();

    // check that btc was received in the destination address
    let balance = bitcoin_rpc
        .get_received_by_address(&funding_btc_address, None)
        .unwrap();
    assert!(balance.as_sat() > 90000000);

    // cache the monero balance before sweeping
    let monero_wallet_lock = monero_wallet.lock().await;
    monero_wallet_lock
        .open_wallet(monero_dest_wallet_name.clone(), None)
        .await
        .unwrap();
    let before_balance = monero_wallet_lock.get_balance(0, None).await.unwrap();
    drop(monero_wallet_lock);

    // Sleep here to work around a race condition between pending
    // SweepXmrAddress requests and tx Acc Lock confirmations. If Acc Lock
    // confirmations are produced before the pending request is queued, no
    // action will take place after this point.
    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some blocks on monero's side
    monero_regtest
        .generate_blocks(10, reusable_xmr_address())
        .await
        .unwrap();

    // run until FailureRefund is received
    retry_until_finish_transition(
        cli_alice_progress_args.clone(),
        "Failure Refund".to_string(),
    )
    .await;

    monero_regtest
        .generate_blocks(1, reusable_xmr_address())
        .await
        .unwrap();

    let monero_wallet_lock = monero_wallet.lock().await;
    monero_wallet_lock
        .open_wallet(monero_dest_wallet_name, None)
        .await
        .unwrap();
    monero_wallet_lock.refresh(Some(1)).await.unwrap();
    let after_balance = monero_wallet_lock.get_balance(0, None).await.unwrap();
    drop(monero_wallet_lock);
    let delta_balance = after_balance.balance - before_balance.balance;
    assert!(delta_balance > monero::Amount::from_pico(998000000000));
    drop(lock);
}

#[allow(clippy::too_many_arguments)]
async fn run_refund_swap_race_cancel(
    swap_id: SwapId,
    data_dir_alice: Vec<String>,
    data_dir_bob: Vec<String>,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    funding_btc_address: bitcoin::Address,
    monero_regtest: monero_rpc::RegtestDaemonJsonRpcClient,
    monero_wallet: Arc<Mutex<monero_rpc::WalletClient>>,
    monero_dest_wallet_name: String,
    execution_mutex: Arc<Mutex<u8>>,
) {
    let cli_bob_progress_args: Vec<String> = progress_args(data_dir_bob.clone(), swap_id);
    let cli_alice_progress_args: Vec<String> = progress_args(data_dir_alice.clone(), swap_id);
    let cli_bob_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_bob, "bitcoin".to_string());
    let cli_alice_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_alice, "monero".to_string());

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    // run until bob has the btc funding address
    let (address, amount) =
        retry_until_bitcoin_funding_address(swap_id, cli_bob_needs_funding_args.clone()).await;

    // fund the bitcoin address
    let lock = execution_mutex.lock().await;
    bitcoin_rpc
        .send_to_address(&address, amount, None, None, None, None, None, None)
        .unwrap();

    info!("waiting for Alice Core Arbitrating Setup");
    retry_until_state_transition(
        cli_alice_progress_args.clone(),
        "Alice Core Arbitrating Setup".to_string(),
    )
    .await;

    // run until Bob Refund Procedure Signatures is received
    info!("waiting for Bob Refund Procedure Signatures");
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Refund Procedure Signatures".to_string(),
    )
    .await;

    // run until the funding infos are cleared again
    info!("waiting for the bitcoin funding info to clear");
    retry_until_funding_info_cleared(swap_id, cli_bob_needs_funding_args.clone()).await;

    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some bitcoin blocks to finalize the bitcoin arb lock tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    // run until the alice has the monero funding address and fund it
    let (monero_address, monero_amount) =
        retry_until_monero_funding_address(swap_id, cli_alice_needs_funding_args.clone()).await;
    send_monero(Arc::clone(&monero_wallet), monero_address, monero_amount).await;

    retry_until_state_transition(
        cli_alice_progress_args.clone(),
        "Alice Accordant Lock".to_string(),
    )
    .await;

    // generate some bitcoin blocks for confirmations
    bitcoin_rpc
        .generate_to_address(20, &reusable_btc_address())
        .unwrap();

    retry_until_state_transition(cli_bob_progress_args.clone(), "Bob Cancel".to_string()).await;

    // generate some bitcoin blocks to finalize the bitcoin cancel tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Cancel Final".to_string(),
    )
    .await;

    // wait a bit for the refund tx to be broadcasted
    tokio::time::sleep(time::Duration::from_secs(20)).await;

    // generate some bitcoin blocks to finalize the bitcoin refund tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    // run until FailureRefund is received
    retry_until_finish_transition(cli_bob_progress_args.clone(), "Failure Refund".to_string())
        .await;

    // generate some blocks on bitcoin's side
    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let (_stdout, _stderr) = run("../swap-cli", cli_bob_progress_args.clone()).unwrap();

    // check that btc was received in the destination address
    let balance = bitcoin_rpc
        .get_received_by_address(&funding_btc_address, None)
        .unwrap();
    assert!(balance.as_sat() > 90000000);

    // cache the monero balance before sweeping
    let monero_wallet_lock = monero_wallet.lock().await;
    monero_wallet_lock
        .open_wallet(monero_dest_wallet_name.clone(), None)
        .await
        .unwrap();
    let before_balance = monero_wallet_lock.get_balance(0, None).await.unwrap();
    drop(monero_wallet_lock);

    // Sleep here to work around a race condition between pending
    // SweepXmrAddress requests and tx Acc Lock confirmations. If Acc Lock
    // confirmations are produced before the pending request is queued, no
    // action will take place after this point.
    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some blocks on monero's side
    monero_regtest
        .generate_blocks(10, reusable_xmr_address())
        .await
        .unwrap();

    // run until FailureRefund is received
    retry_until_finish_transition(
        cli_alice_progress_args.clone(),
        "Failure Refund".to_string(),
    )
    .await;

    monero_regtest
        .generate_blocks(1, reusable_xmr_address())
        .await
        .unwrap();

    let monero_wallet_lock = monero_wallet.lock().await;
    monero_wallet_lock
        .open_wallet(monero_dest_wallet_name, None)
        .await
        .unwrap();
    monero_wallet_lock.refresh(Some(1)).await.unwrap();
    let after_balance = monero_wallet_lock.get_balance(0, None).await.unwrap();
    drop(monero_wallet_lock);
    let delta_balance = after_balance.balance - before_balance.balance;
    assert!(delta_balance > monero::Amount::from_pico(998000000000));
    drop(lock);
}

#[allow(clippy::too_many_arguments)]
async fn run_refund_swap_kill_alice_after_funding(
    swap_id: SwapId,
    data_dir_alice: Vec<String>,
    data_dir_bob: Vec<String>,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    funding_btc_address: bitcoin::Address,
    monero_wallet: Arc<Mutex<monero_rpc::WalletClient>>,
    execution_mutex: Arc<Mutex<u8>>,
    alice_farcasterd: std::process::Child,
) {
    let cli_bob_progress_args: Vec<String> = progress_args(data_dir_bob.clone(), swap_id);
    let cli_alice_progress_args: Vec<String> = progress_args(data_dir_alice.clone(), swap_id);
    let cli_bob_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_bob, "bitcoin".to_string());
    let cli_alice_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_alice, "monero".to_string());

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    // run until bob has the btc funding address
    let (address, amount) =
        retry_until_bitcoin_funding_address(swap_id, cli_bob_needs_funding_args.clone()).await;

    // fund the bitcoin address
    let lock = execution_mutex.lock().await;
    bitcoin_rpc
        .send_to_address(&address, amount, None, None, None, None, None, None)
        .unwrap();

    info!("waiting for Alice Core Arbitrating Setup");
    retry_until_state_transition(
        cli_alice_progress_args.clone(),
        "Alice Core Arbitrating Setup".to_string(),
    )
    .await;

    // run until Bob Refund Procedure Signatures is received
    info!("waiting for Bob Refund Procedure Signatures");
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Refund Procedure Signatures".to_string(),
    )
    .await;

    // run until the funding infos are cleared again
    info!("waiting for the bitcoin funding info to clear");
    retry_until_funding_info_cleared(swap_id, cli_bob_needs_funding_args.clone()).await;

    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some bitcoin blocks to finalize the bitcoin arb lock tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    // run until the alice has the monero funding address and fund it
    let (monero_address, monero_amount) =
        retry_until_monero_funding_address(swap_id, cli_alice_needs_funding_args.clone()).await;
    send_monero(Arc::clone(&monero_wallet), monero_address, monero_amount).await;

    // kill alice
    cleanup_processes(vec![alice_farcasterd]);

    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Accordant Lock".to_string(),
    )
    .await;

    // generate some bitcoin blocks for confirmations to trigger cancel
    bitcoin_rpc
        .generate_to_address(20, &reusable_btc_address())
        .unwrap();

    retry_until_state_transition(cli_bob_progress_args.clone(), "Bob Cancel".to_string()).await;

    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Cancel Final".to_string(),
    )
    .await;

    // Allow some time for the refund transaction to be broadcasted
    tokio::time::sleep(time::Duration::from_secs(20)).await;

    bitcoin_rpc
        .generate_to_address(2, &reusable_btc_address())
        .unwrap();

    // run until FailureRefund is received
    retry_until_finish_transition(cli_bob_progress_args.clone(), "Failure Refund".to_string())
        .await;

    // generate some blocks on bitcoin's side
    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let (_stdout, _stderr) = run("../swap-cli", cli_bob_progress_args.clone()).unwrap();

    // check that btc was received in the destination address
    let balance = bitcoin_rpc
        .get_received_by_address(&funding_btc_address, None)
        .unwrap();
    assert!(balance.as_sat() > 90000000);
    drop(lock);
}

#[allow(clippy::too_many_arguments)]
async fn run_refund_swap_alice_does_not_fund(
    swap_id: SwapId,
    data_dir_alice: Vec<String>,
    data_dir_bob: Vec<String>,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    funding_btc_address: bitcoin::Address,
    execution_mutex: Arc<Mutex<u8>>,
) {
    let cli_alice_progress_args: Vec<String> = progress_args(data_dir_alice.clone(), swap_id);
    let cli_bob_progress_args: Vec<String> = progress_args(data_dir_bob.clone(), swap_id);
    let cli_bob_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_bob, "bitcoin".to_string());
    let cli_alice_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_alice, "monero".to_string());

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    // run until bob has the btc funding address
    let (address, amount) =
        retry_until_bitcoin_funding_address(swap_id, cli_bob_needs_funding_args.clone()).await;

    // fund the bitcoin address
    let lock = execution_mutex.lock().await;
    bitcoin_rpc
        .send_to_address(&address, amount, None, None, None, None, None, None)
        .unwrap();

    info!("waiting for Alice Core Arbitrating Setup");
    retry_until_state_transition(
        cli_alice_progress_args.clone(),
        "Alice Core Arbitrating Setup".to_string(),
    )
    .await;

    // run until Bob Refund Procedure Signatures is received
    info!("waiting for Bob Refund Procedure Signatures");
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Refund Procedure Signatures".to_string(),
    )
    .await;

    // run until the funding infos are cleared again
    info!("waiting for the bitcoin funding info to clear");
    retry_until_funding_info_cleared(swap_id, cli_bob_needs_funding_args.clone()).await;

    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some bitcoin blocks to finalize the bitcoin arb lock tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    // run until the alice has the monero funding address, but do not fund it
    retry_until_monero_funding_address(swap_id, cli_alice_needs_funding_args.clone()).await;

    // generate some bitcoin blocks for confirmations
    bitcoin_rpc
        .generate_to_address(20, &reusable_btc_address())
        .unwrap();

    retry_until_state_transition(cli_bob_progress_args.clone(), "Bob Cancel".to_string()).await;

    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Cancel Final".to_string(),
    )
    .await;

    // Wait a bit for the Refund transaction to be broadcasted
    tokio::time::sleep(time::Duration::from_secs(20)).await;

    bitcoin_rpc
        .generate_to_address(2, &reusable_btc_address())
        .unwrap();

    // run until FailureRefund is received
    retry_until_finish_transition(cli_bob_progress_args.clone(), "Failure Refund".to_string())
        .await;

    // run until FailureRefund is received
    retry_until_finish_transition(
        cli_alice_progress_args.clone(),
        "Failure Refund".to_string(),
    )
    .await;

    // generate some blocks on bitcoin's side
    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let (_stdout, _stderr) = run("../swap-cli", cli_bob_progress_args.clone()).unwrap();

    // check that btc was received in the destination address
    let balance = bitcoin_rpc
        .get_received_by_address(&funding_btc_address, None)
        .unwrap();
    assert!(balance.as_sat() > 90000000);
    drop(lock);
}

#[allow(clippy::too_many_arguments)]
async fn run_punish_swap_kill_bob_before_monero_funding(
    swap_id: SwapId,
    data_dir_alice: Vec<String>,
    data_dir_bob: Vec<String>,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    funding_btc_address: bitcoin::Address,
    monero_regtest: monero_rpc::RegtestDaemonJsonRpcClient,
    monero_wallet: Arc<Mutex<monero_rpc::WalletClient>>,
    execution_mutex: Arc<Mutex<u8>>,
    bob_farcasterd: std::process::Child,
) {
    let cli_bob_progress_args: Vec<String> = progress_args(data_dir_bob.clone(), swap_id);
    let cli_alice_progress_args: Vec<String> = progress_args(data_dir_alice.clone(), swap_id);
    let cli_bob_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_bob, "bitcoin".to_string());
    let cli_alice_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_alice, "monero".to_string());

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    // run until bob has the btc funding address
    let (address, amount) =
        retry_until_bitcoin_funding_address(swap_id, cli_bob_needs_funding_args.clone()).await;

    // fund the bitcoin address
    let lock = execution_mutex.lock().await;
    bitcoin_rpc
        .send_to_address(&address, amount, None, None, None, None, None, None)
        .unwrap();

    info!("waiting for Alice Core Arbitrating Setup");
    retry_until_state_transition(
        cli_alice_progress_args.clone(),
        "Alice Core Arbitrating Setup".to_string(),
    )
    .await;

    // run until Bob Refund Procedure Signatures is received
    info!("waiting for Bob Refund Procedure Signatures");
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Refund Procedure Signatures".to_string(),
    )
    .await;

    // run until the funding infos are cleared again
    info!("waiting for the bitcoin funding info to clear");
    retry_until_funding_info_cleared(swap_id, cli_bob_needs_funding_args.clone()).await;

    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some bitcoin blocks to finalize the bitcoin arb lock tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();
    monero_regtest
        .generate_blocks(11, reusable_xmr_address())
        .await
        .unwrap();

    tokio::time::sleep(time::Duration::from_secs(20)).await;

    // kill bob
    cleanup_processes(vec![bob_farcasterd]);

    // run until alice has the monero funding address
    let (monero_address, monero_amount) =
        retry_until_monero_funding_address(swap_id, cli_alice_needs_funding_args.clone()).await;
    send_monero(Arc::clone(&monero_wallet), monero_address, monero_amount).await;

    tokio::time::sleep(time::Duration::from_secs(20)).await;

    // generate some bitcoin blocks for confirmations
    bitcoin_rpc
        .generate_to_address(20, &reusable_btc_address())
        .unwrap();

    info!("generated 20 bitcoin blocks");

    tokio::time::sleep(time::Duration::from_secs(20)).await;

    // generate some confirmations for the cancel tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    info!("generated 20 bitcoin blocks");

    tokio::time::sleep(time::Duration::from_secs(20)).await;

    // generate some bitcoin blocks for confirmations
    bitcoin_rpc
        .generate_to_address(20, &reusable_btc_address())
        .unwrap();
    info!("generated 20 bitcoin blocks");

    monero_regtest
        .generate_blocks(20, reusable_xmr_address())
        .await
        .unwrap();

    tokio::time::sleep(time::Duration::from_secs(20)).await;

    // generate some bitcoin blocks for confirmations
    bitcoin_rpc
        .generate_to_address(20, &reusable_btc_address())
        .unwrap();
    info!("generated 20 bitcoin blocks");

    tokio::time::sleep(time::Duration::from_secs(20)).await;

    // generate some confirmations for the cancel tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    info!("generated 20 bitcoin blocks");

    // run until the FailurePunish is received
    retry_until_finish_transition(
        cli_alice_progress_args.clone(),
        "Failure Punish".to_string(),
    )
    .await;

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();
    info!("generated 20 bitcoin blocks");

    // check that btc was received in the destination address
    let balance = bitcoin_rpc
        .get_received_by_address(&funding_btc_address, None)
        .unwrap();
    assert!(balance.as_sat() > 90000000);
    drop(lock);
}

#[allow(clippy::too_many_arguments)]
async fn make_and_take_deal_parallel(
    data_dir_maker: Vec<String>,
    data_dir_taker: Vec<String>,
    role: String,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    monero_wallet: Arc<Mutex<monero_rpc::WalletClient>>,
    btc_amount: bitcoin::Amount,
    xmr_amount: monero::Amount,
    previous_deals: Arc<Mutex<HashSet<String>>>,
    previous_swap_ids: Arc<Mutex<HashSet<SwapId>>>,
) -> (String, bitcoin::Address, SwapId) {
    let maker_info_args = info_args(data_dir_maker.clone());
    let taker_info_args = info_args(data_dir_maker.clone());

    // test connection to farcasterd and check that swap-cli is in the correct place
    run("../swap-cli", maker_info_args.clone()).unwrap();

    let (xmr_address, xmr_address_wallet_name) =
        monero_new_dest_address(Arc::clone(&monero_wallet)).await;
    let btc_address = bitcoin_rpc.get_new_address(None, None).unwrap();
    let btc_addr = btc_address.to_string();
    let xmr_addr = xmr_address.to_string();

    let (_stdout, _stderr) = run("../swap-cli", taker_info_args.clone()).unwrap();

    let cli_make_args = make_deal_args(
        data_dir_maker.clone(),
        role,
        btc_addr.clone(),
        btc_amount,
        xmr_addr.clone(),
        xmr_amount,
    );
    let (_stdout, _stderr) = run("../swap-cli", cli_make_args).unwrap();

    // get deal strings
    let deal =
        retry_until_deal_parallel(maker_info_args.clone(), Arc::clone(&previous_deals)).await;

    let cli_take_args = take_deal_args(data_dir_taker.clone(), btc_addr, xmr_addr, deal.clone());
    run("../swap-cli", cli_take_args).unwrap();

    let swap_id =
        retry_until_swap_id_parallel(taker_info_args.clone(), Arc::clone(&previous_swap_ids)).await;

    (xmr_address_wallet_name, btc_address, swap_id)
}

async fn make_and_revoke_deal(
    data_dir_maker: Vec<String>,
    role: String,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    monero_wallet: Arc<Mutex<monero_rpc::WalletClient>>,
    btc_amount: bitcoin::Amount,
    xmr_amount: monero::Amount,
) {
    let maker_info_args = info_args(data_dir_maker.clone());
    // test connection to farcasterd and check that swap-cli is in the correct place
    run("../swap-cli", maker_info_args.clone()).unwrap();

    let (xmr_address, _) = monero_new_dest_address(Arc::clone(&monero_wallet)).await;
    let btc_address = bitcoin_rpc.get_new_address(None, None).unwrap();
    let btc_addr = btc_address.to_string();
    let xmr_addr = xmr_address.to_string();

    let cli_make_args = make_deal_args(
        data_dir_maker.clone(),
        role,
        btc_addr.clone(),
        btc_amount,
        xmr_addr.clone(),
        xmr_amount,
    );
    let (_stdout, _stderr) = run("../swap-cli", cli_make_args).unwrap();

    // get deal string
    let deal = retry_until_deal(maker_info_args.clone()).await;
    revoke_deal(deal[0].clone(), data_dir_maker);

    assert!(get_info(maker_info_args)
        .deals
        .iter()
        .find(|o| format!("{}", o) == deal[0].clone())
        .is_none());
}

async fn make_and_take_deal(
    data_dir_maker: Vec<String>,
    data_dir_taker: Vec<String>,
    role: String,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    monero_wallet: Arc<Mutex<monero_rpc::WalletClient>>,
    btc_amount: bitcoin::Amount,
    xmr_amount: monero::Amount,
) -> (String, bitcoin::Address, SwapId) {
    let maker_info_args = info_args(data_dir_maker.clone());
    let taker_info_args = info_args(data_dir_maker.clone());

    // test connection to farcasterd and check that swap-cli is in the correct place
    run("../swap-cli", maker_info_args.clone()).unwrap();

    let (xmr_address, xmr_address_wallet_name) =
        monero_new_dest_address(Arc::clone(&monero_wallet)).await;
    let btc_address = bitcoin_rpc.get_new_address(None, None).unwrap();
    let btc_addr = btc_address.to_string();
    let xmr_addr = xmr_address.to_string();

    let (stdout, stderr) = run("../swap-cli", taker_info_args.clone()).unwrap();
    info!("stderrr: {:?}", stderr);
    let previous_swap_ids: HashSet<SwapId> =
        cli_output_to_node_info(stdout).swaps.drain(..).collect();

    let cli_make_args = make_deal_args(
        data_dir_maker.clone(),
        role,
        btc_addr.clone(),
        btc_amount,
        xmr_addr.clone(),
        xmr_amount,
    );
    let (_stdout, _stderr) = run("../swap-cli", cli_make_args).unwrap();

    // get deal strings
    let deals = retry_until_deal(maker_info_args.clone()).await;

    let cli_take_args =
        take_deal_args(data_dir_taker.clone(), btc_addr, xmr_addr, deals[0].clone());
    run("../swap-cli", cli_take_args).unwrap();

    let swap_id = retry_until_swap_id(taker_info_args.clone(), previous_swap_ids).await;

    (xmr_address_wallet_name, btc_address, swap_id)
}

async fn make_and_take_deal_with_reconnect(
    data_dir_maker: Vec<String>,
    data_dir_taker: Vec<String>,
    role: String,
    monero_wallet: Arc<Mutex<monero_rpc::WalletClient>>,
    btc_amount: bitcoin::Amount,
    xmr_amount: monero::Amount,
) -> (String, bitcoin::Address, SwapId) {
    let maker_info_args = info_args(data_dir_maker.clone());
    let taker_info_args = info_args(data_dir_maker.clone());

    // test connection to farcasterd and check that swap-cli is in the correct place
    run("../swap-cli", maker_info_args.clone()).unwrap();

    let (xmr_address, xmr_address_wallet_name) =
        monero_new_dest_address(Arc::clone(&monero_wallet)).await;
    // let btc_address = bitcoin_rpc.get_new_address(None, None).unwrap();
    let btc_address =
        bitcoin::Address::from_str("bcrt1qs758ursh4q9z627kt3pp5yysm78ddny6txaqgw").unwrap();
    let btc_addr = btc_address.to_string();
    let xmr_addr = xmr_address.to_string();

    let (stdout, _stderr) = run("../swap-cli", taker_info_args.clone()).unwrap();
    let previous_swap_ids: HashSet<SwapId> =
        cli_output_to_node_info(stdout).swaps.drain(..).collect();

    let cli_make_args = make_deal_args(
        data_dir_maker.clone(),
        role,
        btc_addr.clone(),
        btc_amount,
        xmr_addr.clone(),
        xmr_amount,
    );
    let (stdout, stderr) = run("../swap-cli", cli_make_args).unwrap();
    info!("Deal output: {:?}", stdout);
    info!("Deal error: {:?}", stderr);

    // get deal strings
    info!("retrieving deals");
    let deals = retry_until_deal(maker_info_args.clone()).await;
    info!("Got the deal: {:?}", deals);

    let cli_take_args =
        take_deal_args(data_dir_taker.clone(), btc_addr, xmr_addr, deals[0].clone());
    run("../swap-cli", cli_take_args).unwrap();
    info!("killing connected peerd");
    kill_connected_peerd();

    let swap_id = retry_until_swap_id(taker_info_args.clone(), previous_swap_ids).await;

    (xmr_address_wallet_name, btc_address, swap_id)
}

#[allow(clippy::too_many_arguments)]
async fn run_swaps_parallel(
    swap_info: HashMap<SwapId, SwapParams>,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    monero_regtest: monero_rpc::RegtestDaemonJsonRpcClient,
    monero_wallet: Arc<Mutex<monero_rpc::WalletClient>>,
    execution_mutex: Arc<Mutex<u8>>,
) {
    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let lock = execution_mutex.lock().await;

    // run until bob has the btc funding address
    for (swap_id, SwapParams { data_dir_bob, .. }) in swap_info.iter() {
        let (address, amount) = retry_until_bitcoin_funding_address(
            *swap_id,
            needs_funding_args(data_dir_bob.clone(), "bitcoin".to_string()),
        )
        .await;

        // fund the bitcoin address
        bitcoin_rpc
            .send_to_address(&address, amount, None, None, None, None, None, None)
            .unwrap();
    }

    for (swap_id, SwapParams { data_dir_alice, .. }) in swap_info.iter() {
        info!("waiting for Alice Core Arbitrating Setup");
        retry_until_state_transition(
            progress_args(data_dir_alice.clone(), *swap_id),
            "Alice Core Arbitrating Setup".to_string(),
        )
        .await;
    }

    // run until Bob Refund Procedure Signatures is received
    for (swap_id, SwapParams { data_dir_bob, .. }) in swap_info.iter() {
        info!("waiting for Bob Refund Procedure Signatures");
        retry_until_state_transition(
            progress_args(data_dir_bob.clone(), *swap_id),
            "Bob Refund Procedure Signatures".to_string(),
        )
        .await;
    }

    // run until the funding infos are cleared again
    for (swap_id, SwapParams { data_dir_bob, .. }) in swap_info.iter() {
        info!("waiting for the funding info to clear");
        retry_until_funding_info_cleared(
            *swap_id,
            needs_funding_args(data_dir_bob.clone(), "bitcoin".to_string()),
        )
        .await;
    }

    tokio::time::sleep(time::Duration::from_secs(20)).await;

    // generate some bitcoin blocks to finalize the bitcoin arb lock tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    // run until the alice has the monero funding address
    for (swap_id, SwapParams { data_dir_alice, .. }) in swap_info.iter() {
        let (monero_address, monero_amount) = retry_until_monero_funding_address(
            *swap_id,
            needs_funding_args(data_dir_alice.clone(), "monero".to_string()),
        )
        .await;
        send_monero(Arc::clone(&monero_wallet), monero_address, monero_amount).await;
    }

    // run until the funding infos are cleared again
    for (swap_id, SwapParams { data_dir_alice, .. }) in swap_info.iter() {
        info!("waiting for the funding info to clear");
        retry_until_funding_info_cleared(
            *swap_id,
            needs_funding_args(data_dir_alice.clone(), "monero".to_string()),
        )
        .await;
    }

    // generate some monero blocks to finalize the monero acc lock tx
    monero_regtest
        .generate_blocks(6, reusable_xmr_address())
        .await
        .unwrap();

    // run until Bob Accordant Lock Final is received
    for (swap_id, SwapParams { data_dir_bob, .. }) in swap_info.iter() {
        info!("waiting for Bob Accordant Lock Final");
        retry_until_state_transition(
            progress_args(data_dir_bob.clone(), *swap_id),
            "Bob Accordant Lock Final".to_string(),
        )
        .await;
    }

    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some bitcoin blocks to make the buy tx final
    bitcoin_rpc
        .generate_to_address(5, &reusable_btc_address())
        .unwrap();

    // run until the SuccessSwap is received
    for (swap_id, SwapParams { data_dir_alice, .. }) in swap_info.iter() {
        retry_until_finish_transition(
            progress_args(data_dir_alice.clone(), *swap_id),
            "Success Swap".to_string(),
        )
        .await;
    }

    // generate some blocks on bitcoin's side
    bitcoin_rpc
        .generate_to_address(2, &reusable_btc_address())
        .unwrap();

    // check that btc was received in the destination address
    for (
        _,
        SwapParams {
            destination_btc_address,
            ..
        },
    ) in swap_info.iter()
    {
        let balance = bitcoin_rpc
            .get_received_by_address(destination_btc_address, None)
            .unwrap();
        assert!(balance.as_sat() > 90000000);
    }

    // cache the monero balance before sweeping
    let mut before_balances: HashMap<SwapId, monero::Amount> = HashMap::new();
    for (
        swap_id,
        SwapParams {
            xmr_dest_wallet_name,
            ..
        },
    ) in swap_info.iter()
    {
        let monero_wallet_lock = monero_wallet.lock().await;
        monero_wallet_lock
            .open_wallet(xmr_dest_wallet_name.clone(), None)
            .await
            .unwrap();
        before_balances.insert(
            *swap_id,
            monero_wallet_lock
                .get_balance(0, None)
                .await
                .unwrap()
                .balance,
        );
        drop(monero_wallet_lock);
    }

    // Sleep here to work around a race condition between pending
    // SweepXmrAddress requests and tx Acc Lock confirmations. If Acc Lock
    // confirmations are produced before the pending request is queued, no
    // action will take place after this point.
    tokio::time::sleep(time::Duration::from_secs(20)).await;

    // generate some blocks on monero's side
    monero_regtest
        .generate_blocks(10, reusable_xmr_address())
        .await
        .unwrap();

    // run until SuccessSwap is received
    for (swap_id, SwapParams { data_dir_bob, .. }) in swap_info.iter() {
        retry_until_finish_transition(
            progress_args(data_dir_bob.clone(), *swap_id),
            "Success Swap".to_string(),
        )
        .await;
    }

    monero_regtest
        .generate_blocks(1, reusable_xmr_address())
        .await
        .unwrap();

    for (
        swap_id,
        SwapParams {
            xmr_dest_wallet_name,
            ..
        },
    ) in swap_info.iter()
    {
        let monero_wallet_lock = monero_wallet.lock().await;
        monero_wallet_lock
            .open_wallet(xmr_dest_wallet_name.clone(), None)
            .await
            .unwrap();
        monero_wallet_lock.refresh(Some(1)).await.unwrap();
        let after_balance = monero_wallet_lock.get_balance(0, None).await.unwrap();
        drop(monero_wallet_lock);
        let delta_balance = after_balance.balance - before_balances[swap_id];
        assert!(delta_balance > monero::Amount::from_pico(998000000000));
    }
    drop(lock);
}

#[allow(clippy::too_many_arguments)]
async fn run_user_abort_swap(
    swap_id: SwapId,
    data_dir_alice: Vec<String>,
    data_dir_bob: Vec<String>,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    destination_btc_address: bitcoin::Address,
) {
    let cli_bob_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_bob.clone(), "bitcoin".to_string());

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    // run until bob has the btc funding address
    let (address, amount) =
        retry_until_bitcoin_funding_address(swap_id, cli_bob_needs_funding_args.clone()).await;

    // abort the swap on Alice's side
    abort_swap(swap_id, data_dir_alice);

    // fund the bitcoin address
    bitcoin_rpc
        .send_to_address(&address, amount, None, None, None, None, None, None)
        .unwrap();

    // run until the funding infos are cleared again
    info!("waiting for the bitcoin funding info to clear");
    retry_until_funding_info_cleared(swap_id, cli_bob_needs_funding_args.clone()).await;

    // abort the swap on Bob's side
    abort_swap(swap_id, data_dir_bob);

    // wait a bit for sweep to happen
    tokio::time::sleep(time::Duration::from_secs(10)).await;
    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    // check that btc was received in the destination address
    let balance = bitcoin_rpc
        .get_received_by_address(&destination_btc_address, None)
        .unwrap();
    info!("received balance: {}", balance);
    assert!(balance.as_sat() > 90000000);
}

#[allow(clippy::too_many_arguments)]
async fn run_user_funds_incorrect_swap(
    swap_id: SwapId,
    data_dir_alice: Vec<String>,
    data_dir_bob: Vec<String>,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    destination_btc_address: bitcoin::Address,
) {
    let cli_bob_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_bob.clone(), "bitcoin".to_string());

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    // run until bob has the btc funding address
    let (address, amount) =
        retry_until_bitcoin_funding_address(swap_id, cli_bob_needs_funding_args.clone()).await;

    // abort the swap on Alice's side
    abort_swap(swap_id, data_dir_alice);

    // fund the bitcoin address
    bitcoin_rpc
        .send_to_address(
            &address,
            amount + bitcoin::Amount::from_sat(1),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

    // run until the funding infos are cleared again
    retry_until_funding_info_cleared(swap_id, cli_bob_needs_funding_args.clone()).await;

    // wait a bit for sweep to happen
    tokio::time::sleep(time::Duration::from_secs(10)).await;
    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    // check that btc was received in the destination address
    let balance = bitcoin_rpc
        .get_received_by_address(&destination_btc_address, None)
        .unwrap();
    info!("received balance: {}", balance);
    assert!(balance.as_sat() > 90000000);
}

#[allow(clippy::too_many_arguments)]
async fn run_swap_bob_maker_manual_bitcoin_sweep(
    swap_id: SwapId,
    data_dir_bob: Vec<String>,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    farcasterd_maker: process::Child,
    farcasterd_taker: process::Child,
) {
    let cli_bob_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_bob.clone(), "bitcoin".to_string());

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    // run until bob has the btc funding address
    let (address, amount) =
        retry_until_bitcoin_funding_address(swap_id.clone(), cli_bob_needs_funding_args.clone())
            .await;

    cleanup_processes(vec![farcasterd_taker]);

    // fund the bitcoin address
    bitcoin_rpc
        .send_to_address(&address, amount, None, None, None, None, None, None)
        .unwrap();

    cleanup_processes(vec![farcasterd_maker]);

    let _ = launch_farcasterd_pair().await;

    let before_balance = bitcoin_rpc.get_balance(None, None).unwrap();
    let dest_bitcoin_address = bitcoin_rpc.get_new_address(None, None).unwrap();
    // Sleep here to allow the services to warm up
    tokio::time::sleep(time::Duration::from_secs(2)).await;

    // attempt sweeping the monero wallet
    sweep_bitcoin(data_dir_bob.clone(), address, dest_bitcoin_address);

    tokio::time::sleep(time::Duration::from_secs(5)).await;

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let after_balance = bitcoin_rpc.get_balance(None, None).unwrap();
    let delta_balance = after_balance - before_balance;
    assert!(delta_balance > bitcoin::Amount::from_sat(10000000));
    kill_all();
}

#[allow(clippy::too_many_arguments)]
async fn run_swap_bob_maker_manual_monero_sweep(
    swap_id: SwapId,
    data_dir_alice: Vec<String>,
    data_dir_bob: Vec<String>,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    funding_btc_address: bitcoin::Address,
    monero_regtest: monero_rpc::RegtestDaemonJsonRpcClient,
    monero_wallet: Arc<Mutex<monero_rpc::WalletClient>>,
    monero_dest_wallet_name: String,
    execution_mutex: Arc<Mutex<u8>>,
) {
    let cli_alice_progress_args: Vec<String> = progress_args(data_dir_alice.clone(), swap_id);
    let cli_bob_progress_args: Vec<String> = progress_args(data_dir_bob.clone(), swap_id);
    let cli_bob_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_bob.clone(), "bitcoin".to_string());
    let cli_alice_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_alice, "monero".to_string());

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let lock = execution_mutex.lock().await;

    // run until bob has the btc funding address
    let (address, amount) =
        retry_until_bitcoin_funding_address(swap_id, cli_bob_needs_funding_args.clone()).await;

    // fund the bitcoin address
    bitcoin_rpc
        .send_to_address(&address, amount, None, None, None, None, None, None)
        .unwrap();

    info!("waiting for Alice Core Arbitrating Setup");
    retry_until_state_transition(
        cli_alice_progress_args.clone(),
        "Alice Core Arbitrating Setup".to_string(),
    )
    .await;

    // run until Bob Refund Procedure Signatures is received
    info!("waiting for Bob Refund Procedure Signatures");
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Refund Procedure Signatures".to_string(),
    )
    .await;

    // run until the funding infos are cleared again
    info!("waiting for the bitcoin funding info to clear");
    retry_until_funding_info_cleared(swap_id, cli_bob_needs_funding_args.clone()).await;

    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some bitcoin blocks to finalize the bitcoin arb lock tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    // run until the alice has the monero funding address
    let (monero_address, monero_amount) =
        retry_until_monero_funding_address(swap_id, cli_alice_needs_funding_args.clone()).await;
    send_monero(Arc::clone(&monero_wallet), monero_address, monero_amount).await;

    // run until the funding infos are cleared again
    info!("waiting for the monero funding info to clear");
    retry_until_funding_info_cleared(swap_id, cli_alice_needs_funding_args.clone()).await;

    // generate some monero blocks to finalize the monero acc lock tx
    monero_regtest
        .generate_blocks(6, reusable_xmr_address())
        .await
        .unwrap();

    // run until Bob Accordant Lock Final is received
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Accordant Lock Final".to_string(),
    )
    .await;

    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some bitcoin blocks to make the buy tx final
    bitcoin_rpc
        .generate_to_address(5, &reusable_btc_address())
        .unwrap();

    // run until the SuccessSwap is received
    retry_until_finish_transition(cli_alice_progress_args.clone(), "Success Swap".to_string())
        .await;

    // generate some blocks on bitcoin's side
    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let (_stdout, _stderr) = run("../swap-cli", cli_bob_progress_args.clone()).unwrap();

    // check that btc was received in the destination address
    let balance = bitcoin_rpc
        .get_received_by_address(&funding_btc_address, None)
        .unwrap();
    assert!(balance.as_sat() > 90000000);

    // cache the monero balance before sweeping
    let monero_wallet_lock = monero_wallet.lock().await;
    monero_wallet_lock
        .open_wallet(monero_dest_wallet_name.clone(), None)
        .await
        .unwrap();
    let dest_monero_address = monero_wallet_lock
        .get_address(0, None)
        .await
        .unwrap()
        .address;
    let before_balance = monero_wallet_lock.get_balance(0, None).await.unwrap();
    drop(monero_wallet_lock);

    // Sleep here to ensure the Monero keys are persisted
    tokio::time::sleep(time::Duration::from_secs(5)).await;

    // kill the processes
    kill_all();
    tokio::time::sleep(time::Duration::from_secs(20)).await;
    let _ = launch_farcasterd_pair().await;

    // generate some blocks on monero's side
    monero_regtest
        .generate_blocks(10, reusable_xmr_address())
        .await
        .unwrap();

    // Sleep here to allow the services to warm up
    tokio::time::sleep(time::Duration::from_secs(2)).await;

    // attempt sweeping the monero wallet
    sweep_monero(data_dir_bob.clone(), monero_address, dest_monero_address);

    monero_regtest
        .generate_blocks(1, reusable_xmr_address())
        .await
        .unwrap();

    let monero_wallet_lock = monero_wallet.lock().await;
    monero_wallet_lock
        .open_wallet(monero_dest_wallet_name, None)
        .await
        .unwrap();
    monero_wallet_lock.refresh(Some(1)).await.unwrap();
    let after_balance = monero_wallet_lock.get_balance(0, None).await.unwrap();
    drop(monero_wallet_lock);
    drop(lock);
    let delta_balance = after_balance.balance - before_balance.balance;
    assert!(delta_balance > monero::Amount::from_pico(998000000000));
    kill_all();
}

#[allow(clippy::too_many_arguments)]
async fn run_swap(
    swap_id: SwapId,
    data_dir_alice: Vec<String>,
    data_dir_bob: Vec<String>,
    bitcoin_rpc: Arc<bitcoincore_rpc::Client>,
    funding_btc_address: bitcoin::Address,
    monero_regtest: monero_rpc::RegtestDaemonJsonRpcClient,
    monero_wallet: Arc<Mutex<monero_rpc::WalletClient>>,
    monero_dest_wallet_name: String,
    execution_mutex: Arc<Mutex<u8>>,
) {
    let cli_alice_progress_args: Vec<String> =
        progress_args(data_dir_alice.clone(), swap_id.clone());
    let cli_bob_progress_args: Vec<String> = progress_args(data_dir_bob.clone(), swap_id.clone());
    let cli_bob_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_bob, "bitcoin".to_string());
    let cli_alice_needs_funding_args: Vec<String> =
        needs_funding_args(data_dir_alice, "monero".to_string());

    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let lock = execution_mutex.lock().await;

    // run until bob has the btc funding address
    let (address, amount) =
        retry_until_bitcoin_funding_address(swap_id.clone(), cli_bob_needs_funding_args.clone())
            .await;

    // fund the bitcoin address
    bitcoin_rpc
        .send_to_address(&address, amount, None, None, None, None, None, None)
        .unwrap();

    info!("waiting for Alice Core Arbitrating Setup");
    retry_until_state_transition(
        cli_alice_progress_args.clone(),
        "Alice Core Arbitrating Setup".to_string(),
    )
    .await;

    // run until Bob Refund Procedure Signatures is received
    info!("waiting for Bob Refund Procedure Signatures");
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Refund Procedure Signatures".to_string(),
    )
    .await;

    // run until the funding infos are cleared again
    info!("waiting for the bitcoin funding info to clear");
    retry_until_funding_info_cleared(swap_id.clone(), cli_bob_needs_funding_args.clone()).await;

    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some bitcoin blocks to finalize the bitcoin arb lock tx
    bitcoin_rpc
        .generate_to_address(3, &reusable_btc_address())
        .unwrap();

    // run until the alice has the monero funding address
    let (monero_address, monero_amount) =
        retry_until_monero_funding_address(swap_id, cli_alice_needs_funding_args.clone()).await;
    send_monero(Arc::clone(&monero_wallet), monero_address, monero_amount).await;

    // run until the funding infos are cleared again
    info!("waiting for the monero funding info to clear");
    retry_until_funding_info_cleared(swap_id.clone(), cli_alice_needs_funding_args.clone()).await;

    // generate some monero blocks to finalize the monero acc lock tx
    monero_regtest
        .generate_blocks(6, reusable_xmr_address())
        .await
        .unwrap();

    // run until Bob Accordant Lock Final is received
    retry_until_state_transition(
        cli_bob_progress_args.clone(),
        "Bob Accordant Lock Final".to_string(),
    )
    .await;

    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some bitcoin blocks to make the buy tx final
    bitcoin_rpc
        .generate_to_address(5, &reusable_btc_address())
        .unwrap();

    // run until the SuccessSwap is received
    retry_until_finish_transition(cli_alice_progress_args.clone(), "Success Swap".to_string())
        .await;

    // generate some blocks on bitcoin's side
    bitcoin_rpc
        .generate_to_address(1, &reusable_btc_address())
        .unwrap();

    let (_stdout, _stderr) = run("../swap-cli", cli_bob_progress_args.clone()).unwrap();

    // check that btc was received in the destination address
    let balance = bitcoin_rpc
        .get_received_by_address(&funding_btc_address, None)
        .unwrap();
    assert!(balance.as_sat() > 90000000);

    // cache the monero balance before sweeping
    let monero_wallet_lock = monero_wallet.lock().await;
    monero_wallet_lock
        .open_wallet(monero_dest_wallet_name.clone(), None)
        .await
        .unwrap();
    let before_balance = monero_wallet_lock.get_balance(0, None).await.unwrap();
    drop(monero_wallet_lock);

    // Sleep here to work around a race condition between pending
    // SweepXmrAddress requests and tx Acc Lock confirmations. If Acc Lock
    // confirmations are produced before the pending request is queued, no
    // action will take place after this point.
    tokio::time::sleep(time::Duration::from_secs(10)).await;

    // generate some blocks on monero's side
    monero_regtest
        .generate_blocks(10, reusable_xmr_address())
        .await
        .unwrap();

    // run until SuccessSwap is received
    retry_until_bob_finish_state_transition(
        cli_bob_progress_args.clone(),
        "Success Swap".to_string(),
        monero_regtest.clone(),
    )
    .await;

    monero_regtest
        .generate_blocks(1, reusable_xmr_address())
        .await
        .unwrap();

    let monero_wallet_lock = monero_wallet.lock().await;
    monero_wallet_lock
        .open_wallet(monero_dest_wallet_name, None)
        .await
        .unwrap();
    monero_wallet_lock.refresh(Some(1)).await.unwrap();
    let after_balance = monero_wallet_lock.get_balance(0, None).await.unwrap();
    drop(monero_wallet_lock);
    drop(lock);
    let delta_balance = after_balance.balance - before_balance.balance;
    assert!(delta_balance > monero::Amount::from_pico(998000000000));
}

fn kill_connected_peerd() {
    info!("killing peerd");
    let sys = System::new_all();
    let proc: Vec<&sysinfo::Process> = sys
        .get_processes()
        .iter()
        .filter(|(_, process)| {
            process.name() == "peerd" && process.cmd().contains(&"--listen".to_string())
        })
        .map(|(_id, process)| process)
        .collect();
    let peerd_proc = if proc[0].parent().unwrap() == proc[1].pid() {
        proc[0]
    } else {
        proc[1]
    };
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(peerd_proc.pid().into()),
        nix::sys::signal::Signal::SIGINT,
    )
    .expect("Sending CTR-C to peerd failed");
}

fn info_args(data_dir: Vec<String>) -> Vec<String> {
    data_dir
        .into_iter()
        .chain(vec!["info".to_string()])
        .collect()
}

fn make_deal_args(
    data_dir: Vec<String>,
    role: String,
    btc_addr: String,
    btc_amount: bitcoin::Amount,
    xmr_addr: String,
    xmr_amount: monero::Amount,
) -> Vec<String> {
    data_dir
        .into_iter()
        .chain(vec![
            "make".to_string(),
            "--btc-addr".to_string(),
            btc_addr,
            "--xmr-addr".to_string(),
            xmr_addr,
            "--network".to_string(),
            "Local".to_string(),
            "--arb-blockchain".to_string(),
            "Bitcoin".to_string(),
            "--acc-blockchain".to_string(),
            "Monero".to_string(),
            "--btc-amount".to_string(),
            format!("{}", btc_amount),
            "--xmr-amount".to_string(),
            format!("{}", xmr_amount),
            "--maker-role".to_string(),
            role,
            "--cancel-timelock".to_string(),
            "10".to_string(),
            "--punish-timelock".to_string(),
            "30".to_string(),
            "--fee-strategy".to_string(),
            "1 satoshi/vByte".to_string(),
            "--public-ip-addr".to_string(),
            "127.0.0.1".to_string(),
            "--public-port".to_string(),
            "7067".to_string(),
        ])
        .collect()
}

fn take_deal_args(
    data_dir: Vec<String>,
    btc_addr: String,
    xmr_addr: String,
    deal: String,
) -> Vec<String> {
    data_dir
        .into_iter()
        .chain(vec![
            "take".to_string(),
            "--btc-addr".to_string(),
            btc_addr,
            "--xmr-addr".to_string(),
            xmr_addr,
            "--deal".to_string(),
            deal,
            "--without-validation".to_string(),
        ])
        .collect()
}

fn progress_args(data_dir: Vec<String>, swap_id: SwapId) -> Vec<String> {
    data_dir
        .into_iter()
        .chain(vec!["progress".to_string(), format!("{}", swap_id)])
        .collect()
}

fn sweep_bitcoin_args(
    data_dir: Vec<String>,
    source_addr: bitcoin::Address,
    dest_addr: bitcoin::Address,
) -> Vec<String> {
    data_dir
        .into_iter()
        .chain(vec![
            "sweep-bitcoin-address".to_string(),
            format!("{}", source_addr),
            format!("{}", dest_addr),
        ])
        .collect()
}

fn sweep_monero_args(
    data_dir: Vec<String>,
    source_addr: monero::Address,
    dest_addr: monero::Address,
) -> Vec<String> {
    data_dir
        .into_iter()
        .chain(vec![
            "sweep-monero-address".to_string(),
            format!("{}", source_addr),
            format!("{}", dest_addr),
        ])
        .collect()
}

fn needs_funding_args(data_dir: Vec<String>, currency: String) -> Vec<String> {
    data_dir
        .into_iter()
        .chain(vec!["needs-funding".to_string(), currency])
        .collect()
}

fn revoke_deal_args(data_dir: Vec<String>, deal: String) -> Vec<String> {
    data_dir
        .into_iter()
        .chain(vec!["revoke-deal".to_string(), format!("{}", deal)])
        .collect()
}

fn abort_swap_args(data_dir: Vec<String>, swap_id: SwapId) -> Vec<String> {
    data_dir
        .into_iter()
        .chain(vec!["abort-swap".to_string(), format!("{}", swap_id)])
        .collect()
}

fn restore_checkpoint_args(data_dir: Vec<String>, swap_id: SwapId) -> Vec<String> {
    data_dir
        .into_iter()
        .chain(vec![
            "restore-checkpoint".to_string(),
            format!("{}", swap_id),
        ])
        .collect()
}

fn cli_output_to_node_info(stdout: Vec<String>) -> NodeInfo {
    debug!("{:?}", stdout);
    serde_yaml::from_str(
        &stdout
            .iter()
            .map(|line| format!("{}{}", line, "\n"))
            .collect::<String>(),
    )
    .unwrap()
}

fn cli_output_to_funding_infos(stdout: Vec<String>) -> FundingInfos {
    serde_yaml::from_str(
        &stdout
            .iter()
            .map(|line| format!("{}{}", line, "\n"))
            .collect::<String>(),
    )
    .unwrap()
}

async fn retry_until_deal(args: Vec<String>) -> Vec<String> {
    for _ in 0..ALLOWED_RETRIES {
        let (stdout, stderr) = run("../swap-cli", args.clone()).unwrap();
        debug!("{:?}", stderr);
        let deals: Vec<String> = cli_output_to_node_info(stdout)
            .deals
            .iter()
            .map(|deal| deal.to_string())
            .collect();
        if !deals.is_empty() {
            return deals;
        }
        tokio::time::sleep(time::Duration::from_secs(1)).await;
    }
    panic!("timeout before any deal could be retrieved");
}

fn get_info(args: Vec<String>) -> NodeInfo {
    let (stdout, _stderr) = run("../swap-cli", args).unwrap();
    cli_output_to_node_info(stdout)
}

fn sweep_bitcoin(
    data_dir: Vec<String>,
    source_addr: bitcoin::Address,
    dest_addr: bitcoin::Address,
) {
    let res = run(
        "../swap-cli",
        sweep_bitcoin_args(data_dir, source_addr, dest_addr),
    );
    info!("res: {:?}", res);
}

fn sweep_monero(data_dir: Vec<String>, source_addr: monero::Address, dest_addr: monero::Address) {
    let res = run(
        "../swap-cli",
        sweep_monero_args(data_dir, source_addr, dest_addr),
    );
    info!("res: {:?}", res);
}

fn abort_swap(swap_id: SwapId, data_dir: Vec<String>) {
    let res = run("../swap-cli", abort_swap_args(data_dir, swap_id)).unwrap();
    info!("res: {:?}", res);
}

fn revoke_deal(deal: String, data_dir: Vec<String>) {
    run("../swap-cli", revoke_deal_args(data_dir, deal)).unwrap();
}

fn restore_checkpoint(swap_id: SwapId, data_dir: Vec<String>) {
    let (stdout, _stderr) = run(
        "../swap-cli",
        data_dir
            .clone()
            .into_iter()
            .chain(vec!["list-checkpoints".to_string()])
            .collect::<Vec<String>>(),
    )
    .unwrap();

    let checkpoint_list_yaml = stdout
        .iter()
        .map(|line| format!("{}{}", line, "\n"))
        .collect::<String>();
    let checkpoint_list: Vec<CheckpointEntry> =
        serde_yaml::from_str(&checkpoint_list_yaml).unwrap();

    assert!(checkpoint_list
        .iter()
        .any(|entry| { entry.swap_id == swap_id }));

    let cli_restore_checkpoint_args = restore_checkpoint_args(data_dir, swap_id);
    let (_stdout, _stderr) = run("../swap-cli", cli_restore_checkpoint_args).unwrap();
}

async fn retry_until_deal_parallel(
    args: Vec<String>,
    previous_deals: Arc<Mutex<HashSet<String>>>,
) -> String {
    for _ in 0..ALLOWED_RETRIES {
        let mut previous_deals_lock = previous_deals.lock().await;
        let (stdout, _stderr) = run("../swap-cli", args.clone()).unwrap();
        let new_deals: HashSet<String> = cli_output_to_node_info(stdout)
            .deals
            .drain(..)
            .map(|deal| deal.to_string())
            .collect();
        if let Some(deal) = new_deals.difference(&previous_deals_lock.clone()).next() {
            previous_deals_lock.insert(deal.clone());
            drop(previous_deals_lock);
            return deal.clone();
        }
        tokio::time::sleep(time::Duration::from_secs(1)).await;
    }
    panic!("timeout before any deal could be retrieved");
}

async fn retry_until_swap_id(args: Vec<String>, previous_swap_ids: HashSet<SwapId>) -> SwapId {
    for _ in 0..ALLOWED_RETRIES {
        let (stdout, _stderr) = run("../swap-cli", args.clone()).unwrap();
        let new_swap_ids: HashSet<SwapId> =
            cli_output_to_node_info(stdout).swaps.drain(..).collect();
        if let Some(&swap_id) = new_swap_ids.difference(&previous_swap_ids).next() {
            return swap_id;
        }
        tokio::time::sleep(time::Duration::from_secs(1)).await;
    }
    panic!("timeout before any swapid could be retrieved");
}

async fn retry_until_swap_id_parallel(
    args: Vec<String>,
    previous_swap_ids: Arc<Mutex<HashSet<SwapId>>>,
) -> SwapId {
    for _ in 0..ALLOWED_RETRIES {
        let mut previous_swap_ids_lock = previous_swap_ids.lock().await;
        let (stdout, _stderr) = run("../swap-cli", args.clone()).unwrap();
        let new_swap_ids: HashSet<SwapId> =
            cli_output_to_node_info(stdout).swaps.drain(..).collect();

        if let Some(swap_id) = new_swap_ids
            .difference(&previous_swap_ids_lock.clone())
            .next()
        {
            previous_swap_ids_lock.insert(*swap_id);
            drop(previous_swap_ids_lock);
            return *swap_id;
        }
        tokio::time::sleep(time::Duration::from_secs(1)).await;
    }
    panic!("timeout before any swapid could be retrieved");
}

async fn retry_until_bitcoin_funding_address(
    swap_id: SwapId,
    args: Vec<String>,
) -> (bitcoin::Address, bitcoin::Amount) {
    for _ in 0..ALLOWED_RETRIES {
        let (stdout, _stderr) = run("../swap-cli", args.clone()).unwrap();

        let funding_infos: Vec<BitcoinFundingInfo> = cli_output_to_funding_infos(stdout)
            .swaps_need_funding
            .iter()
            .filter_map(|f| {
                if let FundingInfo::Bitcoin(info) = f {
                    if info.swap_id == swap_id {
                        Some(info)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .cloned()
            .collect();

        if !funding_infos.is_empty() {
            return (funding_infos[0].address.clone(), funding_infos[0].amount);
        }
        tokio::time::sleep(time::Duration::from_secs(1)).await;
    }
    panic!("timeout before any bitcoin funding address could be retrieved");
}

async fn retry_until_funding_info_cleared(swap_id: SwapId, args: Vec<String>) {
    for _ in 0..ALLOWED_RETRIES {
        let (stdout, _stderr) = run("../swap-cli", args.clone()).unwrap();

        let funding_infos: Vec<FundingInfo> = cli_output_to_funding_infos(stdout)
            .swaps_need_funding
            .iter()
            .filter_map(|f| match f {
                FundingInfo::Bitcoin(info) => {
                    if info.swap_id == swap_id {
                        Some(FundingInfo::Bitcoin(info.clone()))
                    } else {
                        None
                    }
                }
                FundingInfo::Monero(info) => {
                    if info.swap_id == swap_id {
                        Some(FundingInfo::Monero(info.clone()))
                    } else {
                        None
                    }
                }
            })
            .collect();

        if funding_infos.is_empty() {
            return;
        }
        tokio::time::sleep(time::Duration::from_secs(1)).await;
    }
    panic!("timeout before any bitcoin funding address could be retrieved");
}

async fn retry_until_monero_funding_address(
    swap_id: SwapId,
    args: Vec<String>,
) -> (monero::Address, monero::Amount) {
    for _ in 0..ALLOWED_RETRIES {
        let (stdout, _stderr) = run("../swap-cli", args.clone()).unwrap();
        let funding_infos: Vec<MoneroFundingInfo> = cli_output_to_funding_infos(stdout)
            .swaps_need_funding
            .iter()
            .filter_map(|f| {
                if let FundingInfo::Monero(info) = f {
                    if info.swap_id == swap_id {
                        Some(info)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .cloned()
            .collect();

        if !funding_infos.is_empty() {
            return (funding_infos[0].address, funding_infos[0].amount);
        }
        tokio::time::sleep(time::Duration::from_secs(1)).await;
    }
    panic!("timeout before any monero funding address could be retrieved");
}

fn output_to_progress(stdout: Vec<String>) -> SwapProgress {
    serde_yaml::from_str(
        &stdout
            .iter()
            .map(|line| format!("{}{}", line, "\n"))
            .collect::<String>(),
    )
    .unwrap()
}

async fn retry_until_bob_finish_state_transition(
    args: Vec<String>,
    finish_state: String,
    monero_regtest: monero_rpc::RegtestDaemonJsonRpcClient,
) -> bool {
    for _ in 0..ALLOWED_RETRIES {
        let (stdout, _stderr) = run("../swap-cli", args.clone()).unwrap();

        let progress = output_to_progress(stdout);
        if progress.progress.iter().any(|v| {
            if let ProgressEvent::StateTransition(StateTransition { new_state, .. }) = v {
                if new_state.state.contains(&finish_state) {
                    true
                } else {
                    false
                }
            } else {
                false
            }
        }) {
            return true;
        }

        monero_regtest
            .generate_blocks(1, reusable_xmr_address())
            .await
            .unwrap();

        tokio::time::sleep(time::Duration::from_secs(1)).await;
    }
    panic!(
        "timeout before finish state {:?} could be retrieved",
        finish_state
    );
}

async fn retry_until_state_transition(args: Vec<String>, finish_state: String) -> bool {
    for _ in 0..ALLOWED_RETRIES {
        let (stdout, _stderr) = run("../swap-cli", args.clone()).unwrap();

        let progress = output_to_progress(stdout);
        if progress.progress.iter().any(|v| {
            if let ProgressEvent::StateTransition(_) = v {
                v.to_string().contains(&finish_state)
            } else {
                false
            }
        }) {
            return true;
        }

        tokio::time::sleep(time::Duration::from_secs(1)).await;
    }
    panic!(
        "timeout before finish state {:?} could be retrieved",
        finish_state
    );
}

async fn retry_until_finish_transition(args: Vec<String>, finish_state: String) -> bool {
    for _ in 0..ALLOWED_RETRIES {
        let (stdout, _stderr) = run("../swap-cli", args.clone()).unwrap();

        let progress = output_to_progress(stdout);
        if progress.progress.iter().any(|v| {
            if let ProgressEvent::StateTransition(StateTransition { new_state, .. }) = v {
                if new_state.state.contains(&finish_state) {
                    true
                } else {
                    false
                }
            } else {
                false
            }
        }) {
            return true;
        }

        tokio::time::sleep(time::Duration::from_secs(1)).await;
    }
    panic!(
        "timeout before finish state {:?} could be retrieved",
        finish_state
    );
}
