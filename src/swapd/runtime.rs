// Copyright 2020-2022 Farcaster Devs & LNP/BP Standards Association
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use super::{
    swap_state::{SwapStateMachine, SwapStateMachineExecutor},
    syncer_client::{SyncerState, SyncerTasks},
    temporal_safety::TemporalSafety,
    StateReport,
};
use crate::swapd::Opts;
use crate::syncerd::types::{Event, TransactionConfirmations};
use crate::syncerd::{Abort, Task, TaskTarget};
use crate::{
    bus::ctl::{Checkpoint, CtlMsg},
    bus::info::{InfoMsg, SwapInfo},
    bus::p2p::PeerMsg,
    bus::sync::SyncMsg,
    bus::{BusMsg, ServiceBus},
    syncerd::{HeightChanged, TransactionRetrieved, XmrAddressAddendum},
};
use crate::{service::Counter, swapd::temporal_safety::SWEEP_MONERO_THRESHOLD};
use crate::{
    service::{Endpoints, Reporter},
    syncerd::AddressTransaction,
};
use crate::{CtlServer, Error, LogStyle, Service, ServiceConfig, ServiceId};

use std::any::Any;
use std::time::{Duration, SystemTime};

use bitcoin::Txid;
use colored::ColoredString;
use farcaster_core::{
    blockchain::Blockchain,
    role::{SwapRole, TradeRole},
    swap::btcxmr::{Deal, DealParameters},
    swap::SwapId,
    transaction::TxLabel,
};

use internet2::addr::{NodeAddr, NodeId};
use microservices::esb::{self, Handler};
use strict_encoding::{StrictDecode, StrictEncode};

pub fn run(config: ServiceConfig, opts: Opts) -> Result<(), Error> {
    let Opts {
        swap_id,
        deal,
        trade_role: local_trade_role,
        arbitrating_finality,
        arbitrating_safety,
        accordant_finality,
        ..
    } = opts;

    let DealParameters {
        cancel_timelock,
        punish_timelock,
        network,
        ..
    } = deal.parameters;

    let local_swap_role = deal.swap_role(&local_trade_role);

    let swap_state_machine = match (local_swap_role, local_trade_role) {
        (SwapRole::Alice, TradeRole::Maker) => SwapStateMachine::StartMaker(SwapRole::Alice),
        (SwapRole::Bob, TradeRole::Maker) => SwapStateMachine::StartMaker(SwapRole::Bob),
        (SwapRole::Alice, TradeRole::Taker) => SwapStateMachine::StartTaker(SwapRole::Alice),
        (SwapRole::Bob, TradeRole::Taker) => SwapStateMachine::StartTaker(SwapRole::Bob),
    };
    info!(
        "{}: {}",
        "Starting swap".to_string().bright_green_bold(),
        swap_id.swap_id()
    );

    let temporal_safety = TemporalSafety {
        cancel_timelock: cancel_timelock.as_u32(),
        punish_timelock: punish_timelock.as_u32(),
        arb_finality: arbitrating_finality.into(),
        safety: arbitrating_safety.into(),
        acc_finality: accordant_finality.into(),
    };

    temporal_safety.valid_params()?;
    let tasks = SyncerTasks {
        counter: Counter::new(),
        watched_addrs: none!(),
        watched_txs: none!(),
        retrieving_txs: none!(),
        sweeping_addr: none!(),
        broadcasting_txs: none!(),
        txids: none!(),
        final_txs: none!(),
        tasks: none!(),
    };
    let syncer_state = SyncerState {
        swap_id,
        local_swap_role,
        local_trade_role,
        tasks,
        monero_height: 0,
        bitcoin_height: 0,
        confirmation_bound: 50000,
        last_tx_event: none!(),
        network,
        bitcoin_syncer: ServiceId::Syncer(Blockchain::Bitcoin, network),
        monero_syncer: ServiceId::Syncer(Blockchain::Monero, network),
        awaiting_funding: false,
        xmr_addr_addendum: None,
        confirmations: none!(),
        broadcasted_txs: none!(),
    };

    let state_report = StateReport::new("Start".to_string(), &temporal_safety, &syncer_state);

    let runtime = Runtime {
        swap_id,
        identity: ServiceId::Swap(swap_id),
        peer_service: ServiceId::dummy_peer_service_id(NodeAddr {
            id: NodeId::from(deal.node_id), // node_id is bitcoin::Pubkey
            addr: deal.peer_address,        // peer_address is InetSocketAddr
        }),
        connected: false,
        started: SystemTime::now(),
        syncer_state,
        temporal_safety,
        enquirer: None,
        pending_peer_request: none!(),
        deal,
        local_trade_role,
        local_swap_role,
        latest_state_report: state_report,
        swap_state_machine,
        unhandled_peer_message: None, // The last message we received and was not handled by the state machine
    };
    let broker = false;
    Service::run(config, runtime, broker)
}

pub struct Runtime {
    pub swap_id: SwapId,
    pub identity: ServiceId,
    pub peer_service: ServiceId,
    pub connected: bool,
    pub started: SystemTime,
    pub enquirer: Option<ServiceId>,
    pub syncer_state: SyncerState,
    pub temporal_safety: TemporalSafety,
    pub pending_peer_request: Vec<PeerMsg>, // Peer requests that failed and are waiting for reconnection
    pub deal: Deal,
    pub local_trade_role: TradeRole,
    pub local_swap_role: SwapRole,
    pub latest_state_report: StateReport,
    pub swap_state_machine: SwapStateMachine,
    pub unhandled_peer_message: Option<PeerMsg>,
}

#[derive(Debug, Clone, Display, StrictEncode, StrictDecode)]
#[display("checkpoint-swapd")]
pub struct CheckpointSwapd {
    pub state: SwapStateMachine,
    pub pending_msg: Option<PeerMsg>,
    pub enquirer: Option<ServiceId>,
    pub xmr_addr_addendum: Option<XmrAddressAddendum>,
    pub temporal_safety: TemporalSafety,
    pub txids: Vec<(TxLabel, Txid)>,
    pub pending_broadcasts: Vec<(bitcoin::Transaction, TxLabel)>,
    pub local_trade_role: TradeRole,
    pub connected_counterparty_node_id: Option<NodeId>,
    pub deal: Deal,
}

impl CtlServer for Runtime {}
impl Reporter for Runtime {
    fn report_to(&self) -> Option<ServiceId> {
        self.enquirer.clone()
    }
}

impl SwapLogging for Runtime {
    fn swap_info(&self) -> (SwapId, SwapRole, TradeRole) {
        (self.swap_id, self.local_swap_role, self.local_trade_role)
    }
}

impl esb::Handler<ServiceBus> for Runtime {
    type Request = BusMsg;
    type Error = Error;

    fn identity(&self) -> ServiceId {
        self.identity.clone()
    }

    fn handle(
        &mut self,
        endpoints: &mut Endpoints,
        bus: ServiceBus,
        source: ServiceId,
        request: BusMsg,
    ) -> Result<(), Self::Error> {
        match (bus, request) {
            // Peer-to-peer message bus, only accept peer message
            (ServiceBus::Msg, BusMsg::P2p(req)) => {
                self.handle_msg(endpoints, source, req)?;
                self.report_potential_state_change(endpoints)
            }
            // Control bus for issuing control commands, only accept Ctl message
            (ServiceBus::Ctl, BusMsg::Ctl(req)) => {
                self.handle_ctl(endpoints, source, req)?;
                self.report_potential_state_change(endpoints)
            }
            // Info command bus, only accept Info message
            (ServiceBus::Info, BusMsg::Info(req)) => self.handle_info(endpoints, source, req),
            // Syncer event bus for blockchain tasks and events, only accept Sync message
            (ServiceBus::Sync, BusMsg::Sync(req)) => {
                self.handle_sync(endpoints, source, req)?;
                self.report_potential_state_change(endpoints)
            }
            // All other pairs are not supported
            (bus, req) => Err(Error::NotSupported(bus, req.to_string())),
        }
    }

    fn handle_err(&mut self, _: &mut Endpoints, _: esb::Error<ServiceId>) -> Result<(), Error> {
        // We do nothing and do not propagate error; it's already being reported
        // with `error!` macro by the controller. If we propagate error here
        // this will make whole daemon panic
        Ok(())
    }
}

impl Runtime {
    pub fn send_peer(&mut self, endpoints: &mut Endpoints, msg: PeerMsg) -> Result<(), Error> {
        self.log_trace(format!(
            "sending peer message {} to {}",
            msg, self.peer_service
        ));
        if let Err(error) = endpoints.send_to(
            ServiceBus::Msg,
            self.identity(),
            self.peer_service.clone(),
            BusMsg::P2p(msg.clone()),
        ) {
            self.log_error(format!(
                "could not send message {} to {} due to {}",
                msg, self.peer_service, error
            ));
            self.connected = false;
            self.log_warn(
                "notifying farcasterd of peer error, farcasterd will attempt to reconnect",
            );
            endpoints.send_to(
                ServiceBus::Ctl,
                self.identity(),
                ServiceId::Farcasterd,
                BusMsg::Ctl(CtlMsg::PeerdUnreachable(self.peer_service.clone())),
            )?;
            self.pending_peer_request.push(msg);
        }
        Ok(())
    }

    pub fn swap_id(&self) -> SwapId {
        match self.identity {
            ServiceId::Swap(swap_id) => swap_id,
            _ => {
                unreachable!("not ServiceId::Swap")
            }
        }
    }

    pub fn broadcast(
        &mut self,
        tx: bitcoin::Transaction,
        tx_label: TxLabel,
        endpoints: &mut Endpoints,
    ) -> Result<(), Error> {
        self.log_info(format!(
            "Broadcasting {} tx({})",
            tx_label.label(),
            tx.txid().tx_hash()
        ));
        let task = self.syncer_state.broadcast(tx, tx_label);
        Ok(endpoints.send_to(
            ServiceBus::Sync,
            self.identity(),
            self.syncer_state.bitcoin_syncer(),
            BusMsg::Sync(SyncMsg::Task(task)),
        )?)
    }

    fn handle_msg(
        &mut self,
        endpoints: &mut Endpoints,
        source: ServiceId,
        request: PeerMsg,
    ) -> Result<(), Error> {
        // Check if message are from consistent peer source
        if matches!(source, ServiceId::Peer(..)) && self.peer_service != source {
            let msg = format!(
                "Incorrect peer connection: expected {}, found {}",
                self.peer_service, source
            );
            self.log_error(&msg);
            return Err(Error::Farcaster(msg));
        }

        if request.swap_id() != self.swap_id() {
            let msg = format!(
                "{} | Incorrect swap_id: expected {}, found {}",
                self.swap_id.bright_blue_italic(),
                self.swap_id(),
                request.swap_id(),
            );
            self.log_error(&msg);
            return Err(Error::Farcaster(msg));
        }

        match request {
            // bob and alice
            PeerMsg::Abort(_) => {
                return Err(Error::Farcaster("Abort not yet supported".to_string()));
            }

            PeerMsg::Ping(_) | PeerMsg::Pong(_) | PeerMsg::PingPeer => {
                return Err(Error::Farcaster(
                    "Ping/Pong must remain in peerd, not supported in swapd".to_string(),
                ));
            }
            _ => {}
        }

        self.execute_state_machine(endpoints, BusMsg::P2p(request), source)?;

        Ok(())
    }

    pub fn handle_ctl(
        &mut self,
        endpoints: &mut Endpoints,
        source: ServiceId,
        request: CtlMsg,
    ) -> Result<(), Error> {
        match request {
            CtlMsg::Hello => {
                self.log_debug(format!(
                    "Received Hello from {}",
                    source.bright_green_bold(),
                ));
            }
            CtlMsg::Terminate if source == ServiceId::Farcasterd => {
                self.log_info(format!("Terminating {}", self.identity()).label());
                std::process::exit(0);
            }

            CtlMsg::Disconnected => {
                self.connected = false;
            }

            CtlMsg::Reconnected => {
                self.connected = true;
            }

            // Set the reconnected service id. This can happen if this is a
            // maker launched swap after restoration and the taker reconnects,
            // after a manual connect call, or a new connection with the same
            // node address is established
            CtlMsg::PeerdReconnected(service_id) => {
                self.log_info(format!("Peer {} reconnected", service_id));
                self.peer_service = service_id;
                self.connected = true;
                for msg in self.pending_peer_request.clone().iter() {
                    self.send_peer(endpoints, msg.clone())?;
                }
                self.pending_peer_request.clear();
            }

            CtlMsg::FailedPeerMessage(msg) => {
                self.log_warn(format!(
                    "Sending the peer message {} failed. Adding to pending peer requests",
                    msg
                ));
                self.pending_peer_request.push(msg);
            }

            CtlMsg::Checkpoint(Checkpoint { swap_id: _, state }) => {
                let CheckpointSwapd {
                    pending_msg,
                    enquirer,
                    temporal_safety,
                    mut txids,
                    mut pending_broadcasts,
                    xmr_addr_addendum,
                    local_trade_role,
                    state,
                    ..
                } = state;
                self.log_info("Restoring swap");
                self.swap_state_machine = state;
                self.enquirer = enquirer;
                self.temporal_safety = temporal_safety;
                // We need to update the peerd for the pending requests in case of reconnect
                self.local_trade_role = local_trade_role;
                self.syncer_state
                    .watch_height(endpoints, Blockchain::Bitcoin)?;
                self.syncer_state
                    .watch_height(endpoints, Blockchain::Monero)?;

                self.log_trace("Watching transactions");
                for (tx_label, txid) in txids.drain(..) {
                    let task = self.syncer_state.watch_tx_btc(txid, tx_label);
                    endpoints.send_to(
                        ServiceBus::Sync,
                        self.identity(),
                        self.syncer_state.bitcoin_syncer(),
                        BusMsg::Sync(SyncMsg::Task(task)),
                    )?;
                }

                self.log_trace("Broadcasting txs pending broadcast");
                for (tx, label) in pending_broadcasts.drain(..) {
                    let task = self.syncer_state.broadcast(tx.clone(), label);
                    endpoints.send_to(
                        ServiceBus::Sync,
                        self.identity(),
                        self.syncer_state.bitcoin_syncer(),
                        BusMsg::Sync(SyncMsg::Task(task)),
                    )?;
                }

                if let Some(XmrAddressAddendum {
                    view_key,
                    address,
                    from_height,
                }) = xmr_addr_addendum
                {
                    let task = self.syncer_state.watch_addr_xmr(
                        address,
                        view_key,
                        TxLabel::AccLock,
                        from_height,
                    );
                    endpoints.send_to(
                        ServiceBus::Sync,
                        self.identity(),
                        self.syncer_state.monero_syncer(),
                        BusMsg::Sync(SyncMsg::Task(task)),
                    )?;
                }

                if let Some(msg) = pending_msg {
                    self.send_peer(endpoints, msg)?;
                }
            }

            req => {
                self.execute_state_machine(endpoints, BusMsg::Ctl(req), source)?;
            }
        }

        Ok(())
    }

    fn handle_info(
        &mut self,
        endpoints: &mut Endpoints,
        source: ServiceId,
        request: InfoMsg,
    ) -> Result<(), Error> {
        match request {
            InfoMsg::GetInfo => {
                let connection = self.peer_service.node_addr();
                let info = SwapInfo {
                    swap_id: self.swap_id,
                    connection,
                    connected: self.connected,
                    state: self.latest_state_report.clone(),
                    uptime: SystemTime::now()
                        .duration_since(self.started)
                        .unwrap_or_else(|_| Duration::from_secs(0)),
                    since: self
                        .started
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_else(|_| Duration::from_secs(0))
                        .as_secs(),
                    deal: self.deal.clone(),
                    local_trade_role: self.local_trade_role,
                    local_swap_role: self.deal.swap_role(&self.local_trade_role),
                    connected_counterparty_node_id: self.peer_service.node_id(),
                };
                self.send_client_info(endpoints, source, InfoMsg::SwapInfo(info))?;
            }

            req => {
                self.log_error(format!(
                    "BusMsg {} is not supported by the INFO interface",
                    req
                ));
            }
        }

        Ok(())
    }

    pub fn handle_sync(
        &mut self,
        endpoints: &mut Endpoints,
        source: ServiceId,
        request: SyncMsg,
    ) -> Result<(), Error> {
        match request {
            SyncMsg::Event(ref event) if source == self.syncer_state.monero_syncer => {
                match &event {
                    Event::HeightChanged(HeightChanged { height, .. }) => {
                        self.syncer_state
                            .handle_height_change(*height, Blockchain::Monero);
                    }

                    Event::TransactionConfirmations(TransactionConfirmations {
                        id,
                        confirmations,
                        ..
                    }) => {
                        self.syncer_state.handle_tx_confs(
                            id,
                            confirmations,
                            self.swap_id(),
                            self.temporal_safety.acc_finality,
                            endpoints,
                        );

                        // saving requests of interest for later replaying latest event
                        if let Some(txlabel) = self.syncer_state.tasks.watched_txs.get(id) {
                            self.syncer_state
                                .last_tx_event
                                .insert(*txlabel, request.clone());
                        }
                    }

                    Event::AddressTransaction(AddressTransaction { id, .. }) => {
                        // saving requests of interest for later replaying latest event
                        if let Some(txlabel) = self.syncer_state.tasks.watched_addrs.get(id) {
                            self.syncer_state
                                .last_tx_event
                                .insert(*txlabel, request.clone());
                        }
                    }

                    Event::SweepSuccess(_) => {}

                    Event::TaskAborted(_) => {}

                    Event::Empty(_) => {}

                    event => {
                        self.log_error(format!("event not handled {}", event));
                    }
                };
            }

            SyncMsg::Event(ref event) if source == self.syncer_state.bitcoin_syncer => {
                match &event {
                    Event::HeightChanged(HeightChanged { height, .. }) => {
                        self.syncer_state
                            .handle_height_change(*height, Blockchain::Bitcoin);
                    }

                    // This re-triggers the tx fetch event in case the transaction was not detected yet
                    Event::TransactionRetrieved(TransactionRetrieved { id, tx: None })
                        if self.syncer_state.tasks.retrieving_txs.contains_key(id)
                            && self.syncer_state.tasks.tasks.contains_key(id) =>
                    {
                        let task = self.syncer_state.tasks.tasks.get(id).unwrap();
                        std::thread::sleep(core::time::Duration::from_millis(500));
                        endpoints.send_to(
                            ServiceBus::Sync,
                            self.identity(),
                            self.syncer_state.bitcoin_syncer(),
                            BusMsg::Sync(SyncMsg::Task(task.clone())),
                        )?;
                    }

                    Event::TransactionConfirmations(TransactionConfirmations {
                        id,
                        confirmations: Some(confirmations),
                        ..
                    }) if self
                        .temporal_safety
                        .final_tx(*confirmations, Blockchain::Bitcoin)
                        && self.syncer_state.tasks.watched_txs.get(id).is_some() =>
                    {
                        self.syncer_state.handle_tx_confs(
                            id,
                            &Some(*confirmations),
                            self.swap_id(),
                            self.temporal_safety.arb_finality,
                            endpoints,
                        );
                        // saving requests of interest for later replaying latest event
                        if let Some(txlabel) = self.syncer_state.tasks.watched_txs.get(id) {
                            self.syncer_state
                                .last_tx_event
                                .insert(*txlabel, request.clone());
                        }
                    }

                    Event::TransactionConfirmations(TransactionConfirmations {
                        id,
                        confirmations,
                        ..
                    }) => {
                        self.syncer_state.handle_tx_confs(
                            id,
                            confirmations,
                            self.swap_id(),
                            self.temporal_safety.arb_finality,
                            endpoints,
                        );
                        // saving requests of interest for later replaying latest event
                        if let Some(txlabel) = self.syncer_state.tasks.watched_txs.get(id) {
                            self.syncer_state
                                .last_tx_event
                                .insert(*txlabel, request.clone());
                        }
                    }

                    Event::TransactionBroadcasted(event) => {
                        self.syncer_state.transaction_broadcasted(event);
                    }

                    Event::AddressTransaction(AddressTransaction { id, .. }) => {
                        // saving requests of interest for later replaying latest event
                        if let Some(txlabel) = self.syncer_state.tasks.watched_addrs.get(id) {
                            self.syncer_state
                                .last_tx_event
                                .insert(*txlabel, request.clone());
                        }
                        self.log_debug(event);
                    }

                    Event::TaskAborted(event) => {
                        self.log_debug(event);
                    }

                    Event::SweepSuccess(event) => {
                        self.log_debug(event);
                    }

                    Event::TransactionRetrieved(event) => {
                        self.log_debug(event);
                    }

                    Event::AddressBalance(event) => {
                        self.log_debug(event);
                    }

                    Event::FeeEstimation(event) => {
                        self.log_debug(event);
                    }
                    Event::Empty(_) => self.log_debug("empty event not handled for Bitcoin"),

                    Event::HealthResult(_) => self.log_debug("ignoring health result in swapd"),
                };
            }
            _ => {}
        }
        self.execute_state_machine(endpoints, BusMsg::Sync(request), source)?;

        Ok(())
    }
}

impl Runtime {
    fn execute_state_machine(
        &mut self,
        endpoints: &mut Endpoints,
        msg: BusMsg,
        source: ServiceId,
    ) -> Result<(), Error> {
        if let Some(ssm) = SwapStateMachineExecutor::execute(
            self,
            endpoints,
            source.clone(),
            msg.clone(),
            self.swap_state_machine.clone(),
        )? {
            self.swap_state_machine = ssm;
            // On SwapEnd, report immediately to ensure the progress message goes out before the swap is terminated, then let farcasterd know of the outcome.
            if let SwapStateMachine::SwapEnd(outcome) = &self.swap_state_machine {
                let outcome = outcome.clone(); // so we don't borrow self anymore
                self.abort_all_syncer_tasks(endpoints)?;
                self.report_potential_state_change(endpoints)?;
                self.send_ctl(
                    endpoints,
                    ServiceId::Farcasterd,
                    BusMsg::Ctl(CtlMsg::SwapOutcome(outcome)),
                )?;
            }
            if matches!(self.swap_state_machine, SwapStateMachine::SwapEnd(_)) {
                self.report_potential_state_change(endpoints)?;
                return Ok(());
            }
            // Unset previously set unhandled peer message
            if let BusMsg::P2p(peer_msg) = msg {
                if Some(peer_msg.type_id())
                    == self.unhandled_peer_message.as_ref().map(|p| p.type_id())
                {
                    self.unhandled_peer_message = None;
                }
            }
            // Try to handle previously unhandled peer message
            if let Some(peer_msg) = self.unhandled_peer_message.clone() {
                self.handle_msg(endpoints, source.clone(), peer_msg)?;
            }
            // Replay syncer events to ensure we immediately advance through states that can be skipped
            for event in self.syncer_state.last_tx_event.clone().values() {
                self.handle_sync(endpoints, source.clone(), event.clone())?;
            }
        } else if let BusMsg::P2p(peer_msg) = msg {
            self.unhandled_peer_message = Some(peer_msg);
        }
        Ok(())
    }

    fn report_potential_state_change(&mut self, endpoints: &mut Endpoints) -> Result<(), Error> {
        // Generate a new state report for the clients
        let new_state_report = StateReport::new(
            self.swap_state_machine.to_string(),
            &self.temporal_safety,
            &self.syncer_state,
        );
        if self.latest_state_report != new_state_report {
            let progress = self
                .latest_state_report
                .generate_progress_update_or_transition(&new_state_report);
            self.latest_state_report = new_state_report;
            self.report_progress(endpoints, progress)?;
        }
        Ok(())
    }

    pub fn checkpoint_state(
        &mut self,
        endpoints: &mut Endpoints,
        pending_msg: Option<PeerMsg>,
        next_state: SwapStateMachine,
    ) -> Result<(), Error> {
        endpoints.send_to(
            ServiceBus::Ctl,
            self.identity(),
            ServiceId::Database,
            BusMsg::Ctl(CtlMsg::Checkpoint(Checkpoint {
                swap_id: self.swap_id,
                state: CheckpointSwapd {
                    state: next_state,
                    pending_msg,
                    enquirer: self.enquirer.clone(),
                    temporal_safety: self.temporal_safety.clone(),
                    txids: self.syncer_state.tasks.txids.clone().drain().collect(),
                    pending_broadcasts: self.syncer_state.pending_broadcast_txs(),
                    xmr_addr_addendum: self.syncer_state.xmr_addr_addendum.clone(),
                    local_trade_role: self.local_trade_role,
                    connected_counterparty_node_id: self.peer_service.node_id(),
                    deal: self.deal.clone(),
                },
            })),
        )?;
        Ok(())
    }

    pub fn abort_all_syncer_tasks(&mut self, endpoints: &mut Endpoints) -> Result<(), Error> {
        let abort_all = Task::Abort(Abort {
            task_target: TaskTarget::AllTasks,
            respond: false,
        });

        endpoints.send_to(
            ServiceBus::Sync,
            self.identity(),
            self.syncer_state.monero_syncer(),
            BusMsg::Sync(SyncMsg::Task(abort_all.clone())),
        )?;
        endpoints.send_to(
            ServiceBus::Sync,
            self.identity(),
            self.syncer_state.bitcoin_syncer(),
            BusMsg::Sync(SyncMsg::Task(abort_all)),
        )?;
        Ok(())
    }

    pub fn log_monero_maturity(&self, address: monero::Address) {
        let acc_confs_needs = self
            .syncer_state
            .get_confs(TxLabel::AccLock)
            .map(|confs| SWEEP_MONERO_THRESHOLD.saturating_sub(confs))
            .unwrap_or(SWEEP_MONERO_THRESHOLD);
        let sweep_block = self.syncer_state.height(Blockchain::Monero) + acc_confs_needs as u64;
        self.log_info(format!(
            "Tx {} needs {} more confirmations to spending maturity, and has {} confirmations.\n\
                {} reaches your address {} after block {}",
            TxLabel::AccLock.label(),
            acc_confs_needs.bright_green_bold(),
            self.syncer_state.get_confs(TxLabel::AccLock).unwrap_or(0),
            Blockchain::Monero.label(),
            address.addr(),
            sweep_block.bright_blue_bold(),
        ));
    }

    pub fn report_progress_message_log_fail(
        &mut self,
        endpoints: &mut Endpoints,
        msg: impl ToString,
    ) {
        if let Err(err) = self.report_progress_message(endpoints, msg) {
            self.log_error(format!("Error sending progress message: {}", err))
        }
    }
}

pub trait SwapLogging {
    fn swap_info(&self) -> (SwapId, SwapRole, TradeRole);

    fn log_info(&self, msg: impl std::fmt::Display) {
        info!("{} | {}", self.log_prefix(), msg);
    }

    fn log_error(&self, msg: impl std::fmt::Display) {
        error!("{} | {}", self.log_prefix(), msg);
    }

    fn log_debug(&self, msg: impl std::fmt::Display) {
        debug!("{} | {}", self.log_prefix(), msg);
    }

    fn log_trace(&self, msg: impl std::fmt::Display) {
        trace!("{} | {}", self.log_prefix(), msg);
    }

    fn log_warn(&self, msg: impl std::fmt::Display) {
        warn!("{} | {}", self.log_prefix(), msg);
    }

    fn log_prefix(&self) -> ColoredString {
        let (swap_id, swap_role, trade_role) = self.swap_info();
        format!("{} as {} {}", swap_id, swap_role, trade_role,).bright_blue_italic()
    }
}
