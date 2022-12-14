// Copyright 2020-2022 Farcaster Devs & LNP/BP Standards Association
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::bus::bridge::BridgeMsg;
use crate::bus::ctl::FundingInfo;
use crate::bus::ctl::ProtoPublicOffer;
use crate::bus::ctl::PubOffer;
use crate::bus::info::Address;
use crate::bus::info::OfferStatusSelector;
use crate::bus::info::ProgressEvent;
use crate::bus::AddressSecretKey;
use crate::bus::Failure;
use crate::bus::HealthCheckSelector;
use crate::bus::OptionDetails;
use crate::bus::Outcome;
use crate::service::Endpoints;
use crate::swapd::StateReport;
use crate::syncerd::SweepAddressAddendum;
use crate::syncerd::SweepBitcoinAddress;
use crate::syncerd::SweepMoneroAddress;
use farcaster_core::bitcoin::fee::SatPerVByte;
use farcaster_core::bitcoin::timelock::CSVTimelock;
use farcaster_core::blockchain::Blockchain;
use farcaster_core::blockchain::FeeStrategy;
use farcaster_core::blockchain::Network;
use farcaster_core::role::SwapRole;
use farcaster_core::role::TradeRole;
use farcaster_core::swap::btcxmr::Offer;
use farcaster_core::swap::{btcxmr::PublicOffer, SwapId};
use internet2::addr::InetSocketAddr;
use internet2::session::LocalSession;
use internet2::SendRecvMessage;
use std::collections::HashMap;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::Arc;
use tokio::runtime::Builder;
use tokio::sync::oneshot::error::RecvError;
use tokio::sync::Mutex;
use tonic_web::GrpcWebLayer;
use tower_http::cors::AllowOrigin;
use tower_http::cors::CorsLayer;
use tower_layer::Layer;
use uuid::Uuid;

use crate::bus::{ctl::CtlMsg, info::InfoMsg, info::SwapInfo};
use crate::bus::{BusMsg, ServiceBus};
use crate::{CtlServer, Error, Service, ServiceConfig, ServiceId};
use internet2::{zeromq::ZmqSocketType, TypedEnum};
use microservices::esb;
use microservices::ZMQ_CONTEXT;

use farcaster::farcaster_server::{Farcaster, FarcasterServer};
use tonic::{transport::Server, Request as GrpcRequest, Response as GrpcResponse, Status};

use self::farcaster::*;

pub mod farcaster {
    tonic::include_proto!("farcaster");
}

impl From<TradeRole> for farcaster::TradeRole {
    fn from(t: TradeRole) -> farcaster::TradeRole {
        match t {
            TradeRole::Maker => farcaster::TradeRole::Maker,
            TradeRole::Taker => farcaster::TradeRole::Taker,
        }
    }
}

impl From<farcaster::SwapRole> for SwapRole {
    fn from(t: farcaster::SwapRole) -> SwapRole {
        match t {
            farcaster::SwapRole::Alice => SwapRole::Alice,
            farcaster::SwapRole::Bob => SwapRole::Bob,
        }
    }
}

impl From<SwapRole> for farcaster::SwapRole {
    fn from(t: SwapRole) -> farcaster::SwapRole {
        match t {
            SwapRole::Alice => farcaster::SwapRole::Alice,
            SwapRole::Bob => farcaster::SwapRole::Bob,
        }
    }
}

impl From<farcaster::Network> for Network {
    fn from(t: farcaster::Network) -> Network {
        match t {
            farcaster::Network::Mainnet => Network::Mainnet,
            farcaster::Network::Testnet => Network::Testnet,
            farcaster::Network::Local => Network::Local,
        }
    }
}

impl From<Network> for farcaster::Network {
    fn from(t: Network) -> farcaster::Network {
        match t {
            Network::Mainnet => farcaster::Network::Mainnet,
            Network::Testnet => farcaster::Network::Testnet,
            Network::Local => farcaster::Network::Local,
        }
    }
}

impl From<farcaster::Blockchain> for Blockchain {
    fn from(t: farcaster::Blockchain) -> Blockchain {
        match t {
            farcaster::Blockchain::Monero => Blockchain::Monero,
            farcaster::Blockchain::Bitcoin => Blockchain::Bitcoin,
        }
    }
}

impl From<Blockchain> for farcaster::Blockchain {
    fn from(t: Blockchain) -> farcaster::Blockchain {
        match t {
            Blockchain::Monero => farcaster::Blockchain::Monero,
            Blockchain::Bitcoin => farcaster::Blockchain::Bitcoin,
        }
    }
}

impl From<farcaster::OfferSelector> for OfferStatusSelector {
    fn from(t: farcaster::OfferSelector) -> OfferStatusSelector {
        match t {
            farcaster::OfferSelector::All => OfferStatusSelector::All,
            farcaster::OfferSelector::Open => OfferStatusSelector::Open,
            farcaster::OfferSelector::InProgress => OfferStatusSelector::InProgress,
            farcaster::OfferSelector::Ended => OfferStatusSelector::Ended,
        }
    }
}

impl From<Outcome> for farcaster::Outcome {
    fn from(t: Outcome) -> farcaster::Outcome {
        match t {
            Outcome::SuccessSwap => farcaster::Outcome::SuccessSwap,
            Outcome::FailureRefund => farcaster::Outcome::FailureRefund,
            Outcome::FailurePunish => farcaster::Outcome::FailurePunish,
            Outcome::FailureAbort => farcaster::Outcome::FailureAbort,
        }
    }
}

impl From<StateReport> for farcaster::State {
    fn from(state_report: StateReport) -> farcaster::State {
        match state_report.clone() {
            StateReport::StartA => farcaster::State {
                state: Some(farcaster::state::State::StartA(StartA {})),
            },
            StateReport::CommitA => farcaster::State {
                state: Some(farcaster::state::State::CommitA(CommitA {})),
            },
            StateReport::RevealA => farcaster::State {
                state: Some(farcaster::state::State::RevealA(RevealA {})),
            },
            StateReport::RefundSigA {
                arb_block_height,
                acc_block_height,
                arb_locked,
                acc_locked,
                buy_published,
                cancel_seen,
                refund_seen,
                overfunded,
                arb_lock_confirmations,
                acc_lock_confirmations,
                blocks_until_cancel_possible,
                cancel_confirmations,
                blocks_until_punish_possible,
                blocks_until_safe_buy,
            } => farcaster::State {
                state: Some(farcaster::state::State::RefundSigA(RefundSigA {
                    arb_block_height,
                    acc_block_height,
                    arb_locked,
                    acc_locked,
                    buy_published,
                    cancel_seen,
                    refund_seen,
                    overfunded,
                    arb_lock_confirmations: arb_lock_confirmations
                        .map(|c| farcaster::refund_sig_a::ArbLockConfirmations::ArbConfs(c)),
                    acc_lock_confirmations: acc_lock_confirmations
                        .map(|c| farcaster::refund_sig_a::AccLockConfirmations::AccConfs(c)),
                    blocks_until_cancel_possible: blocks_until_cancel_possible.map(|b| {
                        farcaster::refund_sig_a::BlocksUntilCancelPossible::CancelBlocks(b)
                    }),
                    cancel_confirmations: cancel_confirmations
                        .map(|c| farcaster::refund_sig_a::CancelConfirmations::CancelConfs(c)),
                    blocks_until_punish_possible: blocks_until_punish_possible.map(|b| {
                        farcaster::refund_sig_a::BlocksUntilPunishPossible::PunishBlocks(b)
                    }),
                    blocks_until_safe_buy: blocks_until_safe_buy
                        .map(|b| farcaster::refund_sig_a::BlocksUntilSafeBuy::BuyBlocks(b)),
                })),
            },
            StateReport::FinishA(outcome) => farcaster::State {
                state: Some(farcaster::state::State::FinishA(FinishA {
                    outcome: farcaster::Outcome::from(outcome).into(),
                })),
            },
            StateReport::StartB => farcaster::State {
                state: Some(farcaster::state::State::StartB(StartB {})),
            },
            StateReport::CommitB => farcaster::State {
                state: Some(farcaster::state::State::CommitB(CommitB {})),
            },
            StateReport::RevealB => farcaster::State {
                state: Some(farcaster::state::State::RevealB(RevealB {})),
            },
            StateReport::CoreArbB {
                arb_block_height,
                acc_block_height,
                arb_locked,
                acc_locked,
                buy_published,
                refund_seen,
                arb_lock_confirmations,
                acc_lock_confirmations,
                blocks_until_cancel_possible,
                cancel_confirmations,
                blocks_until_refund,
                blocks_until_punish_possible,
            } => farcaster::State {
                state: Some(farcaster::state::State::CoreArbB(CoreArbB {
                    arb_block_height,
                    acc_block_height,
                    arb_locked,
                    acc_locked,
                    buy_published,
                    refund_seen,
                    arb_lock_confirmations: arb_lock_confirmations
                        .map(|c| farcaster::core_arb_b::ArbLockConfirmations::ArbConfs(c)),
                    acc_lock_confirmations: acc_lock_confirmations
                        .map(|c| farcaster::core_arb_b::AccLockConfirmations::AccConfs(c)),
                    blocks_until_cancel_possible: blocks_until_cancel_possible
                        .map(|b| farcaster::core_arb_b::BlocksUntilCancelPossible::CancelBlocks(b)),
                    cancel_confirmations: cancel_confirmations
                        .map(|c| farcaster::core_arb_b::CancelConfirmations::CancelConfs(c)),
                    blocks_until_refund: blocks_until_refund
                        .map(|b| farcaster::core_arb_b::BlocksUntilRefund::RefundBlocks(b)),
                    blocks_until_punish_possible: blocks_until_punish_possible
                        .map(|b| farcaster::core_arb_b::BlocksUntilPunishPossible::PunishBlocks(b)),
                })),
            },
            StateReport::BuySigB {
                arb_block_height,
                acc_block_height,
                buy_tx_seen,
                arb_lock_confirmations,
                acc_lock_confirmations,
                blocks_until_cancel_possible,
                cancel_confirmations,
                blocks_until_refund,
                blocks_until_punish_possible,
                blocks_until_safe_monero_buy_sweep,
            } => farcaster::State {
                state: Some(farcaster::state::State::BuySigB(BuySigB {
                    arb_block_height,
                    acc_block_height,
                    buy_tx_seen,
                    arb_lock_confirmations: arb_lock_confirmations
                        .map(|c| farcaster::buy_sig_b::ArbLockConfirmations::ArbConfs(c)),
                    acc_lock_confirmations: acc_lock_confirmations
                        .map(|c| farcaster::buy_sig_b::AccLockConfirmations::AccConfs(c)),
                    blocks_until_cancel_possible: blocks_until_cancel_possible
                        .map(|b| farcaster::buy_sig_b::BlocksUntilCancelPossible::CancelBlocks(b)),
                    cancel_confirmations: cancel_confirmations
                        .map(|c| farcaster::buy_sig_b::CancelConfirmations::CancelConfs(c)),
                    blocks_until_refund: blocks_until_refund
                        .map(|b| farcaster::buy_sig_b::BlocksUntilRefund::RefundBlocks(b)),
                    blocks_until_punish_possible: blocks_until_punish_possible
                        .map(|b| farcaster::buy_sig_b::BlocksUntilPunishPossible::PunishBlocks(b)),
                    blocks_until_safe_monero_buy_sweep: blocks_until_safe_monero_buy_sweep.map(
                        |b| farcaster::buy_sig_b::BlocksUntilSafeMoneroBuySweep::BuyMoneroBlocks(b),
                    ),
                })),
            },
            StateReport::FinishB(outcome) => farcaster::State {
                state: Some(farcaster::state::State::FinishB(FinishB {
                    outcome: farcaster::Outcome::from(outcome).into(),
                })),
            },
        }
    }
}

impl From<farcaster::HealthCheckSelector> for HealthCheckSelector {
    fn from(s: farcaster::HealthCheckSelector) -> Self {
        match s {
            farcaster::HealthCheckSelector::CheckAll => HealthCheckSelector::All,
            farcaster::HealthCheckSelector::CheckLocal => {
                HealthCheckSelector::Network(Network::Local)
            }
            farcaster::HealthCheckSelector::CheckMainnet => {
                HealthCheckSelector::Network(Network::Mainnet)
            }
            farcaster::HealthCheckSelector::CheckTestnet => {
                HealthCheckSelector::Network(Network::Testnet)
            }
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq, PartialOrd, Hash, Display)]
#[display(Debug)]
pub struct IdCounter(u64);

impl IdCounter {
    fn increment(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
}

pub struct FarcasterService {
    tokio_tx_request: tokio::sync::mpsc::Sender<(u64, BusMsg)>,
    pending_requests: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<BusMsg>>>>,
    id_counter: Arc<Mutex<IdCounter>>,
}

impl FarcasterService {
    async fn process_request(
        &self,
        msg: BusMsg,
    ) -> Result<tokio::sync::oneshot::Receiver<BusMsg>, Status> {
        let mut id_counter = self.id_counter.lock().await;
        let id = id_counter.increment();
        drop(id_counter);

        if let Err(error) = self.tokio_tx_request.send((id, msg)).await {
            return Err(Status::internal(error.to_string()));
        }

        let (oneshot_tx, oneshot_rx) = tokio::sync::oneshot::channel::<BusMsg>();
        let mut pending_requests = self.pending_requests.lock().await;
        pending_requests.insert(id, oneshot_tx);
        drop(pending_requests);
        Ok(oneshot_rx)
    }
}

fn process_error_response<T>(msg: Result<BusMsg, RecvError>) -> Result<GrpcResponse<T>, Status> {
    match msg {
        Err(error) => Err(Status::internal(error.to_string())),
        Ok(BusMsg::Ctl(CtlMsg::Failure(Failure { info, .. }))) => Err(Status::internal(info)),
        Ok(BusMsg::Info(InfoMsg::Failure(Failure { info, .. }))) => Err(Status::internal(info)),
        _ => Err(Status::internal("received unexpected internal response")),
    }
}

#[tonic::async_trait]
impl Farcaster for FarcasterService {
    async fn info(
        &self,
        request: GrpcRequest<InfoRequest>,
    ) -> Result<GrpcResponse<InfoResponse>, Status> {
        debug!("Received a grpc info request: {:?}", request);
        let oneshot_rx = self
            .process_request(BusMsg::Bridge(BridgeMsg::Info {
                request: InfoMsg::GetInfo,
                service_id: ServiceId::Farcasterd,
            }))
            .await?;
        match oneshot_rx.await {
            Ok(BusMsg::Info(InfoMsg::NodeInfo(info))) => {
                let reply = farcaster::InfoResponse {
                    id: request.into_inner().id,
                    listens: info
                        .listens
                        .iter()
                        .map(|listen| format!("{}", listen))
                        .collect(),
                    uptime: info.uptime.as_secs(),
                    since: info.since,
                    peers: info.peers.iter().map(|peer| format!("{}", peer)).collect(),
                    swaps: info
                        .swaps
                        .iter()
                        .map(|swap| format!("{:#x}", swap))
                        .collect(),
                    offers: info
                        .offers
                        .iter()
                        .map(|offer| format!("{}", offer))
                        .collect(),
                };
                Ok(GrpcResponse::new(reply))
            }
            res => process_error_response(res),
        }
    }

    async fn peers(
        &self,
        request: GrpcRequest<PeersRequest>,
    ) -> Result<GrpcResponse<PeersResponse>, Status> {
        debug!("Received a grpc peers request: {:?}", request);
        let oneshot_rx = self
            .process_request(BusMsg::Bridge(BridgeMsg::Info {
                request: InfoMsg::ListPeers,
                service_id: ServiceId::Farcasterd,
            }))
            .await?;
        match oneshot_rx.await {
            Ok(BusMsg::Info(InfoMsg::PeerList(peers))) => {
                let reply = farcaster::PeersResponse {
                    id: request.into_inner().id,
                    peers: peers.iter().map(|peer| format!("{}", peer)).collect(),
                };
                Ok(GrpcResponse::new(reply))
            }
            res => process_error_response(res),
        }
    }

    async fn swap_info(
        &self,
        request: GrpcRequest<SwapInfoRequest>,
    ) -> Result<GrpcResponse<SwapInfoResponse>, Status> {
        debug!("Received a grpc swap info request: {:?}", request);
        let SwapInfoRequest {
            id,
            swap_id: string_swap_id,
        } = request.into_inner();
        let swap_id = SwapId::from_str(&string_swap_id)
            .map_err(|_| Status::invalid_argument(format!("Invalid or malformed swap id")))?;
        let oneshot_rx = self
            .process_request(BusMsg::Bridge(BridgeMsg::Info {
                request: InfoMsg::GetInfo,
                service_id: ServiceId::Swap(swap_id),
            }))
            .await?;
        match oneshot_rx.await {
            Ok(BusMsg::Info(InfoMsg::SwapInfo(SwapInfo {
                swap_id: _,
                connection,
                connected,
                state,
                uptime,
                since,
                public_offer,
                local_trade_role,
                local_swap_role,
                connected_counterparty_node_id,
            }))) => {
                let reply = SwapInfoResponse {
                    id,
                    connection: connection.map(|p| p.to_string()).unwrap_or("".to_string()),
                    connected,
                    uptime: uptime.as_secs(),
                    since: since,
                    public_offer: public_offer.to_string(),
                    trade_role: farcaster::TradeRole::from(local_trade_role).into(),
                    swap_role: farcaster::SwapRole::from(local_swap_role).into(),
                    connected_counterparty_node_id: connected_counterparty_node_id
                        .map(|n| n.to_string())
                        .unwrap_or("".to_string()),
                    state: state.to_string(),
                };
                Ok(GrpcResponse::new(reply))
            }
            res => process_error_response(res),
        }
    }

    async fn list_offers(
        &self,
        request: GrpcRequest<ListOffersRequest>,
    ) -> Result<GrpcResponse<ListOffersResponse>, Status> {
        debug!("Received a grpc request: {:?}", request);
        let ListOffersRequest {
            id,
            selector: grpc_offer_selector,
        } = request.into_inner();
        let selector = farcaster::OfferSelector::from_i32(grpc_offer_selector)
            .ok_or(Status::invalid_argument("selector"))?
            .into();
        let oneshot_rx = self
            .process_request(BusMsg::Bridge(BridgeMsg::Info {
                request: InfoMsg::ListOffers(selector),
                service_id: ServiceId::Farcasterd,
            }))
            .await?;
        match oneshot_rx.await {
            Ok(BusMsg::Info(InfoMsg::OfferList(mut offers))) => {
                let reply = ListOffersResponse {
                    id,
                    public_offers: offers.drain(..).map(|o| o.offer).collect(),
                };
                Ok(GrpcResponse::new(reply))
            }
            Ok(BusMsg::Info(InfoMsg::OfferStatusList(mut offers))) => {
                let reply = ListOffersResponse {
                    id,
                    public_offers: offers.drain(..).map(|o| o.offer.to_string()).collect(),
                };
                Ok(GrpcResponse::new(reply))
            }
            Err(error) => Err(Status::internal(format!("{}", error))),
            Ok(BusMsg::Ctl(CtlMsg::Failure(Failure { info, .. }))) => Err(Status::internal(info)),
            _ => Err(Status::invalid_argument("received invalid response")),
        }
    }

    async fn offer_info(
        &self,
        request: GrpcRequest<OfferInfoRequest>,
    ) -> Result<GrpcResponse<OfferInfoResponse>, Status> {
        debug!("Received a grpc offer info request: {:?}", request);
        let OfferInfoRequest {
            id,
            public_offer: string_public_offer,
        } = request.into_inner();
        let public_offer = PublicOffer::from_str(&string_public_offer)
            .map_err(|_| Status::invalid_argument("public offer malformed"))?;

        let reply = OfferInfoResponse {
            id,
            arbitrating_amount: public_offer.offer.arbitrating_amount.as_sat(),
            accordant_amount: public_offer.offer.accordant_amount.as_pico(),
            cancel_timelock: public_offer.offer.cancel_timelock.as_u32(),
            punish_timelock: public_offer.offer.punish_timelock.as_u32(),
            fee_strategy: public_offer.offer.fee_strategy.to_string(),
            maker_role: farcaster::SwapRole::from(public_offer.offer.maker_role).into(),
            uuid: public_offer.offer.uuid.to_string(),
            network: farcaster::Network::from(public_offer.offer.network).into(),
            arbitrating_blockchain: farcaster::Blockchain::from(
                public_offer.offer.arbitrating_blockchain,
            )
            .into(),
            accordant_blockchain: farcaster::Blockchain::from(
                public_offer.offer.accordant_blockchain,
            )
            .into(),
            node_id: public_offer.node_id.to_string(),
            peer_address: public_offer.peer_address.to_string(),
        };
        Ok(GrpcResponse::new(reply))
    }

    async fn checkpoints(
        &self,
        request: GrpcRequest<CheckpointsRequest>,
    ) -> Result<GrpcResponse<CheckpointsResponse>, Status> {
        debug!("Received a grpc checkpoints request: {:?}", request);
        let CheckpointsRequest { id, selector } = request.into_inner();

        let selector = farcaster::CheckpointSelector::from_i32(selector)
            .ok_or(Status::invalid_argument("selector"))?
            .into();

        let oneshot_rx = match selector {
            farcaster::CheckpointSelector::AllCheckpoints => {
                self.process_request(BusMsg::Bridge(BridgeMsg::Info {
                    request: InfoMsg::RetrieveAllCheckpointInfo,
                    service_id: ServiceId::Database,
                }))
                .await?
            }
            farcaster::CheckpointSelector::AvailableForRestore => {
                let oneshot_rx = self
                    .process_request(BusMsg::Bridge(BridgeMsg::Info {
                        request: InfoMsg::RetrieveAllCheckpointInfo,
                        service_id: ServiceId::Database,
                    }))
                    .await?;
                match oneshot_rx.await {
                    Ok(BusMsg::Info(InfoMsg::CheckpointList(checkpoint_entries))) => {
                        self.process_request(BusMsg::Bridge(BridgeMsg::Info {
                            request: InfoMsg::CheckpointList(checkpoint_entries),
                            service_id: ServiceId::Farcasterd,
                        }))
                        .await?
                    }
                    Err(error) => {
                        return Err(Status::internal(format!("{}", error)));
                    }
                    Ok(BusMsg::Ctl(CtlMsg::Failure(Failure { info, .. }))) => {
                        return Err(Status::internal(info));
                    }
                    _ => {
                        return Err(Status::invalid_argument("received invalid response"));
                    }
                }
            }
        };

        match oneshot_rx.await {
            Ok(BusMsg::Info(InfoMsg::CheckpointList(checkpoint_entries))) => {
                let reply = farcaster::CheckpointsResponse {
                    id,
                    checkpoint_entries: checkpoint_entries
                        .iter()
                        .map(|entry| farcaster::CheckpointEntry {
                            swap_id: format!("{:#x}", entry.swap_id),
                            public_offer: format!("{}", entry.public_offer),
                            trade_role: farcaster::TradeRole::from(entry.trade_role) as i32,
                        })
                        .collect(),
                };
                Ok(GrpcResponse::new(reply))
            }
            res => process_error_response(res),
        }
    }

    async fn restore_checkpoint(
        &self,
        request: GrpcRequest<RestoreCheckpointRequest>,
    ) -> Result<GrpcResponse<RestoreCheckpointResponse>, Status> {
        debug!("Received a grpc restore checkpoints request: {:?}", request);
        let RestoreCheckpointRequest {
            id,
            swap_id: string_swap_id,
        } = request.into_inner();
        let swap_id = match SwapId::from_str(&string_swap_id) {
            Ok(swap_id) => swap_id,
            Err(_) => {
                return Err(Status::invalid_argument(format!("Invalid swap id")));
            }
        };
        let oneshot_rx = self
            .process_request(BusMsg::Bridge(BridgeMsg::Info {
                request: InfoMsg::GetCheckpointEntry(swap_id),
                service_id: ServiceId::Database,
            }))
            .await?;
        match oneshot_rx.await {
            Ok(BusMsg::Info(InfoMsg::CheckpointEntry(checkpoint_entry))) => {
                let oneshot_rx = self
                    .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                        request: CtlMsg::RestoreCheckpoint(checkpoint_entry),
                        service_id: ServiceId::Farcasterd,
                    }))
                    .await?;
                match oneshot_rx.await {
                    Ok(BusMsg::Info(InfoMsg::String(message))) => {
                        let reply = farcaster::RestoreCheckpointResponse {
                            id,
                            status: message,
                        };
                        Ok(GrpcResponse::new(reply))
                    }
                    res => process_error_response(res),
                }
            }
            res => process_error_response(res),
        }
    }

    async fn funding_addresses(
        &self,
        request: GrpcRequest<FundingAddressesRequest>,
    ) -> Result<GrpcResponse<FundingAddressesResponse>, Status> {
        let FundingAddressesRequest {
            id,
            blockchain: grpc_blockchain,
        } = request.into_inner();

        let blockchain: Blockchain = farcaster::Blockchain::from_i32(grpc_blockchain)
            .ok_or(Status::invalid_argument("arbitrating blockchain"))?
            .into();

        let oneshot_rx = self
            .process_request(BusMsg::Bridge(BridgeMsg::Info {
                request: InfoMsg::GetAddresses(blockchain),
                service_id: ServiceId::Database,
            }))
            .await?;
        match oneshot_rx.await {
            Ok(BusMsg::Info(InfoMsg::BitcoinAddressList(addresses))) => {
                let reply = FundingAddressesResponse {
                    id,
                    addresses: addresses
                        .iter()
                        .map(|a| AddressSwapIdPair {
                            address: a.address.to_string(),
                            address_swap_id: a.swap_id.map(|c| {
                                farcaster::address_swap_id_pair::AddressSwapId::SwapId(
                                    c.to_string(),
                                )
                            }),
                        })
                        .collect(),
                };
                Ok(GrpcResponse::new(reply))
            }
            Ok(BusMsg::Ctl(CtlMsg::Failure(Failure { info, .. }))) => Err(Status::internal(info)),
            _ => Err(Status::internal(format!("Received invalid response"))),
        }
    }

    async fn make(
        &self,
        request: GrpcRequest<MakeRequest>,
    ) -> Result<GrpcResponse<MakeResponse>, Status> {
        debug!("Received a grpc make request: {:?}", request);
        let MakeRequest {
            id,
            network: grpc_network,
            arbitrating_blockchain: grpc_arb_blockchain,
            accordant_blockchain: grpc_acc_blockchain,
            arbitrating_amount: int_arb_amount,
            accordant_amount: int_acc_amount,
            arbitrating_addr: str_arb_addr,
            accordant_addr: str_acc_addr,
            cancel_timelock: int_cancel_timelock,
            punish_timelock: int_punish_timelock,
            fee_strategy: str_fee_strategy,
            maker_role: grpc_swap_role,
            public_ip_addr: str_public_ip_addr,
            public_port,
        } = request.into_inner();

        let network: Network = farcaster::Network::from_i32(grpc_network)
            .ok_or(Status::invalid_argument("network"))?
            .into();
        let arbitrating_blockchain: Blockchain =
            farcaster::Blockchain::from_i32(grpc_arb_blockchain)
                .ok_or(Status::invalid_argument("arbitrating blockchain"))?
                .into();
        let accordant_blockchain: Blockchain = farcaster::Blockchain::from_i32(grpc_acc_blockchain)
            .ok_or(Status::invalid_argument("accordant blockchain"))?
            .into();
        let arbitrating_amount = bitcoin::Amount::from_sat(int_arb_amount);
        let accordant_amount = monero::Amount::from_pico(int_acc_amount);
        let arbitrating_addr = bitcoin::Address::from_str(&str_arb_addr)
            .map_err(|_| Status::invalid_argument("arbitrating address"))?;
        let accordant_addr = monero::Address::from_str(&str_acc_addr)
            .map_err(|_| Status::invalid_argument("accordant_address"))?;
        let cancel_timelock = CSVTimelock::new(int_cancel_timelock);
        let punish_timelock = CSVTimelock::new(int_punish_timelock);
        let maker_role: SwapRole = farcaster::SwapRole::from_i32(grpc_swap_role)
            .ok_or(Status::invalid_argument("maker role"))?
            .into();
        let public_ip_addr = IpAddr::from_str(&str_public_ip_addr)
            .map_err(|_| Status::invalid_argument("public ip address"))?;
        let fee_strategy: FeeStrategy<SatPerVByte> = FeeStrategy::from_str(&str_fee_strategy).map_err(|_| Status::invalid_argument("
        fee strategy is required to be formated as a fixed value, e.g. \"100 satoshi/vByte\" or a range, e.g. \"50 satoshi/vByte-150 satoshi/vByte\" "))?;

        // Monero local address types are mainnet address types
        if network != accordant_addr.network.into() && network != Network::Local {
            return Err(Status::invalid_argument(format!(
                "Error: The address {} is not for {}",
                accordant_addr, network
            )));
        }
        if network != arbitrating_addr.network.into() {
            return Err(Status::invalid_argument(format!(
                "Error: The address {} is not for {}",
                arbitrating_addr, network
            )));
        }
        if arbitrating_amount > bitcoin::Amount::from_str("0.01 BTC").unwrap()
            && network == Network::Mainnet
        {
            return Err(Status::invalid_argument(format!(
                "Error: Bitcoin amount {} too high, mainnet amount capped at 0.01 BTC.",
                arbitrating_amount
            )));
        }
        if accordant_amount > monero::Amount::from_str("2 XMR").unwrap()
            && network == Network::Mainnet
        {
            return Err(Status::invalid_argument(format!(
                "Error: Monero amount {} too high, mainnet amount capped at 2 XMR.",
                accordant_amount
            )));
        }
        if accordant_amount < monero::Amount::from_str("0.001 XMR").unwrap() {
            return Err(Status::invalid_argument(format!(
                "Error: Monero amount {} too low, require at least 0.001 XMR",
                accordant_amount
            )));
        }
        let offer = Offer {
            uuid: Uuid::new_v4(),
            network,
            arbitrating_blockchain,
            accordant_blockchain,
            arbitrating_amount,
            accordant_amount,
            cancel_timelock,
            punish_timelock,
            fee_strategy,
            maker_role,
        };
        let public_addr = InetSocketAddr::socket(public_ip_addr, public_port as u16);
        let proto_offer = ProtoPublicOffer {
            offer,
            public_addr,
            arbitrating_addr,
            accordant_addr,
        };

        let oneshot_rx = self
            .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                request: CtlMsg::MakeOffer(proto_offer),
                service_id: ServiceId::Farcasterd,
            }))
            .await?;
        match oneshot_rx.await {
            Ok(BusMsg::Info(InfoMsg::MadeOffer(made_offer))) => {
                let reply = farcaster::MakeResponse {
                    id,
                    offer: made_offer.offer_info.offer,
                };
                Ok(GrpcResponse::new(reply))
            }
            res => process_error_response(res),
        }
    }

    async fn revoke_offer(
        &self,
        request: GrpcRequest<RevokeOfferRequest>,
    ) -> Result<GrpcResponse<RevokeOfferResponse>, Status> {
        debug!("Received a grpc revoke offer request: {:?}", request);
        let RevokeOfferRequest {
            id,
            public_offer: str_public_offer,
        } = request.into_inner();

        let public_offer = PublicOffer::from_str(&str_public_offer)
            .map_err(|_| Status::invalid_argument("public offer"))?;

        let oneshot_rx = self
            .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                request: CtlMsg::RevokeOffer(public_offer),
                service_id: ServiceId::Farcasterd,
            }))
            .await?;
        match oneshot_rx.await {
            Ok(BusMsg::Info(InfoMsg::String(_))) => {
                let reply = farcaster::RevokeOfferResponse { id };
                Ok(GrpcResponse::new(reply))
            }
            res => process_error_response(res),
        }
    }

    async fn abort_swap(
        &self,
        request: GrpcRequest<AbortSwapRequest>,
    ) -> Result<GrpcResponse<AbortSwapResponse>, Status> {
        debug!("Received a grpc abort swap request: {:?}", request);
        let AbortSwapRequest {
            id,
            swap_id: str_swap_id,
        } = request.into_inner();
        let swap_id =
            SwapId::from_str(&str_swap_id).map_err(|_| Status::invalid_argument("swap id"))?;

        let oneshot_rx = self
            .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                request: CtlMsg::AbortSwap,
                service_id: ServiceId::Swap(swap_id),
            }))
            .await?;
        match oneshot_rx.await {
            Ok(BusMsg::Info(InfoMsg::String(_))) => {
                let reply = farcaster::AbortSwapResponse { id };
                Ok(GrpcResponse::new(reply))
            }
            res => process_error_response(res),
        }
    }

    async fn progress(
        &self,
        request: GrpcRequest<ProgressRequest>,
    ) -> Result<GrpcResponse<ProgressResponse>, Status> {
        debug!("Received a grpc progress request: {:?}", request);
        let ProgressRequest {
            id,
            swap_id: str_swap_id,
        } = request.into_inner();
        let swap_id =
            SwapId::from_str(&str_swap_id).map_err(|_| Status::invalid_argument("swap id"))?;

        let oneshot_rx = self
            .process_request(BusMsg::Bridge(BridgeMsg::Info {
                request: InfoMsg::ReadProgress(swap_id),
                service_id: ServiceId::Farcasterd,
            }))
            .await?;

        match oneshot_rx.await {
            Ok(BusMsg::Info(InfoMsg::SwapProgress(mut progress))) => {
                let reply = ProgressResponse {
                    id,
                    progress: progress
                        .progress
                        .drain(..)
                        .map(|p| match p {
                            ProgressEvent::Message(m) => farcaster::Progress {
                                progress: Some(farcaster::progress::Progress::Message(
                                    m.to_string(),
                                )),
                            },
                            ProgressEvent::StateUpdate(su) => farcaster::Progress {
                                progress: Some(farcaster::progress::Progress::StateUpdate(
                                    su.into(),
                                )),
                            },
                            ProgressEvent::StateTransition(st) => farcaster::Progress {
                                progress: Some(farcaster::progress::Progress::StateTransition(
                                    farcaster::StateTransition {
                                        old_state: Some(st.old_state.into()),
                                        new_state: Some(st.new_state.into()),
                                    },
                                )),
                            },
                            ProgressEvent::Failure(Failure { info, .. }) => farcaster::Progress {
                                progress: Some(farcaster::progress::Progress::Failure(
                                    info.to_string(),
                                )),
                            },
                            ProgressEvent::Success(OptionDetails(s)) => farcaster::Progress {
                                progress: Some(farcaster::progress::Progress::Success(
                                    s.unwrap_or("".to_string()),
                                )),
                            },
                        })
                        .collect(),
                };

                Ok(GrpcResponse::new(reply))
            }
            res => process_error_response(res),
        }
    }

    async fn connect_swap(
        &self,
        request: GrpcRequest<ConnectSwapRequest>,
    ) -> Result<GrpcResponse<ConnectSwapResponse>, Status> {
        let ConnectSwapRequest {
            id,
            swap_id: str_swap_id,
        } = request.into_inner();

        let swap_id =
            SwapId::from_str(&str_swap_id).map_err(|_| Status::invalid_argument("swap id"))?;

        let oneshot_rx = self
            .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                request: CtlMsg::Connect(swap_id),
                service_id: ServiceId::Farcasterd,
            }))
            .await?;

        match oneshot_rx.await {
            Ok(BusMsg::Ctl(CtlMsg::ConnectSuccess)) => {
                let reply = ConnectSwapResponse { id };
                Ok(GrpcResponse::new(reply))
            }
            Ok(BusMsg::Ctl(CtlMsg::Failure(Failure { info, code: _ }))) => {
                Err(Status::internal(info))
            }
            _ => Err(Status::internal(format!("Received invalid response"))),
        }
    }

    async fn needs_funding(
        &self,
        request: GrpcRequest<NeedsFundingRequest>,
    ) -> Result<GrpcResponse<NeedsFundingResponse>, Status> {
        debug!("Received a grpc needs funding request: {:?}", request);
        let NeedsFundingRequest {
            id,
            blockchain: int_blockchain,
        } = request.into_inner();

        let blockchain: Blockchain = farcaster::Blockchain::from_i32(int_blockchain)
            .ok_or(Status::invalid_argument("blockchain"))?
            .into();

        let oneshot_rx = self
            .process_request(BusMsg::Bridge(BridgeMsg::Info {
                request: InfoMsg::NeedsFunding(blockchain),
                service_id: ServiceId::Farcasterd,
            }))
            .await?;

        match oneshot_rx.await {
            Ok(BusMsg::Info(InfoMsg::FundingInfos(infos))) => {
                let reply = NeedsFundingResponse {
                    id,
                    funding_infos: infos
                        .swaps_need_funding
                        .iter()
                        .map(|info| match info {
                            FundingInfo::Bitcoin(b_info) => farcaster::FundingInfo {
                                swap_id: b_info.swap_id.to_string(),
                                address: b_info.address.to_string(),
                                amount: b_info.amount.as_sat(),
                            },
                            FundingInfo::Monero(m_info) => farcaster::FundingInfo {
                                swap_id: m_info.swap_id.to_string(),
                                address: m_info.address.to_string(),
                                amount: m_info.amount.as_pico(),
                            },
                        })
                        .collect(),
                };
                Ok(GrpcResponse::new(reply))
            }
            res => process_error_response(res),
        }
    }

    async fn health_check(
        &self,
        request: GrpcRequest<HealthCheckRequest>,
    ) -> Result<GrpcResponse<HealthCheckResponse>, Status> {
        let HealthCheckRequest {
            id,
            selector: grpc_selector,
        } = request.into_inner();

        let selector: HealthCheckSelector = farcaster::HealthCheckSelector::from_i32(grpc_selector)
            .ok_or(Status::invalid_argument("selector"))?
            .into();

        match selector {
            HealthCheckSelector::Network(network) => {
                let oneshot_rx = self
                    .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                        request: CtlMsg::HealthCheck(Blockchain::Bitcoin, network),
                        service_id: ServiceId::Farcasterd,
                    }))
                    .await?;

                let bitcoin_health = match oneshot_rx.await {
                    Ok(BusMsg::Ctl(CtlMsg::HealthResult(health))) => health,
                    _ => {
                        return Err(Status::internal("Error during health check".to_string()));
                    }
                };

                let oneshot_rx = self
                    .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                        request: CtlMsg::HealthCheck(Blockchain::Monero, network),
                        service_id: ServiceId::Farcasterd,
                    }))
                    .await?;

                let monero_health = match oneshot_rx.await {
                    Ok(BusMsg::Ctl(CtlMsg::HealthResult(health))) => health,
                    _ => {
                        return Err(Status::internal("Error during health check".to_string()));
                    }
                };
                Ok(GrpcResponse::new(HealthCheckResponse {
                    id,
                    health_report: Some(
                        farcaster::health_check_response::HealthReport::ReducedHealthReport(
                            ReducedHealthReport {
                                bitcoin_health: bitcoin_health.to_string(),
                                monero_health: monero_health.to_string(),
                            },
                        ),
                    ),
                }))
            }
            HealthCheckSelector::All => {
                let oneshot_rx = self
                    .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                        request: CtlMsg::HealthCheck(Blockchain::Bitcoin, Network::Testnet),
                        service_id: ServiceId::Farcasterd,
                    }))
                    .await?;

                let bitcoin_testnet_health = match oneshot_rx.await {
                    Ok(BusMsg::Ctl(CtlMsg::HealthResult(health))) => health,
                    _ => {
                        return Err(Status::internal("Error during health check".to_string()));
                    }
                };

                let oneshot_rx = self
                    .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                        request: CtlMsg::HealthCheck(Blockchain::Bitcoin, Network::Mainnet),
                        service_id: ServiceId::Farcasterd,
                    }))
                    .await?;

                let bitcoin_mainnet_health = match oneshot_rx.await {
                    Ok(BusMsg::Ctl(CtlMsg::HealthResult(health))) => health,
                    _ => {
                        return Err(Status::internal("Error during health check".to_string()));
                    }
                };

                let oneshot_rx = self
                    .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                        request: CtlMsg::HealthCheck(Blockchain::Bitcoin, Network::Local),
                        service_id: ServiceId::Farcasterd,
                    }))
                    .await?;

                let bitcoin_local_health = match oneshot_rx.await {
                    Ok(BusMsg::Ctl(CtlMsg::HealthResult(health))) => health,
                    _ => {
                        return Err(Status::internal("Error during health check".to_string()));
                    }
                };

                let oneshot_rx = self
                    .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                        request: CtlMsg::HealthCheck(Blockchain::Monero, Network::Testnet),
                        service_id: ServiceId::Farcasterd,
                    }))
                    .await?;

                let monero_testnet_health = match oneshot_rx.await {
                    Ok(BusMsg::Ctl(CtlMsg::HealthResult(health))) => health,
                    _ => {
                        return Err(Status::internal("Error during health check".to_string()));
                    }
                };

                let oneshot_rx = self
                    .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                        request: CtlMsg::HealthCheck(Blockchain::Monero, Network::Mainnet),
                        service_id: ServiceId::Farcasterd,
                    }))
                    .await?;

                let monero_mainnet_health = match oneshot_rx.await {
                    Ok(BusMsg::Ctl(CtlMsg::HealthResult(health))) => health,
                    _ => {
                        return Err(Status::internal("Error during health check".to_string()));
                    }
                };

                let oneshot_rx = self
                    .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                        request: CtlMsg::HealthCheck(Blockchain::Monero, Network::Local),
                        service_id: ServiceId::Farcasterd,
                    }))
                    .await?;

                let monero_local_health = match oneshot_rx.await {
                    Ok(BusMsg::Ctl(CtlMsg::HealthResult(health))) => health,
                    _ => {
                        return Err(Status::internal("Error during health check".to_string()));
                    }
                };

                Ok(GrpcResponse::new(HealthCheckResponse {
                    id,
                    health_report: Some(
                        farcaster::health_check_response::HealthReport::CompleteHealthReport(
                            CompleteHealthReport {
                                bitcoin_mainnet_health: bitcoin_mainnet_health.to_string(),
                                bitcoin_testnet_health: bitcoin_testnet_health.to_string(),
                                bitcoin_local_health: bitcoin_local_health.to_string(),
                                monero_mainnet_health: monero_mainnet_health.to_string(),
                                monero_testnet_health: monero_testnet_health.to_string(),
                                monero_local_health: monero_local_health.to_string(),
                            },
                        ),
                    ),
                }))
            }
        }
    }

    async fn sweep_address(
        &self,
        request: GrpcRequest<SweepAddressRequest>,
    ) -> Result<GrpcResponse<SweepAddressResponse>, Status> {
        debug!("Received a grpc sweep address request: {:?}", request);
        let SweepAddressRequest {
            id,
            source_address: str_source_address,
            destination_address: str_destination_address,
        } = request.into_inner();

        if let (Ok(source_address), Ok(destination_address)) = (
            bitcoin::Address::from_str(&str_source_address),
            bitcoin::Address::from_str(&str_destination_address),
        ) {
            let oneshot_rx = self
                .process_request(BusMsg::Bridge(BridgeMsg::Info {
                    service_id: ServiceId::Database,
                    request: InfoMsg::GetAddressSecretKey(Address::Bitcoin(source_address.clone())),
                }))
                .await?;
            match oneshot_rx.await {
                Ok(BusMsg::Info(InfoMsg::AddressSecretKey(AddressSecretKey::Bitcoin {
                    secret_key_info,
                    address: _,
                }))) => {
                    let oneshot_rx = self
                        .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                            request: CtlMsg::SweepAddress(SweepAddressAddendum::Bitcoin(
                                SweepBitcoinAddress {
                                    source_address,
                                    source_secret_key: secret_key_info.secret_key,
                                    destination_address,
                                },
                            )),
                            service_id: ServiceId::Farcasterd,
                        }))
                        .await?;
                    match oneshot_rx.await {
                        Ok(BusMsg::Info(InfoMsg::String(message))) => {
                            let reply = SweepAddressResponse { id, message };
                            Ok(GrpcResponse::new(reply))
                        }
                        res => process_error_response(res),
                    }
                }
                res => process_error_response(res),
            }
        } else if let (Ok(source_address), Ok(destination_address)) = (
            monero::Address::from_str(&str_source_address),
            monero::Address::from_str(&str_destination_address),
        ) {
            let oneshot_rx = self
                .process_request(BusMsg::Bridge(BridgeMsg::Info {
                    service_id: ServiceId::Database,
                    request: InfoMsg::GetAddressSecretKey(Address::Monero(source_address.clone())),
                }))
                .await?;
            match oneshot_rx.await {
                Ok(BusMsg::Info(InfoMsg::AddressSecretKey(AddressSecretKey::Monero {
                    secret_key_info,
                    address: _,
                }))) => {
                    let oneshot_rx = self
                        .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                            request: CtlMsg::SweepAddress(SweepAddressAddendum::Monero(
                                SweepMoneroAddress {
                                    source_spend_key: secret_key_info.spend,
                                    source_view_key: secret_key_info.view,
                                    destination_address,
                                    minimum_balance: monero::Amount::from_pico(0),
                                    from_height: secret_key_info.creation_height,
                                },
                            )),
                            service_id: ServiceId::Farcasterd,
                        }))
                        .await?;
                    match oneshot_rx.await {
                        Ok(BusMsg::Info(InfoMsg::String(message))) => {
                            let reply = SweepAddressResponse { id, message };
                            Ok(GrpcResponse::new(reply))
                        }
                        res => process_error_response(res),
                    }
                }
                res => process_error_response(res),
            }
        } else {
            Err(Status::invalid_argument(format!("address malformed")))
        }
    }

    async fn take(
        &self,
        request: GrpcRequest<TakeRequest>,
    ) -> Result<GrpcResponse<TakeResponse>, Status> {
        debug!("Received a grpc take request: {:?}", request);
        let TakeRequest {
            id,
            public_offer: str_public_offer,
            bitcoin_address: str_bitcoin_address,
            monero_address: str_monero_address,
        } = request.into_inner();

        let bitcoin_address = bitcoin::Address::from_str(&str_bitcoin_address)
            .map_err(|_| Status::invalid_argument("arbitrating address"))?;
        let monero_address = monero::Address::from_str(&str_monero_address)
            .map_err(|_| Status::invalid_argument("accordant_address"))?;
        let public_offer = PublicOffer::from_str(&str_public_offer)
            .map_err(|_| Status::invalid_argument("public offer"))?;

        let PublicOffer { offer, .. } = public_offer.clone();

        let network = offer.network;
        let arbitrating_amount = offer.arbitrating_amount;
        let accordant_amount = offer.accordant_amount;

        if network != bitcoin_address.network.into() {
            return Err(Status::invalid_argument(format!(
                "Error: The address {} is not for {}",
                bitcoin_address, network
            )));
        }
        // monero local address types are mainnet address types
        if network != monero_address.network.into() && network != Network::Local {
            return Err(Status::invalid_argument(format!(
                "Error: The address {} is not for {}",
                monero_address, network
            )));
        }

        if arbitrating_amount > bitcoin::Amount::from_str("0.01 BTC").unwrap()
            && network == Network::Mainnet
        {
            return Err(Status::invalid_argument(format!(
                "Error: Bitcoin amount {} too high, mainnet amount capped at 0.01 BTC.",
                arbitrating_amount
            )));
        }
        if accordant_amount > monero::Amount::from_str("2 XMR").unwrap()
            && network == Network::Mainnet
        {
            return Err(Status::invalid_argument(format!(
                "Error: Monero amount {} too high, mainnet amount capped at 2 XMR.",
                accordant_amount
            )));
        }
        if accordant_amount < monero::Amount::from_str("0.001 XMR").unwrap() {
            return Err(Status::invalid_argument(format!(
                "Error: Monero amount {} too low, require at least 0.001 XMR",
                accordant_amount
            )));
        }

        let oneshot_rx = self
            .process_request(BusMsg::Bridge(BridgeMsg::Ctl {
                request: CtlMsg::TakeOffer(PubOffer {
                    public_offer,
                    bitcoin_address: bitcoin_address,
                    monero_address: monero_address,
                }),
                service_id: ServiceId::Farcasterd,
            }))
            .await?;

        match oneshot_rx.await {
            Ok(BusMsg::Info(InfoMsg::TookOffer(_))) => {
                let reply = farcaster::TakeResponse { id };
                Ok(GrpcResponse::new(reply))
            }
            res => process_error_response(res),
        }
    }
}

pub struct GrpcServer {
    grpc_port: u16,
    grpc_ip: String,
}

fn request_loop(
    mut tokio_rx_request: tokio::sync::mpsc::Receiver<(u64, BusMsg)>,
    tx_request: zmq::Socket,
) -> tokio::task::JoinHandle<Result<(), Error>> {
    tokio::task::spawn(async move {
        let mut session = LocalSession::with_zmq_socket(ZmqSocketType::Push, tx_request);
        while let Some((id, request)) = tokio_rx_request.recv().await {
            debug!("sending request over grpc bridge: {:?}", request);
            let grpc_client_address: Vec<u8> = ServiceId::GrpcdClient(id).into();
            let grpc_address: Vec<u8> = ServiceId::Grpcd.into();
            if let Err(err) = session.send_routed_message(
                &grpc_client_address,
                &grpc_address,
                &grpc_address,
                &request.serialize(),
            ) {
                error!(
                    "Error encountered while sending request to GRPC runtime: {}",
                    err
                );
                return Err(Error::Farcaster(err.to_string()));
            }
        }
        Ok(())
    })
}

fn response_loop(
    mpsc_rx_response: Receiver<(u64, BusMsg)>,
    pending_requests_lock: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<BusMsg>>>>,
) -> tokio::task::JoinHandle<Result<(), Error>> {
    tokio::task::spawn(async move {
        loop {
            let response = mpsc_rx_response.try_recv();
            match response {
                Ok((id, request)) => {
                    let mut pending_requests = pending_requests_lock.lock().await;
                    if let Some(sender) = pending_requests.remove(&id) {
                        if let Err(err) = sender.send(request) {
                            error!(
                                "Error encountered while sending response to Grpc server: {}",
                                err
                            );
                            break;
                        }
                    } else {
                        error!("id {} not found in pending grpc requests", id);
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    return Err(Error::Farcaster(
                        "Response receiver disconnected in grpc runtime".to_string(),
                    ))
                }
                _ => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            };
        }
        Ok(())
    })
}

fn server_loop(
    service: FarcasterService,
    addr: SocketAddr,
) -> tokio::task::JoinHandle<Result<(), Error>> {
    tokio::task::spawn(async move {
        let layer = CorsLayer::new().allow_origin(AllowOrigin::any());
        let layer = GrpcWebLayer::new().layer(layer);
        if let Err(err) = Server::builder()
            .accept_http1(true)
            .layer(layer)
            .add_service(FarcasterServer::new(service))
            .serve(addr)
            .await
        {
            error!("Error encountered while running grpc server: {}", err);
            Err(Error::Farcaster(err.to_string()))
        } else {
            Ok(())
        }
    })
}

impl GrpcServer {
    fn run(
        &mut self,
        rx_response: Receiver<(u64, BusMsg)>,
        tx_request: zmq::Socket,
    ) -> Result<(), Error> {
        // We panic here, because we cannot recover from a bad configuration
        let addr = format!("{}:{}", self.grpc_ip, self.grpc_port)
            .parse()
            .expect("invalid grpc server bind address");
        info!("Binding grpc to address: {}", addr);

        std::thread::spawn(move || {
            // We panic on the async runtime failing, because this is indicative
            // of a deeper problem that won't be solved by re-trying
            let rt = Builder::new_multi_thread()
                .worker_threads(3)
                .enable_all()
                .build()
                .expect("failed to build new tokio runtime");
            rt.block_on(async {
                let (tokio_tx_request, tokio_rx_request) = tokio::sync::mpsc::channel(1000);

                let pending_requests: Arc<
                    Mutex<HashMap<u64, tokio::sync::oneshot::Sender<BusMsg>>>,
                > = Arc::new(Mutex::new(map![]));
                let request_handle = request_loop(tokio_rx_request, tx_request);
                let response_handle = response_loop(rx_response, Arc::clone(&pending_requests));

                let service = FarcasterService {
                    id_counter: Arc::new(Mutex::new(IdCounter(0))),
                    tokio_tx_request,
                    pending_requests,
                };

                let server_handle = server_loop(service, addr);

                // this drives the tokio execution
                let res = tokio::try_join!(request_handle, response_handle, server_handle);
                warn!("exiting grpc server run routine with: {:?}", res);
            });

            // Connect to the PULL inproc socket to let the microserver runtime
            // know that the server runtime terminated
            let tx_request = ZMQ_CONTEXT
                .socket(zmq::PUSH)
                .expect("Panic while creating a zmq socket");
            tx_request
                .connect("inproc://grpcdbridge")
                .expect("Panic while connecting to bridge socket");
            let mut session = LocalSession::with_zmq_socket(ZmqSocketType::Push, tx_request);
            let request = BusMsg::Bridge(BridgeMsg::GrpcServerTerminated);
            debug!(
                "sending grpc runtime exited request over grpc bridge: {:?}",
                request
            );
            let grpc_address: Vec<u8> = ServiceId::Grpcd.into();
            if let Err(err) = session.send_routed_message(
                &grpc_address,
                &grpc_address,
                &grpc_address,
                &request.serialize(),
            ) {
                error!("Failed to send the grpc server terminated message to the runtime. The Grpc server will not recover. Error: {}", err);
            }
        });
        Ok(())
    }
}

pub fn run(config: ServiceConfig, grpc_port: u16, grpc_ip: String) -> Result<(), Error> {
    let (tx_response, rx_response): (Sender<(u64, BusMsg)>, Receiver<(u64, BusMsg)>) =
        std::sync::mpsc::channel();

    let tx_request = ZMQ_CONTEXT.socket(zmq::PUSH)?;
    let rx_request = ZMQ_CONTEXT.socket(zmq::PULL)?;
    tx_request.connect("inproc://grpcdbridge")?;
    rx_request.bind("inproc://grpcdbridge")?;

    let mut server = GrpcServer {
        grpc_port,
        grpc_ip: grpc_ip.clone(),
    };
    server.run(rx_response, tx_request)?;

    let runtime = Runtime {
        identity: ServiceId::Grpcd,
        tx_response,
        grpc_port,
        grpc_ip,
    };

    let mut service = Service::service(config, runtime)?;
    service.add_bridge_service_bus(rx_request)?;
    service.run_loop()?;
    unreachable!()
}

pub struct Runtime {
    identity: ServiceId,
    tx_response: Sender<(u64, BusMsg)>,
    grpc_port: u16,
    grpc_ip: String,
}

impl CtlServer for Runtime {}

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
            // Control bus for database command, only accept Ctl message
            (ServiceBus::Ctl, BusMsg::Ctl(req)) => self.handle_ctl(endpoints, source, req),
            // Info bus for client, only accept Info message
            (ServiceBus::Info, BusMsg::Info(req)) => self.handle_info(endpoints, source, req),
            // Internal bridge, accept all type of message
            (ServiceBus::Bridge, req) => self.handle_bridge(endpoints, source, req),
            // All other pairs are not supported
            (_, request) => Err(Error::NotSupported(bus, request.to_string())),
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
    fn handle_ctl(
        &mut self,
        _endpoints: &mut Endpoints,
        source: ServiceId,
        request: CtlMsg,
    ) -> Result<(), Error> {
        match request {
            CtlMsg::Hello => {
                debug!("Received Hello from {}", source);
            }

            req => {
                if let ServiceId::GrpcdClient(id) = source {
                    self.tx_response
                        .send((id, BusMsg::Ctl(req)))
                        .map_err(|err| Error::Farcaster(err.to_string()))?;
                } else {
                    error!("Grpcd server can only handle messages addressed to a grpcd client");
                }
            }
        }

        Ok(())
    }

    fn handle_info(
        &mut self,
        _endpoints: &mut Endpoints,
        source: ServiceId,
        request: InfoMsg,
    ) -> Result<(), Error> {
        if let ServiceId::GrpcdClient(id) = source {
            self.tx_response
                .send((id, BusMsg::Info(request)))
                .map_err(|err| Error::Farcaster(err.to_string()))?;
        } else {
            error!("Grpcd server can only handle messages addressed to a grpcd client");
        }

        Ok(())
    }

    fn handle_bridge(
        &mut self,
        endpoints: &mut Endpoints,
        source: ServiceId,
        request: BusMsg,
    ) -> Result<(), Error> {
        debug!("GRPCD BRIDGE RPC request: {}, {}", request, source);
        match request {
            BusMsg::Bridge(BridgeMsg::Ctl {
                request,
                service_id,
            }) => endpoints.send_to(ServiceBus::Ctl, source, service_id, BusMsg::Ctl(request))?,
            BusMsg::Bridge(BridgeMsg::Info {
                request,
                service_id,
            }) => endpoints.send_to(ServiceBus::Info, source, service_id, BusMsg::Info(request))?,
            BusMsg::Bridge(BridgeMsg::GrpcServerTerminated) => {
                // Re-create the grpc server runtime.
                let (tx_response, rx_response): (Sender<(u64, BusMsg)>, Receiver<(u64, BusMsg)>) =
                    std::sync::mpsc::channel();

                let tx_request = ZMQ_CONTEXT.socket(zmq::PUSH)?;
                tx_request.connect("inproc://grpcdbridge")?;

                let mut server = GrpcServer {
                    grpc_port: self.grpc_port.clone(),
                    grpc_ip: self.grpc_ip.clone(),
                };
                server.run(rx_response, tx_request)?;
                self.tx_response = tx_response;
            }
            _ => error!("Could not send this type of request over the bridge"),
        }
        Ok(())
    }
}
