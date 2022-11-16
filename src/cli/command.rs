// LNP Node: node running lightning network protocol and generalized lightning
// channels.
// Written in 2020 by
//     Dr. Maxim Orlovsky <orlovsky@pandoracore.com>
//
// To the extent possible under law, the author(s) have dedicated all
// copyright and related and neighboring rights to this software to
// the public domain worldwide. This software is distributed without
// any warranty.
//
// You should have received a copy of the MIT License
// along with this software.
// If not, see <https://opensource.org/licenses/MIT>.

use farcaster_core::swap::btcxmr::Offer;
use std::io::{self, Read};
use std::str::FromStr;
use uuid::Uuid;

use internet2::addr::{InetSocketAddr, NodeAddr};
use microservices::shell::Exec;

use clap::IntoApp;
use clap_complete::generate;
use clap_complete::shells::*;
use farcaster_core::{
    blockchain::{Blockchain, Network},
    negotiation::PublicOffer,
    role::SwapRole,
    swap::SwapId,
};

use super::Command;
use crate::bus::{
    ctl::{self, CtlMsg},
    info::{Address, InfoMsg},
    AddressSecretKey,
};
use crate::bus::{BusMsg, Failure, FailureCode};
use crate::client::Client;
use crate::syncerd::{SweepAddressAddendum, SweepBitcoinAddress, SweepMoneroAddress};
use crate::{Error, LogStyle, ServiceId};

impl Exec for Command {
    type Client = Client;
    type Error = Error;

    fn exec(self, runtime: &mut Self::Client) -> Result<(), Self::Error> {
        debug!("Performing {:?}: {}", self, self);
        match self {
            Command::Info { subject } => {
                let err = format!(
                    "{}",
                    "Subject parameter must be either remote node address, swap id, or syncer"
                        .err()
                );
                let target_service_id = match subject.len() {
                    0 => {
                        runtime.request_info(ServiceId::Farcasterd, InfoMsg::GetInfo)?;
                        ServiceId::Farcasterd
                    }
                    1 => {
                        let subj = subject.get(0).expect("vec of lenght 1");
                        if let Ok(node_addr) = NodeAddr::from_str(&subj) {
                            runtime.request_info(ServiceId::Peer(node_addr), InfoMsg::GetInfo)?;
                            ServiceId::Peer(node_addr)
                        } else if let Ok(swap_id) = SwapId::from_str(&subj) {
                            runtime.request_info(ServiceId::Swap(swap_id), InfoMsg::GetInfo)?;
                            ServiceId::Swap(swap_id)
                        } else {
                            return Err(Error::Other(err));
                        }
                    }
                    2 => {
                        let blockchain =
                            Blockchain::from_str(&subject.get(0).expect("vec of lenght 2"))?;
                        let network = Network::from_str(&subject.get(1).expect("vec of lenght 2"))?;
                        runtime.request_info(
                            ServiceId::Syncer(blockchain, network),
                            InfoMsg::GetInfo,
                        )?;
                        ServiceId::Syncer(blockchain, network)
                    }
                    _ => {
                        return Err(Error::Other(err));
                    }
                };
                match runtime.response()? {
                    BusMsg::Info(InfoMsg::NodeInfo(info)) => println!("{}", info),
                    BusMsg::Info(InfoMsg::PeerInfo(info)) => println!("{}", info),
                    BusMsg::Info(InfoMsg::SwapInfo(info)) => println!("{}", info),
                    BusMsg::Info(InfoMsg::SyncerInfo(info)) => println!("{}", info),
                    BusMsg::Ctl(CtlMsg::Failure(Failure { code, .. }))
                        if code == FailureCode::TargetServiceNotFound =>
                    {
                        match target_service_id {
                            ServiceId::Peer(node_addr) => {
                                return Err(Error::Farcaster(format!(
                                    "No connected peerd with address {}",
                                    node_addr
                                )));
                            }
                            ServiceId::Swap(swap_id) => {
                                return Err(Error::Farcaster(format!(
                                    "No running swap with id {}",
                                    swap_id
                                )));
                            }
                            ServiceId::Syncer(blockchain, network) => {
                                return Err(Error::Farcaster(format!(
                                    "No running syncer for {} {}",
                                    blockchain, network
                                )));
                            }
                            _ => {
                                return Err(Error::Farcaster(format!(
                                    "The service {:?} does not exist",
                                    subject
                                )));
                            }
                        }
                    }
                    _ => {
                        return Err(Error::Other(
                            "Server returned unrecognizable response".to_string(),
                        ))
                    }
                }
            }

            Command::Peers => {
                runtime.request_info(ServiceId::Farcasterd, InfoMsg::ListPeers)?;
                runtime.report_response_or_fail()?;
            }

            Command::ListSwaps => {
                runtime.request_info(ServiceId::Farcasterd, InfoMsg::ListSwaps)?;
                runtime.report_response_or_fail()?;
            }

            // TODO: only list offers matching list of OfferIds
            Command::ListOffers { select } => {
                runtime.request_info(ServiceId::Farcasterd, InfoMsg::ListOffers(select.into()))?;
                runtime.report_response_or_fail()?;
            }

            Command::ListTasks {
                blockchain,
                network,
            } => {
                runtime.request_info(ServiceId::Syncer(blockchain, network), InfoMsg::ListTasks)?;
                runtime.report_response_or_fail()?;
            }

            Command::ListListens => {
                runtime.request_info(ServiceId::Farcasterd, InfoMsg::ListListens)?;
                runtime.report_response_or_fail()?;
            }

            Command::ListCheckpoints => {
                runtime.request_info(ServiceId::Database, InfoMsg::RetrieveAllCheckpointInfo)?;
                runtime.report_response_or_fail()?;
            }

            Command::RestoreCheckpoint { swap_id } => {
                runtime.request_info(ServiceId::Database, InfoMsg::GetCheckpointEntry(swap_id))?;
                if let BusMsg::Info(InfoMsg::CheckpointEntry(entry)) = runtime.report_failure()? {
                    runtime.request_ctl(ServiceId::Farcasterd, CtlMsg::RestoreCheckpoint(entry))?;
                    runtime.report_response_or_fail()?;
                } else {
                    return Err(Error::Farcaster("Received unexpected response".to_string()));
                }
            }

            Command::Make {
                network,
                arbitrating_blockchain,
                accordant_blockchain,
                arbitrating_amount,
                accordant_amount,
                arbitrating_addr,
                accordant_addr,
                cancel_timelock,
                punish_timelock,
                fee_strategy,
                maker_role,
                public_ip_addr,
                bind_ip_addr,
                port,
            } => {
                // Monero local address types are mainnet address types
                if network != accordant_addr.network.into() && network != Network::Local {
                    eprintln!(
                        "Error: The address {} is not for {}",
                        accordant_addr, network
                    );
                    return Ok(());
                }
                if network != arbitrating_addr.network.into() {
                    eprintln!(
                        "Error: The address {} is not for {}",
                        arbitrating_addr, network
                    );
                    return Ok(());
                }
                if arbitrating_amount > bitcoin::Amount::from_str("0.01 BTC").unwrap()
                    && network == Network::Mainnet
                {
                    eprintln!(
                        "Error: Bitcoin amount {} too high, mainnet amount capped at 0.01 BTC.",
                        arbitrating_amount
                    );
                    return Ok(());
                }
                if accordant_amount > monero::Amount::from_str("2 XMR").unwrap()
                    && network == Network::Mainnet
                {
                    eprintln!(
                        "Error: Monero amount {} too high, mainnet amount capped at 2 XMR.",
                        accordant_amount
                    );
                    return Ok(());
                }
                if accordant_amount < monero::Amount::from_str("0.001 XMR").unwrap() {
                    eprintln!(
                        "Error: Monero amount {} too low, require at least 0.001 XMR",
                        accordant_amount
                    );
                    return Ok(());
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
                let public_addr = InetSocketAddr::socket(public_ip_addr, port);
                let bind_addr = InetSocketAddr::socket(bind_ip_addr, port);
                let proto_offer = ctl::ProtoPublicOffer {
                    offer,
                    public_addr,
                    bind_addr,
                    arbitrating_addr,
                    accordant_addr,
                };
                runtime.request_ctl(ServiceId::Farcasterd, CtlMsg::MakeOffer(proto_offer))?;
                // report success or failure of the request to cli
                runtime.report_response_or_fail()?;
            }

            Command::OfferInfo { public_offer } => {
                println!(
                    "\n Trading {}\n",
                    offer_buy_information(&public_offer.offer)
                );
                println!(
                    "{}",
                    serde_yaml::to_string(&public_offer).expect("already parsed")
                );
            }

            Command::Take {
                public_offer,
                bitcoin_address,
                monero_address,
                without_validation,
            } => {
                let PublicOffer {
                    version: _,
                    offer,
                    node_id,
                    peer_address,
                } = public_offer.clone();

                let network = offer.network;
                let arbitrating_amount = offer.arbitrating_amount;
                let accordant_amount = offer.accordant_amount;

                if network != bitcoin_address.network.into() {
                    eprintln!(
                        "Error: The address {} is not for {}",
                        bitcoin_address, network
                    );
                    return Ok(());
                }
                // monero local address types are mainnet address types
                if network != monero_address.network.into() && network != Network::Local {
                    eprintln!(
                        "Error: The address {} is not for {}",
                        monero_address, network
                    );
                    return Ok(());
                }

                if arbitrating_amount > bitcoin::Amount::from_str("0.01 BTC").unwrap()
                    && network == Network::Mainnet
                {
                    eprintln!(
                        "Error: Bitcoin amount {} too high, mainnet amount capped at 0.01 BTC.",
                        arbitrating_amount
                    );
                    return Ok(());
                }
                if accordant_amount > monero::Amount::from_str("2 XMR").unwrap()
                    && network == Network::Mainnet
                {
                    eprintln!(
                        "Error: Monero amount {} too high, mainnet amount capped at 2 XMR.",
                        accordant_amount
                    );
                    return Ok(());
                }
                if accordant_amount < monero::Amount::from_str("0.001 XMR").unwrap() {
                    eprintln!(
                        "Error: Monero amount {} too low, require at least 0.001 XMR",
                        accordant_amount
                    );
                    return Ok(());
                }

                if !without_validation {
                    println!(
                        "\nWant to buy {}?\n\nCarefully validate offer!\n",
                        offer_buy_information(&offer)
                    );
                    println!("Trade counterparty: {}@{}\n", &node_id, peer_address);
                    println!(
                        "{}",
                        serde_yaml::to_string(&public_offer).expect("already parsed")
                    );
                }
                if without_validation || take_offer() {
                    // pass offer to farcasterd to initiate the swap
                    runtime.request_ctl(
                        ServiceId::Farcasterd,
                        CtlMsg::TakeOffer(ctl::PubOffer {
                            public_offer,
                            external_address: bitcoin_address,
                            internal_address: monero_address,
                        }),
                    )?;
                    // report success of failure of the request to cli
                    runtime.report_response_or_fail()?;
                }
            }

            Command::RevokeOffer { public_offer } => {
                runtime.request_ctl(ServiceId::Farcasterd, CtlMsg::RevokeOffer(public_offer))?;
                runtime.report_response_or_fail()?;
            }

            Command::AbortSwap { swap_id } => {
                runtime.request_ctl(ServiceId::Swap(swap_id), CtlMsg::AbortSwap)?;
                runtime.report_response_or_fail()?;
            }

            Command::Progress { swapid, follow } => {
                if follow {
                    // subscribe to progress event and loop until Finish event is received or user
                    // ctrl-c the cli. Expect to recieve a stream of event responses
                    runtime
                        .request_info(ServiceId::Farcasterd, InfoMsg::SubscribeProgress(swapid))?;
                    let res = runtime.report_progress();
                    // if user didn't ctrl-c before that point we can cleanly unsubscribe the
                    // client from the notification stream and then return the result from report
                    // progress
                    runtime.request_info(
                        ServiceId::Farcasterd,
                        InfoMsg::UnsubscribeProgress(swapid),
                    )?;
                    return res;
                } else {
                    // request a read progress response. Expect to recieve only one response and
                    // quit
                    runtime.request_info(ServiceId::Farcasterd, InfoMsg::ReadProgress(swapid))?;
                    runtime.report_response_or_fail()?;
                }
            }

            Command::NeedsFunding { blockchain } => {
                runtime.request_info(ServiceId::Farcasterd, InfoMsg::NeedsFunding(blockchain))?;
                runtime.report_response_or_fail()?;
            }

            Command::SweepBitcoinAddress {
                source_address,
                destination_address,
            } => {
                runtime.request_info(
                    ServiceId::Database,
                    InfoMsg::GetAddressSecretKey(Address::Bitcoin(source_address.clone())),
                )?;
                if let BusMsg::Info(InfoMsg::AddressSecretKey(AddressSecretKey::Bitcoin {
                    secret_key_info,
                    ..
                })) = runtime.report_failure()?
                {
                    runtime.request_ctl(
                        ServiceId::Farcasterd,
                        CtlMsg::SweepAddress(SweepAddressAddendum::Bitcoin(SweepBitcoinAddress {
                            source_address,
                            source_secret_key: secret_key_info.secret_key,
                            destination_address,
                        })),
                    )?;
                    runtime.report_response_or_fail()?;
                } else {
                    return Err(Error::Farcaster("Received unexpected response".to_string()));
                }
            }

            Command::SweepMoneroAddress {
                source_address,
                destination_address,
            } => {
                runtime.request_info(
                    ServiceId::Database,
                    InfoMsg::GetAddressSecretKey(Address::Monero(source_address)),
                )?;
                if let BusMsg::Info(InfoMsg::AddressSecretKey(AddressSecretKey::Monero {
                    secret_key_info,
                    ..
                })) = runtime.report_failure()?
                {
                    runtime.request_ctl(
                        ServiceId::Farcasterd,
                        CtlMsg::SweepAddress(SweepAddressAddendum::Monero(SweepMoneroAddress {
                            source_spend_key: secret_key_info.spend,
                            source_view_key: secret_key_info.view,
                            destination_address,
                            minimum_balance: monero::Amount::from_pico(0),
                        })),
                    )?;
                    runtime.report_response_or_fail()?;
                } else {
                    return Err(Error::Farcaster("Received unexpected response".to_string()));
                }
            }

            Command::Completion { shell } => {
                let mut app = super::Opts::command();
                let name = app.get_name().to_string();
                match shell {
                    Shell::Bash => generate(Bash, &mut app, &name, &mut io::stdout()),
                    Shell::Elvish => generate(Elvish, &mut app, &name, &mut io::stdout()),
                    Shell::Fish => generate(Fish, &mut app, &name, &mut io::stdout()),
                    Shell::PowerShell => generate(PowerShell, &mut app, &name, &mut io::stdout()),
                    Shell::Zsh => generate(Zsh, &mut app, &name, &mut io::stdout()),
                    _ => {
                        return Err(Error::Other(s!(
                            "Unsupported shell, cannot generate completion scripts!"
                        )))
                    }
                }
            }
        }

        Ok(())
    }
}

fn take_offer() -> bool {
    println!("Take it? [y/n]");
    let mut input = [0u8; 1];
    std::io::stdin().read_exact(&mut input).unwrap_or(());
    match std::str::from_utf8(&input[..]) {
        Ok("y") | Ok("Y") => true,
        Ok("n") | Ok("N") => {
            println!("Rejecting offer");
            false
        }
        _ => take_offer(),
    }
}

fn offer_buy_information(offer: &Offer) -> String {
    match offer.maker_role.other() {
        SwapRole::Alice => format!(
            "{} for {} at {} BTC/XMR",
            offer.arbitrating_amount,
            offer.accordant_amount,
            offer.arbitrating_amount.as_btc() / offer.accordant_amount.as_xmr()
        ),
        SwapRole::Bob => format!(
            "{} for {} at {} XMR/BTC",
            offer.accordant_amount,
            offer.arbitrating_amount,
            offer.accordant_amount.as_xmr() / offer.arbitrating_amount.as_btc()
        ),
    }
}
