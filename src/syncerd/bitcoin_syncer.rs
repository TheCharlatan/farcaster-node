use crate::{LogStyle, farcaster_core::consensus::Decodable};
use crate::internet2::Duplex;
use crate::internet2::Encrypt;
use crate::internet2::TypedEnum;
use crate::rpc::request::SyncerdBridgeEvent;
use crate::rpc::Request;
use crate::syncerd::opts::Coin;
use crate::syncerd::runtime::SyncerServers;
use crate::syncerd::runtime::SyncerdTask;
use crate::syncerd::syncer_state::AddressTx;
use crate::syncerd::syncer_state::SyncerState;
use crate::syncerd::syncer_state::WatchedTransaction;
use crate::ServiceId;
use crate::{error::Error, syncerd::syncer_state::create_set};
use bitcoin::hashes::{
    hex::{FromHex, ToHex},
    Hash,
};
use bitcoin::BlockHash;
use bitcoin::Script;
use electrum_client::raw_client::RawClient;
use electrum_client::Hex32Bytes;
use electrum_client::{raw_client::ElectrumSslStream, HeaderNotification};
use electrum_client::{Client, ElectrumApi};
use farcaster_core::consensus;
use internet2::zmqsocket::Connection;
use internet2::zmqsocket::ZmqType;
use internet2::PlainTranscoder;
use internet2::ZMQ_CONTEXT;
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::io;
use std::iter::FromIterator;
use std::marker::{Send, Sized};
use std::sync::mpsc::Sender;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread;
use std::time::Duration;
use tokio::runtime::Runtime;

use hex;

use farcaster_core::bitcoin::tasks::BtcAddressAddendum;
use farcaster_core::syncer::*;

pub trait Rpc: Sized {
    fn new(electrum_server: String) -> Result<Self, electrum_client::Error>;

    fn get_height(&mut self) -> u64;

    fn send_raw_transaction(&mut self, tx: Vec<u8>) -> Result<String, electrum_client::Error>;

    fn ping(&mut self) -> Result<(), Error>;
}

pub struct ElectrumRpc {
    client: Client,
    height: u64,
    block_hash: BlockHash,
    addresses: HashMap<BtcAddressAddendum, Hex32Bytes>,
    ping_count: u8,
}

pub struct Block {
    height: u64,
    block_hash: BlockHash,
}

#[derive(Debug)]
pub struct AddressNotif {
    address: BtcAddressAddendum,
    txs: Vec<AddressTx>,
}

impl ElectrumRpc {
    pub fn subscribe_script(
        &mut self,
        address_addendum: BtcAddressAddendum,
    ) -> Result<Option<AddressNotif>, Error> {
        match self.client.script_subscribe(&bitcoin::Script::from(
            address_addendum.script_pubkey.clone(),
        )) {
            Ok(Some(script_status)) => {
                trace!("script_status:\n{:?}", &script_status);
                if self
                    .addresses
                    .insert(address_addendum.clone(), script_status)
                    .is_none()
                {
                    let txs =
                        self.handle_address_notification(address_addendum.clone(), script_status);
                    trace!("creating AddressNotif with txs: {:?}", txs);
                    let notif = AddressNotif {
                        address: address_addendum,
                        txs,
                    };
                    Ok(Some(notif))
                } else {
                    Err(Error::Farcaster(s!("address_addendum already registered")))
                }
            }
            Ok(None) => Ok(None),

            Err(e) => Err(Error::Farcaster(e.to_string())),
        }
    }
    pub fn new_block_check(&mut self) -> Vec<Block> {
        let mut blocks = vec![];
        while let Ok(Some(HeaderNotification { height, header })) = self.client.block_headers_pop()
        {
            self.height = height as u64;
            self.block_hash = header.block_hash();
            trace!("new height received: {:?}", self.height);
            blocks.push(Block {
                height: self.height,
                block_hash: self.block_hash,
            });
        }
        blocks
    }

    // check if an address received a new transaction
    pub fn address_change_check(&mut self) -> Vec<AddressNotif> {
        let mut notifs: Vec<AddressNotif> = vec![];
        for (address, previous_status) in self.addresses.clone().into_iter() {
            let mut txs: Vec<AddressTx> = vec![];
            // get pending notifications for this address/script_pubkey
            while let Ok(Some(script_status)) = self
                .client
                .script_pop(&bitcoin::Script::from(address.script_pubkey.clone()))
            {
                if script_status != previous_status {
                    trace!("creating address notifications");
                    txs.extend(self.handle_address_notification(address.clone(), script_status));
                } else {
                    trace!("state did not change for given address");
                }
            }
            if txs.len() > 0 {
                trace!("address update to notify");
                notifs.push(AddressNotif { address, txs })
            }
        }
        notifs
    }

    fn handle_address_notification(
        &mut self,
        address_addendum: BtcAddressAddendum,
        script_status: Hex32Bytes,
    ) -> Vec<AddressTx> {
        let mut txs = vec![];
        if let Some(_) = self
            .addresses
            .insert(address_addendum.clone(), script_status)
        {
            trace!(
                "updated address {:?} with script_status {:?}",
                address_addendum,
                script_status
            );
        } else {
            error!("unknown address");
        }
        // now that we have established _something_ has changed get the full transaction
        // history of the address
        let script = bitcoin::Script::from(address_addendum.script_pubkey);
        if let Ok(tx_history) = self.client.script_get_history(&script) {
            for hist in tx_history {
                trace!("history: {:?}", hist);
                let mut our_amount: u64 = 0;
                let txid = hist.tx_hash;
                // get the full transaction to calculate our_amount
                let tx = self
                    .client
                    .transaction_get(&txid)
                    .expect("cant get transaction");
                for output in tx.output.iter() {
                    if output.script_pubkey == script {
                        our_amount += output.value
                    }
                }
                // if the transaction is mined, get the blockhash of the block containing it
                let block_hash = if hist.height > 0 {
                    self.client
                        .transaction_get_verbose(&txid)
                        .expect("transaction_get_verbose")
                        .blockhash
                        .unwrap()
                        .to_vec()
                } else {
                    vec![]
                };
                txs.push(AddressTx {
                    our_amount,
                    tx_id: txid.to_vec(),
                    block_hash,
                    tx: bitcoin::consensus::serialize(&tx),
                })
            }
        } else {
            error!("Failed to get address history")
        }
        txs
    }

    fn query_transactions(&self, state: &mut SyncerState) {
        for (_, watched_tx) in state.transactions.clone().iter() {
            let tx_id = bitcoin::Txid::from_slice(&watched_tx.task.hash).unwrap();
            match self.client.transaction_get_verbose(&tx_id) {
                Ok(tx) => {
                    debug!("Updated tx: {}", &tx_id);
                    let blockhash = tx.blockhash.map(|x| x.to_vec());
                    state.change_transaction(tx.txid.to_vec(), blockhash, tx.confirmations)
                }
                Err(err) => {
                    debug!("{}", err)
                }
            }
        }
    }
}

impl Rpc for ElectrumRpc {
    fn new(electrum_server: String) -> Result<Self, electrum_client::Error> {
        let client = Client::new(&electrum_server)?;
        let header = client.block_headers_subscribe()?;
        info!("New ElectrumRpc at height {:?}", header.height);

        Ok(Self {
            client,
            addresses: none!(),
            height: header.height as u64,
            block_hash: header.header.block_hash(),
            ping_count: 0,
        })
    }

    fn send_raw_transaction(&mut self, tx: Vec<u8>) -> Result<String, electrum_client::Error> {
        let result = self.client.transaction_broadcast_raw(&tx)?;
        Ok(result.to_string())
    }

    fn get_height(&mut self) -> u64 {
        self.height
    }

    fn ping(&mut self) -> Result<(), Error> {
        if self.ping_count % 30 == 0 {
            if self.client.ping().is_err() {
                return Err(Error::Other("ping failed".to_string()));
            }
            self.ping_count = 0;
        }
        self.ping_count += 1;
        Ok(())
    }
}

pub trait Synclet {
    fn run(
        &mut self,
        rx: Receiver<SyncerdTask>,
        tx: zmq::Socket,
        syncer_address: Vec<u8>,
        syncer_servers: SyncerServers,
    );
}

pub struct BitcoinSyncer {}

impl BitcoinSyncer {
    pub fn new() -> Self {
        Self {}
    }
}

impl Synclet for BitcoinSyncer {
    fn run(
        &mut self,
        rx: Receiver<SyncerdTask>,
        tx: zmq::Socket,
        syncer_address: Vec<u8>,
        syncer_servers: SyncerServers,
    ) {
        let _handle = std::thread::spawn(move || {
            let mut state = SyncerState::new();
            let mut rpc = ElectrumRpc::new(syncer_servers.electrum_server)
                .expect("Instatiating electrum client");
            let mut connection = Connection::from_zmq_socket(ZmqType::Push, tx);
            let mut transcoder = PlainTranscoder {};
            let writer = connection.as_sender();

            state.change_height(rpc.height, rpc.block_hash.to_vec());
            info!("Entering bitcoin_syncer event loop");
            let mut i = 0;
            loop {
                // check if the server can still be reached
                rpc.ping().unwrap();

                match rx.try_recv() {
                    Ok(syncerd_task) => {
                        match syncerd_task.task {
                            Task::Abort(task) => {
                                state.abort(task.id, syncerd_task.source);
                            }
                            Task::BroadcastTransaction(task) => {
                                // TODO: match error and emit event with fail code
                                trace!("trying to broadcast tx: {:?}", task.tx.to_hex());
                                match rpc.send_raw_transaction(task.tx) {
                                    Ok(txid) => info!("successfully broadcasted tx {}", txid.addr()),
                                    Err(e) => error!("failed to broadcast tx: {}", e.err()),
                                }
                            }
                            Task::WatchAddress(task) => {
                                let mut res = std::io::Cursor::new(task.addendum.clone());
                                match BtcAddressAddendum::consensus_decode(&mut res) {
                                    Err(e) => {
                                        error!("Aborting watch address task - unable to decode address addendum: {}", e);
                                        state.abort(task.id, syncerd_task.source);
                                    }
                                    Ok(address_addendum) => {
                                        trace!("subscribing to address: {:?}", &address_addendum);
                                        state.watch_address(task, syncerd_task.source).expect("watch_address");
                                        match rpc.subscribe_script(address_addendum.clone()) {
                                            Ok(Some(address_transactions)) => {
                                                trace!(
                                                    "address transactions {:?}",
                                                    &address_transactions
                                                );
                                                state.change_address(
                                                    address_addendum,
                                                    create_set(address_transactions.txs),
                                                );
                                            }
                                            Ok(None) => {
                                                let msg = "Address subscription successful, but no transactions on this BTC address \
                                                           yet, events will be emmited when transactions are observed";
                                                debug!("{}", &msg);
                                            }
                                            Err(e) => {
                                                error!("Not Ok(Option<AddressTransactions>): {}", e)
                                            }
                                        }
                                    }
                                }
                            }
                            Task::WatchHeight(task) => {
                                state.watch_height(task, syncerd_task.source);
                            }
                            Task::WatchTransaction(task) => {
                                trace!("Watch tx task: {}", task);
                                state.watch_transaction(task, syncerd_task.source);
                            }
                        }
                        // added data to state, check if we received more from the channel before
                        // sending out events
                        continue;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        error!("disconnected");
                        return;
                    }
                    Err(TryRecvError::Empty) => {
                        // do nothing
                    }
                }

                // check and process address/script_pubkey notifications
                let mut addrs_notifs = rpc.address_change_check();
                while let Some(AddressNotif { address, txs }) = addrs_notifs.pop() {
                    trace!("processing address notification: {:?} {:?}", &address, &txs);
                    state.change_address(address, create_set(txs));
                }
                // check and process new block notifications
                let mut new_blocks = rpc.new_block_check();
                while let Some(block_notif) = new_blocks.pop() {
                    state.change_height(block_notif.height, block_notif.block_hash.to_vec());
                }
                if !state.events.is_empty() || i == 0 || i % 25 == 0 {
                    trace!("pending events: {:?}\n emmiting them now", state.events);
                    rpc.query_transactions(&mut state);
                }
                // now consume the requests
                while let Some((event, source)) = state.events.pop() {
                    let request = Request::SyncerdBridgeEvent(SyncerdBridgeEvent { event, source });
                    debug!("sending request over syncerd bridge: {:?}", request);
                    writer
                        .send_routed(
                            &syncer_address,
                            &syncer_address,
                            &syncer_address,
                            &transcoder.encrypt(request.serialize()),
                        )
                        .expect("Failed on send_routed from bitcoin_syncer");
                }

                thread::sleep(std::time::Duration::from_secs(1));
                i += 1;
            }
        });
    }
}

// #[test]
// pub fn syncer_state() {
//     let (tx, rx): (Sender<SyncerdTask>, Receiver<SyncerdTask>) =
// std::sync::mpsc::channel();     let tx_event =
// ZMQ_CONTEXT.socket(zmq::PAIR).unwrap();     let rx_event =
// ZMQ_CONTEXT.socket(zmq::PAIR).unwrap();     tx_event.connect("inproc://
// syncerdbridge").unwrap();     rx_event.bind("inproc://syncerdbridge").
// unwrap();     let mut syncer = BitcoinSyncer::new();
//     syncer.run(rx, tx_event, ServiceId::Syncer(Coin::Bitcoin).into());
//     let task = SyncerdTask {
//         task: Task::WatchHeight(WatchHeight {
//             id: 0,
//             lifetime: 100000000,
//             addendum: vec![],
//         }),
//         source: ServiceId::Syncer(Coin::Bitcoin),
//     };
//     tx.send(task).unwrap();
//     let message = rx_event.recv_multipart(0);
//     assert!(message.is_ok());
// }
