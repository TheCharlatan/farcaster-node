use std::io;

use bitcoin::secp256k1::ecdsa::Signature;
use farcaster_core::{
    bitcoin::{
        segwitv0::{BuyTx, CancelTx, FundingTx, LockTx, PunishTx, RefundTx},
        BitcoinSegwitV0,
    },
    blockchain::FeePriority,
    consensus::{self, CanonicalBytes, Decodable, Encodable},
    crypto::{
        dleq::DLEQProof, ArbitratingKeyId, CommitmentEngine, GenerateKey, ProveCrossGroupDleq,
    },
    impl_strict_encoding,
    monero::Monero,
    role::{SwapRole, TradeRole},
    swap::btcxmr::{
        message::{RefundProcedureSignatures, RevealProof},
        Alice, Bob, EncryptedSignature, KeyManager, Parameters, PublicOffer,
    },
    swap::{
        btcxmr::{
            message::{
                BuyProcedureSignature, CommitAliceParameters, CommitBobParameters,
                CoreArbitratingSetup,
            },
            FullySignedPunish, TxSignatures,
        },
        SwapId,
    },
    transaction::{Broadcastable, Fundable, Transaction, Witnessable},
};
use strict_encoding::{StrictDecode, StrictEncode};

use crate::{
    bus::{
        ctl::{self, CheckpointState, CtlMsg, Tx},
        p2p::{Commit, PeerMsg, Reveal},
        AddressSecretKey, BusMsg, ServiceBus,
    },
    databased::checkpoint_send,
    swapd::runtime::Runtime,
    walletd::runtime::CheckpointWallet,
    Endpoints, Error, LogStyle, ServiceId,
};

#[derive(Clone, Debug, StrictEncode, StrictDecode)]
pub enum Wallet {
    Alice(AliceState),
    Bob(BobState),
}

#[derive(Clone, Debug)]
pub struct AliceState {
    pub alice: Alice,
    pub local_trade_role: TradeRole,
    pub local_params: Parameters,
    pub key_manager: KeyManager,
    pub pub_offer: PublicOffer,
    pub remote_commit: Option<CommitBobParameters>,
    pub remote_params: Option<Parameters>,
    pub remote_proof: Option<DLEQProof>,
    pub core_arb_setup: Option<CoreArbitratingSetup>,
    pub alice_cancel_signature: Option<Signature>,
    pub adaptor_refund: Option<EncryptedSignature>,
}

impl Encodable for AliceState {
    fn consensus_encode<W: io::Write>(&self, writer: &mut W) -> Result<usize, io::Error> {
        let mut len = self.alice.consensus_encode(writer)?;
        len += self.local_trade_role.consensus_encode(writer)?;
        len += self.local_params.consensus_encode(writer)?;
        len += self.key_manager.consensus_encode(writer)?;
        len += self.pub_offer.consensus_encode(writer)?;
        len += self.remote_commit.consensus_encode(writer)?;
        len += self.remote_params.consensus_encode(writer)?;
        len += self.remote_proof.consensus_encode(writer)?;
        len += self.core_arb_setup.consensus_encode(writer)?;
        len += farcaster_core::consensus::Encodable::consensus_encode(
            &self.alice_cancel_signature.as_canonical_bytes(),
            writer,
        )?;
        len += self.adaptor_refund.consensus_encode(writer)?;
        Ok(len)
    }
}

impl Decodable for AliceState {
    fn consensus_decode<D: io::Read>(d: &mut D) -> Result<Self, consensus::Error> {
        Ok(AliceState {
            alice: Decodable::consensus_decode(d)?,
            local_trade_role: Decodable::consensus_decode(d)?,
            local_params: Decodable::consensus_decode(d)?,
            key_manager: Decodable::consensus_decode(d)?,
            pub_offer: Decodable::consensus_decode(d)?,
            remote_commit: Decodable::consensus_decode(d)?,
            remote_params: Decodable::consensus_decode(d)?,
            remote_proof: Decodable::consensus_decode(d)?,
            core_arb_setup: Decodable::consensus_decode(d)?,
            alice_cancel_signature: Option::<Signature>::from_canonical_bytes(
                farcaster_core::unwrap_vec_ref!(d).as_ref(),
            )?,
            adaptor_refund: Decodable::consensus_decode(d)?,
        })
    }
}

impl_strict_encoding!(AliceState);

impl AliceState {
    pub fn new(
        alice: Alice,
        local_trade_role: TradeRole,
        local_params: Parameters,
        key_manager: KeyManager,
        pub_offer: PublicOffer,
        remote_commit: Option<CommitBobParameters>,
    ) -> Self {
        Self {
            alice,
            local_trade_role,
            local_params,
            key_manager,
            pub_offer,
            remote_commit,
            remote_params: None,
            remote_proof: None,
            core_arb_setup: None,
            alice_cancel_signature: None,
            adaptor_refund: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BobState {
    pub bob: Bob,
    pub local_trade_role: TradeRole,
    pub local_params: Parameters,
    pub key_manager: KeyManager,
    pub pub_offer: PublicOffer,
    pub funding_tx: Option<FundingTx>,
    pub remote_commit_params: Option<CommitAliceParameters>,
    pub remote_params: Option<Parameters>,
    pub remote_proof: Option<DLEQProof>,
    pub core_arb_setup: Option<CoreArbitratingSetup>,
    pub adaptor_buy: Option<BuyProcedureSignature>,
}

impl Encodable for BobState {
    fn consensus_encode<W: io::Write>(&self, writer: &mut W) -> Result<usize, io::Error> {
        let mut len = self.bob.consensus_encode(writer)?;
        len += self.local_trade_role.consensus_encode(writer)?;
        len += self.local_params.consensus_encode(writer)?;
        len += self.key_manager.consensus_encode(writer)?;
        len += self.pub_offer.consensus_encode(writer)?;
        len += self.funding_tx.consensus_encode(writer)?;
        len += self.remote_commit_params.consensus_encode(writer)?;
        len += self.remote_params.consensus_encode(writer)?;
        len += self.remote_proof.consensus_encode(writer)?;
        len += self.core_arb_setup.consensus_encode(writer)?;
        len += self.adaptor_buy.consensus_encode(writer)?;
        Ok(len)
    }
}

impl Decodable for BobState {
    fn consensus_decode<D: io::Read>(d: &mut D) -> Result<Self, consensus::Error> {
        Ok(BobState {
            bob: Decodable::consensus_decode(d)?,
            local_trade_role: Decodable::consensus_decode(d)?,
            local_params: Decodable::consensus_decode(d)?,
            key_manager: Decodable::consensus_decode(d)?,
            pub_offer: Decodable::consensus_decode(d)?,
            funding_tx: Decodable::consensus_decode(d)?,
            remote_commit_params: Decodable::consensus_decode(d)?,
            remote_params: Decodable::consensus_decode(d)?,
            remote_proof: Decodable::consensus_decode(d)?,
            core_arb_setup: Decodable::consensus_decode(d)?,
            adaptor_buy: Decodable::consensus_decode(d)?,
        })
    }
}

impl BobState {
    pub fn new(
        bob: Bob,
        local_trade_role: TradeRole,
        local_params: Parameters,
        key_manager: KeyManager,
        pub_offer: PublicOffer,
        funding_tx: Option<FundingTx>,
        remote_commit_params: Option<CommitAliceParameters>,
    ) -> Self {
        Self {
            bob,
            local_trade_role,
            local_params,
            key_manager,
            pub_offer,
            funding_tx,
            remote_commit_params,
            remote_params: None,
            remote_proof: None,
            core_arb_setup: None,
            adaptor_buy: None,
        }
    }
}

impl_strict_encoding!(BobState);

impl Wallet {
    fn process_ctl_msg_with_wallet(
        &mut self,
        request: CtlMsg,
        runtime: Runtime,
        endpoints: &mut Endpoints,
        swapd_id: SwapId,
    ) -> Result<(), Error> {
        match request {
            // First message received from farcaster to create a taker wallet.
            CtlMsg::TakeOffer(ctl::PubOffer {
                public_offer,
                external_address,
                internal_address,
            }) if source == ServiceId::Farcasterd => {
                let PublicOffer { offer, .. } = public_offer.clone();

                let swap_id: SwapId = SwapId::random();
                self.xmr_addrs.insert(swap_id, internal_address);

                // since we're takers, we are on the other side of the trade
                let taker_role = offer.maker_role.other();
                let wallet_index = self.node_secrets.increment_wallet_counter();
                let mut key_manager = KeyManager::new(self.node_secrets.wallet_seed, wallet_index)?;
                match taker_role {
                    SwapRole::Bob => {
                        let bob = Bob::new(
                            BitcoinSegwitV0::new(),
                            Monero,
                            external_address,
                            FeePriority::Low,
                        );
                        let local_params =
                            bob.generate_parameters(&mut key_manager, &public_offer)?;
                        let funding = create_funding(&mut key_manager, offer.network)?;
                        let funding_addr = funding.get_address()?;
                        self.send_ctl(
                            endpoints,
                            ServiceId::Database,
                            BusMsg::Ctl(CtlMsg::SetAddressSecretKey(AddressSecretKey::Bitcoin {
                                address: funding_addr.clone(),
                                secret_key: key_manager
                                    .get_or_derive_bitcoin_key(ArbitratingKeyId::Lock)?,
                            })),
                        )?;
                        info!("{} | Loading {}", swap_id.swap_id(), "Wallet::Bob".label());
                        let local_trade_role = TradeRole::Taker;
                        if self.wallets.get(&swap_id).is_none() {
                            let local_wallet = BobState::new(
                                bob,
                                local_trade_role,
                                local_params.clone(),
                                key_manager,
                                public_offer.clone(),
                                Some(funding),
                                None,
                            );
                            self.wallets.insert(swap_id, Wallet::Bob(local_wallet));
                        } else {
                            error!("{} | Wallet already exists", swap_id.swap_id());
                            return Ok(());
                        }
                        let launch_swap = LaunchSwap {
                            local_trade_role,
                            public_offer,
                            local_params: Params::Bob(local_params),
                            swap_id,
                            remote_commit: None,
                            funding_address: Some(funding_addr),
                        };
                        endpoints.send_to(
                            ServiceBus::Ctl,
                            source,
                            ServiceId::Farcasterd,
                            BusMsg::Ctl(CtlMsg::LaunchSwap(launch_swap)),
                        )?;
                    }
                    SwapRole::Alice => {
                        let alice = Alice::new(
                            BitcoinSegwitV0::new(),
                            Monero,
                            external_address,
                            FeePriority::Low,
                        );
                        let local_params =
                            alice.generate_parameters(&mut key_manager, &public_offer)?;
                        let wallet_seed = self.node_secrets.wallet_seed;
                        let key_manager = KeyManager::new(wallet_seed, wallet_index)?;
                        let local_trade_role = TradeRole::Taker;

                        if self.wallets.get(&swap_id).is_none() {
                            // TODO instead of storing in state, start building
                            // requests and store the state in there directly
                            info!("{} | Loading Alice Taker's Wallet", swap_id.swap_id());
                            let wallet = AliceState::new(
                                alice,
                                local_trade_role,
                                local_params.clone(),
                                key_manager,
                                public_offer.clone(),
                                None,
                            );
                            self.wallets.insert(swap_id, Wallet::Alice(wallet));
                        } else {
                            error!("{} | Wallet already exists", swap_id.swap_id());
                        }
                        let launch_swap = LaunchSwap {
                            local_trade_role,
                            public_offer,
                            local_params: Params::Alice(local_params),
                            swap_id,
                            remote_commit: None,
                            funding_address: None,
                        };
                        endpoints.send_to(
                            ServiceBus::Ctl,
                            source,
                            ServiceId::Farcasterd,
                            BusMsg::Ctl(CtlMsg::LaunchSwap(launch_swap)),
                        )?;
                    }
                };
            }

            // First message received from farcaster to create a maker wallet.
            CtlMsg::TakerCommitted(TakerCommitted {
                swap_id,
                arbitrating_addr,
                accordant_addr,
                taker_commit,
            }) => {
                if self.btc_addrs.insert(swap_id, arbitrating_addr).is_some() {
                    error!(
                        "{} | Bitcoin address replaced accidentally",
                        swap_id.swap_id()
                    )
                };
                if self.xmr_addrs.insert(swap_id, accordant_addr).is_some() {
                    error!(
                        "{} | Monero address replaced accidentally",
                        swap_id.swap_id()
                    )
                };
                let TakeCommit {
                    commit: remote_commit,
                    public_offer,
                    ..
                } = taker_commit;
                trace!(
                    "Offer {} is known, you created it previously, initiating swap with taker",
                    &public_offer
                );
                let PublicOffer { offer, .. } = public_offer.clone();
                let external_address = self.btc_addrs.get(&swap_id).expect("checked above").clone();
                match offer.maker_role {
                    SwapRole::Bob => {
                        let bob = Bob::new(
                            BitcoinSegwitV0::new(),
                            Monero,
                            external_address,
                            FeePriority::Low,
                        );
                        let wallet_index = self.node_secrets.increment_wallet_counter();
                        let mut key_manager =
                            KeyManager::new(self.node_secrets.wallet_seed, wallet_index)?;
                        let local_params =
                            bob.generate_parameters(&mut key_manager, &public_offer)?;
                        if self.wallets.get(&swap_id).is_none() {
                            let funding = create_funding(&mut key_manager, offer.network)?;
                            let funding_addr = funding.get_address()?;
                            self.send_ctl(
                                endpoints,
                                ServiceId::Database,
                                BusMsg::Ctl(CtlMsg::SetAddressSecretKey(
                                    AddressSecretKey::Bitcoin {
                                        address: funding_addr.clone(),
                                        secret_key: key_manager
                                            .get_or_derive_bitcoin_key(ArbitratingKeyId::Lock)?,
                                    },
                                )),
                            )?;
                            info!("{} | Loading {}", swap_id.swap_id(), "Wallet::Bob".label());
                            let local_trade_role = TradeRole::Maker;
                            if let Commit::AliceParameters(remote_commit) = remote_commit.clone() {
                                let bob_wallet = BobState::new(
                                    bob,
                                    local_trade_role,
                                    local_params.clone(),
                                    key_manager,
                                    public_offer.clone(),
                                    Some(funding),
                                    Some(remote_commit),
                                );
                                self.wallets.insert(swap_id, Wallet::Bob(bob_wallet));
                            } else {
                                error!("{} | Not Commit::Alice", swap_id.swap_id());
                                return Ok(());
                            }
                            let launch_swap = LaunchSwap {
                                local_trade_role,
                                public_offer: public_offer,
                                local_params: Params::Bob(local_params),
                                swap_id,
                                remote_commit: Some(remote_commit),
                                funding_address: Some(funding_addr),
                            };
                            self.swaps.insert(swap_id, None);
                            self.send_ctl(
                                endpoints,
                                ServiceId::Farcasterd,
                                BusMsg::Ctl(CtlMsg::LaunchSwap(launch_swap)),
                            )?;
                        } else {
                            error!("{} | Wallet already existed", swap_id.swap_id());
                        }
                    }
                    SwapRole::Alice => {
                        let alice = Alice::new(
                            BitcoinSegwitV0::new(),
                            Monero,
                            external_address,
                            FeePriority::Low,
                        );
                        let wallet_seed = self.node_secrets.wallet_seed;
                        let wallet_index = self.node_secrets.increment_wallet_counter();
                        let mut key_manager = KeyManager::new(wallet_seed, wallet_index)?;
                        let local_params =
                            alice.generate_parameters(&mut key_manager, &public_offer)?;
                        if self.wallets.get(&swap_id).is_none() {
                            info!(
                                "{} | Loading {}",
                                swap_id.swap_id(),
                                "Wallet::Alice".label()
                            );
                            if let Commit::BobParameters(bob_commit) = remote_commit.clone() {
                                let local_trade_role = TradeRole::Maker;
                                let alice_state = AliceState::new(
                                    alice,
                                    local_trade_role,
                                    local_params.clone(),
                                    key_manager,
                                    public_offer.clone(),
                                    Some(bob_commit),
                                );

                                self.wallets.insert(swap_id, Wallet::Alice(alice_state));

                                let launch_swap = LaunchSwap {
                                    local_trade_role,
                                    public_offer: public_offer,
                                    local_params: Params::Alice(local_params),
                                    swap_id,
                                    remote_commit: Some(remote_commit),
                                    funding_address: None,
                                };
                                self.send_ctl(
                                    endpoints,
                                    ServiceId::Farcasterd,
                                    BusMsg::Ctl(CtlMsg::LaunchSwap(launch_swap)),
                                )?;
                            } else {
                                error!("{} | Not Commit::Bob", swap_id.swap_id());
                            }
                        } else {
                            error!("{} | Wallet already existed", swap_id.swap_id());
                        }
                    }
                }
            }

            CtlMsg::Tx(Tx::Funding(tx)) => {
                let swap_id = get_swap_id(&source)?;
                if let Some(Wallet::Bob(BobState {
                    funding_tx: Some(funding),
                    ..
                })) = self.wallets.get_mut(&swap_id)
                {
                    if funding.was_seen() {
                        warn!(
                            "{} | Funding was previously updated, ignoring",
                            swap_id.swap_id(),
                        );
                        return Ok(());
                    }
                    funding_update(funding, tx)?;
                    debug!(
                        "{} | bob's wallet informs swapd that funding was successfully updated",
                        swap_id,
                    );
                    endpoints.send_to(
                        ServiceBus::Ctl,
                        ServiceId::Wallet,
                        ServiceId::Swap(swap_id),
                        BusMsg::Ctl(CtlMsg::FundingUpdated),
                    )?;
                }
            }

            CtlMsg::Tx(Tx::Buy(buy_tx)) => {
                let swap_id = get_swap_id(&source)?;
                if let Some(Wallet::Bob(BobState {
                    bob,
                    local_params,
                    key_manager,
                    remote_params: Some(alice_params),
                    adaptor_buy: Some(adaptor_buy),
                    pub_offer,
                    ..
                })) = self
                {
                    let sk_a_btc = bob.recover_accordant_key(
                        key_manager,
                        alice_params,
                        adaptor_buy.buy_adaptor_sig.clone(),
                        buy_tx,
                    );
                    let mut sk_a_btc_buf: Vec<u8> = (*sk_a_btc.as_ref()).into();
                    sk_a_btc_buf.reverse();
                    let sk_a = monero::PrivateKey::from_slice(sk_a_btc_buf.as_ref())
                        .expect("Valid Monero Private Key");
                    info!(
                        "{} | Extracted monero key from Buy tx: {}",
                        swap_id.swap_id(),
                        sk_a.label()
                    );
                    let sk_b = key_manager.get_or_derive_monero_spend_key()?;
                    let spend = sk_a + sk_b;
                    info!(
                        "{} | Full secret monero spending key: {}",
                        swap_id.swap_id(),
                        spend.bright_green_bold()
                    );
                    let view_key_alice = *alice_params
                        .accordant_shared_keys
                        .clone()
                        .into_iter()
                        .find(|vk| vk.tag() == &SharedKeyId::new(SHARED_VIEW_KEY_ID))
                        .unwrap()
                        .elem();

                    let view_key_bob = *local_params
                        .accordant_shared_keys
                        .clone()
                        .into_iter()
                        .find(|vk| vk.tag() == &SharedKeyId::new(SHARED_VIEW_KEY_ID))
                        .unwrap()
                        .elem();
                    let view = view_key_alice + view_key_bob;
                    info!(
                        "{} | Full secret monero view key: {}",
                        swap_id.swap_id(),
                        view.bright_green_bold()
                    );
                    let network = pub_offer.offer.network.into();
                    let keypair = monero::KeyPair { view, spend };
                    let corresponding_address = monero::Address::from_keypair(network, &keypair);
                    info!(
                        "{} | Corresponding address: {}",
                        swap_id.swap_id(),
                        corresponding_address.addr()
                    );
                    let address = self
                        .xmr_addrs
                        .remove(&get_swap_id(&source)?)
                        .expect("checked at the start of a swap");

                    endpoints.send_to(
                        ServiceBus::Ctl,
                        ServiceId::Wallet,
                        ServiceId::Database,
                        BusMsg::Ctl(CtlMsg::SetAddressSecretKey(AddressSecretKey::Monero {
                            address: corresponding_address,
                            spend: keypair.spend.as_bytes().try_into().unwrap(),
                            view: keypair.view.as_bytes().try_into().unwrap(),
                        })),
                    )?;

                    let sweep_keys = SweepMoneroAddress {
                        source_view_key: view,
                        source_spend_key: spend,
                        destination_address: address,
                        minimum_balance: pub_offer.offer.accordant_amount,
                    };
                    endpoints.send_to(
                        ServiceBus::Ctl,
                        ServiceId::Wallet,
                        source,
                        BusMsg::Ctl(CtlMsg::SweepAddress(SweepAddressAddendum::Monero(
                            sweep_keys,
                        ))),
                    )?;
                }
            }

            CtlMsg::Tx(Tx::Refund(refund_tx)) => {
                let swap_id = get_swap_id(&source)?;
                if let Some(Wallet::Alice(AliceState {
                    alice,
                    local_params,
                    key_manager,
                    remote_params: Some(bob_params), //remote
                    adaptor_refund: Some(adaptor_refund),
                    pub_offer,
                    ..
                })) = self.wallets.get_mut(&swap_id)
                {
                    let sk_b_btc = alice.recover_accordant_key(
                        key_manager,
                        bob_params,
                        adaptor_refund.clone(),
                        refund_tx,
                    );
                    let mut sk_b_btc_buf: Vec<u8> = (*sk_b_btc.as_ref()).into();
                    sk_b_btc_buf.reverse();
                    let sk_b = monero::PrivateKey::from_slice(sk_b_btc_buf.as_ref())
                        .expect("Valid Monero Private Key");
                    info!(
                        "{} | Extracted monero key from Refund tx: {}",
                        swap_id.swap_id(),
                        sk_b.label()
                    );

                    let sk_a = key_manager.get_or_derive_monero_spend_key()?;
                    let spend = sk_a + sk_b;
                    info!(
                        "{} | Full secret monero spending key: {}",
                        swap_id.swap_id(),
                        spend.bright_green_bold()
                    );

                    let view_key_bob = *bob_params
                        .accordant_shared_keys
                        .clone()
                        .into_iter()
                        .find(|vk| vk.tag() == &SharedKeyId::new(SHARED_VIEW_KEY_ID))
                        .unwrap()
                        .elem();

                    let view_key_alice = *local_params
                        .accordant_shared_keys
                        .clone()
                        .into_iter()
                        .find(|vk| vk.tag() == &SharedKeyId::new(SHARED_VIEW_KEY_ID))
                        .unwrap()
                        .elem();
                    let view = view_key_alice + view_key_bob;
                    info!(
                        "{} | Full secret monero view key: {}",
                        swap_id.swap_id(),
                        view.bright_green_bold()
                    );
                    let network = pub_offer.offer.network.into();
                    let keypair = monero::KeyPair { view, spend };
                    let corresponding_address = monero::Address::from_keypair(network, &keypair);
                    info!(
                        "{} | Corresponding address: {}",
                        swap_id.swap_id(),
                        corresponding_address.addr()
                    );
                    let address = self
                        .xmr_addrs
                        .remove(&get_swap_id(&source)?)
                        .expect("checked at the start of a swap");

                    endpoints.send_to(
                        ServiceBus::Ctl,
                        ServiceId::Wallet,
                        ServiceId::Database,
                        BusMsg::Ctl(CtlMsg::SetAddressSecretKey(AddressSecretKey::Monero {
                            address: corresponding_address,
                            spend: keypair.spend.as_bytes().try_into().unwrap(),
                            view: keypair.view.as_bytes().try_into().unwrap(),
                        })),
                    )?;

                    let sweep_keys = SweepMoneroAddress {
                        source_view_key: view,
                        source_spend_key: spend,
                        destination_address: address,
                        minimum_balance: pub_offer.offer.accordant_amount,
                    };
                    endpoints.send_to(
                        ServiceBus::Ctl,
                        ServiceId::Wallet,
                        source,
                        BusMsg::Ctl(CtlMsg::SweepAddress(SweepAddressAddendum::Monero(
                            sweep_keys,
                        ))),
                    )?;
                } else {
                    error!("Call to refund transaction expects an Alice wallet");
                }
            }

            CtlMsg::GetKeys(GetKeys(wallet_token)) => {
                if wallet_token != self.wallet_token {
                    return Err(Error::InvalidToken);
                }
                trace!("sent Secret request to farcasterd");
                endpoints.send_to(
                    ServiceBus::Ctl,
                    ServiceId::Wallet,
                    ServiceId::Farcasterd,
                    BusMsg::Ctl(CtlMsg::Keys(Keys(
                        self.node_secrets.peerd_secret_key,
                        self.node_secrets.node_id(),
                    ))),
                )?;
            }

            CtlMsg::GetSweepBitcoinAddress(source_address) => {
                let swap_id = get_swap_id(&source)?;
                if let Some(Wallet::Bob(BobState {
                    key_manager, bob, ..
                })) = self.wallets.get_mut(&swap_id)
                {
                    let source_secret_key =
                        key_manager.get_or_derive_bitcoin_key(ArbitratingKeyId::Lock)?;
                    let destination_address = bob.refund_address.clone();
                    endpoints.send_to(
                        ServiceBus::Ctl,
                        ServiceId::Wallet,
                        source,
                        BusMsg::Ctl(CtlMsg::SweepAddress(SweepAddressAddendum::Bitcoin(
                            SweepBitcoinAddress {
                                source_secret_key,
                                source_address,
                                destination_address,
                            },
                        ))),
                    )?;
                } else {
                    error!("get funding key requires a bob wallet")
                }
            }

            CtlMsg::SwapOutcome(success) => {
                let swap_id = get_swap_id(&source)?;
                let success = match success {
                    Outcome::Buy => success.bright_green_bold(),
                    _ => success.err(),
                };
                info!(
                    "{} | {} in swap, cleaning up data",
                    swap_id.swap_id(),
                    &success,
                );
                self.clean_up_after_swap(&swap_id);
            }

            CtlMsg::Checkpoint(Checkpoint { swap_id, state }) => match state {
                CheckpointState::CheckpointWallet(CheckpointWallet { wallet, xmr_addr }) => {
                    info!("{} | Restoring wallet for swap", swap_id.swap_id());
                    if !self.wallets.contains_key(&swap_id) {
                        self.wallets.insert(swap_id, wallet);
                    } else {
                        warn!("{} | Did not restore full wallet, a wallet with this key already exists.", swap_id.swap_id());
                    }
                    self.xmr_addrs.insert(swap_id, xmr_addr);
                }
                s => {
                    error!(
                        "{} | Checkpoint {} not supported in walletd",
                        swap_id.swap_id(),
                        s
                    );
                }
            },

            req => {
                error!(
                    "BusMsg {} is not supported by the CTL interface",
                    req.to_string()
                );
            }
        }

        Ok(())
    }

    fn process_peer_msg_with_wallet(
        &mut self,
        request: PeerMsg,
        runtime: Runtime,
        endpoints: &mut Endpoints,
        swap_id: SwapId,
    ) -> Result<(), Error> {
        match request {
            PeerMsg::MakerCommit(commit) => {
                match commit {
                    Commit::BobParameters(_) => {
                        if let Wallet::Alice(AliceState {
                            remote_commit, // None
                            ..
                        }) = self
                        {
                            if remote_commit.is_some() {
                                error!("{} | Bob commit (remote) already set", swap_id.swap_id(),);
                            } else if let Commit::BobParameters(commit) = commit {
                                trace!("Setting bob commit");
                                *remote_commit = Some(commit);
                            }
                        } else {
                            error!(
                                "{} | Wallet not found or not on correct state",
                                swap_id.swap_id(),
                            );
                            return Ok(());
                        }
                    }
                    Commit::AliceParameters(_) => {
                        if let Wallet::Bob(BobState {
                            remote_commit_params, // None
                            ..
                        }) = self
                        {
                            if remote_commit_params.is_some() {
                                error!("{} | Alice commit (remote) already set", swap_id.swap_id(),);
                            } else if let Commit::AliceParameters(commit) = commit {
                                trace!("Setting alice commit");
                                *remote_commit_params = Some(commit);
                            }
                        } else {
                            error!(
                                "{} | Wallet not found or not on correct state",
                                swap_id.swap_id(),
                            );
                            return Ok(());
                        }
                    }
                }
                let proof = match self {
                    Wallet::Alice(AliceState { local_params, .. }) => local_params.proof.as_ref(),
                    Wallet::Bob(BobState { local_params, .. }) => local_params.proof.as_ref(),
                };
                endpoints.send_to(
                    ServiceBus::Msg,
                    ServiceId::Wallet,
                    ServiceId::Swap(swap_id),
                    BusMsg::P2p(PeerMsg::Reveal(Reveal::Proof(RevealProof {
                        swap_id,
                        proof: proof.expect("local proof is always Some").clone(),
                    }))),
                )?;
            }

            PeerMsg::Reveal(Reveal::Proof(reveal)) => match self {
                Wallet::Alice(AliceState { remote_proof, .. }) => {
                    *remote_proof = Some(reveal.proof)
                }
                Wallet::Bob(BobState { remote_proof, .. }) => *remote_proof = Some(reveal.proof),
            },

            PeerMsg::Reveal(reveal) => {
                match reveal {
                    // receiving from counterparty Bob, thus I'm Alice (Maker or Taker)
                    Reveal::BobParameters(reveal) => {
                        if let Wallet::Alice(AliceState {
                            local_params,
                            key_manager,
                            pub_offer,
                            remote_commit: Some(bob_commit),
                            remote_params,                 // None
                            remote_proof: Some(bob_proof), // Should be Some() at this stage
                            ..
                        }) = self
                        {
                            if remote_params.is_some() {
                                error!("{} | Bob params already set", swap_id.swap_id(),);
                                return Ok(());
                            }

                            trace!("Setting bob params: {}", reveal);
                            bob_commit.verify_with_reveal(&CommitmentEngine, reveal.clone())?;
                            let remote_params_candidate: Parameters = reveal.into_parameters();
                            let proof_verification = key_manager.verify_proof(
                                &remote_params_candidate.spend,
                                &remote_params_candidate.adaptor,
                                bob_proof.clone(), /* remote_params_candidate.proof.
                                                    * clone(), */
                            );
                            if proof_verification.is_err() {
                                error!("{} | DLEQ proof invalid", swap_id.swap_id());
                                return Ok(());
                            }
                            info!("{} | Proof successfully verified", swap_id.swap_id());
                            *remote_params = Some(remote_params_candidate);
                            // if we're maker, send Ctl RevealProof to counterparty
                            if pub_offer.swap_role(&TradeRole::Maker) == SwapRole::Alice {
                                endpoints.send_to(
                                    ServiceBus::Msg,
                                    ServiceId::Wallet,
                                    ServiceId::Swap(swap_id),
                                    BusMsg::P2p(PeerMsg::Reveal(Reveal::Proof(RevealProof {
                                        swap_id,
                                        proof: local_params
                                            .proof
                                            .clone()
                                            .expect("local proof is always Some"),
                                    }))),
                                )?;
                            }
                            // nothing to do yet, waiting for Msg
                            // CoreArbitratingSetup to proceed
                        } else {
                            error!("{} | only Wallet::Alice", swap_id.swap_id(),);
                        }
                        return Ok(());
                    }
                    // getting parameters from counterparty alice routed through
                    // swapd, thus I'm Bob on this swap: Bob can proceed
                    Reveal::AliceParameters(reveal) => {
                        if let Wallet::Bob(BobState {
                            bob,
                            local_trade_role,
                            local_params,
                            key_manager,
                            pub_offer,
                            funding_tx: Some(funding_tx),
                            remote_commit_params,
                            remote_params,                    // None
                            remote_proof: Some(remote_proof), // Some
                            core_arb_setup,                   // None
                            adaptor_buy,
                        }) = self
                        {
                            // set wallet params
                            if remote_params.is_some() {
                                error!("{} | Alice params already set", swap_id.swap_id(),);
                                return Ok(());
                            }

                            trace!("Setting remote params: {}", reveal);
                            let remote_params_candidate: Parameters = reveal.into_parameters();
                            let proof_verification = key_manager.verify_proof(
                                &remote_params_candidate.spend,
                                &remote_params_candidate.adaptor,
                                remote_proof.clone(),
                            );

                            if proof_verification.is_err() {
                                error!("{} | DLEQ proof invalid", swap_id.swap_id());
                                return Ok(());
                            }
                            info!("{} | Proof successfully verified", swap_id.swap_id());
                            *remote_params = Some(remote_params_candidate);

                            // if we're maker, send Ctl RevealProof to counterparty
                            if pub_offer.swap_role(&TradeRole::Maker) == SwapRole::Bob {
                                endpoints.send_to(
                                    ServiceBus::Msg,
                                    ServiceId::Wallet,
                                    ServiceId::Swap(swap_id),
                                    BusMsg::P2p(PeerMsg::Reveal(Reveal::Proof(RevealProof {
                                        swap_id,
                                        proof: local_params
                                            .proof
                                            .clone()
                                            .expect("local proof is always Some"),
                                    }))),
                                )?;
                            }

                            // checkpoint here after proof verification and potentially sending RevealProof
                            debug!("checkpointing bob pre lock.");
                            checkpoint_send(
                                endpoints,
                                swap_id,
                                ServiceId::Wallet,
                                ServiceId::Database,
                                CheckpointState::CheckpointWallet(CheckpointWallet {
                                    xmr_addr: *self
                                        .xmr_addrs
                                        .get(&swap_id)
                                        .expect("checked at start of swap"),
                                    wallet: self.clone(),
                                }),
                            )?;

                            // set wallet core_arb_txs
                            if core_arb_setup.is_some() {
                                error!("{} | Core Arb Txs already set", swap_id.swap_id(),);
                                return Ok(());
                            }
                            if !funding_tx.was_seen() {
                                error!("{} | Funding not yet seen", swap_id.swap_id());
                                return Ok(());
                            }
                            // FIXME should be set before
                            let core_arbitrating_txs = bob.core_arbitrating_transactions(
                                &remote_params.clone().expect("alice_params set above"),
                                local_params,
                                funding_tx.clone(),
                                pub_offer.to_arbitrating_params(),
                            )?;
                            let cosign_arbitrating_cancel =
                                bob.cosign_arbitrating_cancel(key_manager, &core_arbitrating_txs)?;
                            *core_arb_setup = Some(
                                core_arbitrating_txs
                                    .into_arbitrating_setup(swap_id, cosign_arbitrating_cancel),
                            );
                            let core_arb_setup_msg =
                                PeerMsg::CoreArbitratingSetup(core_arb_setup.clone().unwrap());
                            endpoints.send_to(
                                ServiceBus::Msg,
                                ServiceId::Wallet,
                                ServiceId::Swap(swap_id),
                                BusMsg::P2p(core_arb_setup_msg),
                            )?;
                        } else {
                            error!("{} | only Wallet::Bob", swap_id.swap_id());
                        }
                    }
                    _ => {
                        unreachable!("pattern matched above with PeerMsg::Reveal(Reveal::Proof(_))")
                    }
                }
            }

            PeerMsg::RefundProcedureSignatures(RefundProcedureSignatures {
                swap_id: _,
                cancel_sig: alice_cancel_sig,
                refund_adaptor_sig,
            }) => {
                if let Wallet::Bob(BobState {
                    bob,
                    local_params,
                    key_manager,
                    pub_offer,
                    remote_params: Some(remote_params),
                    core_arb_setup: Some(core_arb_setup),
                    adaptor_buy, // None
                    funding_tx,
                    local_trade_role,
                    remote_commit_params,
                    remote_proof,
                }) = self
                {
                    let core_arb_txs = core_arb_setup.clone().into_arbitrating_tx();

                    bob.validate_adaptor_refund(
                        key_manager,
                        remote_params,
                        local_params,
                        &core_arb_txs,
                        &refund_adaptor_sig,
                    )?;

                    if adaptor_buy.is_some() {
                        error!("{} | adaptor_buy already set", swap_id.swap_id());
                        return Ok(());
                    }
                    *adaptor_buy = Some(bob.sign_adaptor_buy(
                        swap_id,
                        key_manager,
                        remote_params,
                        local_params,
                        &core_arb_txs,
                        pub_offer.to_arbitrating_params(),
                    )?);

                    debug!("checkpointing bob pre buy sig.");
                    checkpoint_send(
                        endpoints,
                        swap_id,
                        ServiceId::Wallet,
                        ServiceId::Database,
                        CheckpointState::CheckpointWallet(CheckpointWallet {
                            xmr_addr: *self
                                .xmr_addrs
                                .get(&swap_id)
                                .expect("checked at start of swap"),
                            wallet: self.clone(),
                        }),
                    )?;

                    {
                        // lock
                        let sig = bob.sign_arbitrating_lock(key_manager, &core_arb_txs)?;
                        let tx = core_arb_setup.lock.clone();
                        let mut lock_tx = LockTx::from_partial(tx);
                        let lock_pubkey = key_manager.get_pubkey(ArbitratingKeyId::Lock)?;
                        lock_tx.add_witness(lock_pubkey, sig)?;
                        let finalized_lock_tx =
                            Broadcastable::<bitcoin::Transaction>::finalize_and_extract(
                                &mut lock_tx,
                            )?;

                        endpoints.send_to(
                            ServiceBus::Ctl,
                            ServiceId::Wallet,
                            ServiceId::Swap(swap_id),
                            BusMsg::Ctl(CtlMsg::Tx(Tx::Lock(finalized_lock_tx))),
                        )?;
                    }

                    {
                        // cancel
                        let tx = core_arb_setup.cancel.clone();
                        let mut cancel_tx = CancelTx::from_partial(tx);
                        cancel_tx.add_witness(remote_params.cancel, alice_cancel_sig)?;
                        cancel_tx.add_witness(local_params.cancel, core_arb_setup.cancel_sig)?;
                        let finalized_cancel_tx =
                            Broadcastable::<bitcoin::Transaction>::finalize_and_extract(
                                &mut cancel_tx,
                            )?;
                        endpoints.send_to(
                            ServiceBus::Ctl,
                            ServiceId::Wallet,
                            ServiceId::Swap(swap_id),
                            BusMsg::Ctl(CtlMsg::Tx(Tx::Cancel(finalized_cancel_tx))),
                        )?;
                    }

                    {
                        // refund
                        let TxSignatures { sig, adapted_sig } =
                            bob.fully_sign_refund(key_manager, &core_arb_txs, &refund_adaptor_sig)?;
                        let tx = core_arb_setup.refund.clone();
                        let mut refund_tx = RefundTx::from_partial(tx);

                        refund_tx.add_witness(local_params.refund, sig)?;
                        refund_tx.add_witness(remote_params.refund, adapted_sig)?;
                        let final_refund_tx =
                            Broadcastable::<bitcoin::Transaction>::finalize_and_extract(
                                &mut refund_tx,
                            )?;
                        endpoints.send_to(
                            ServiceBus::Ctl,
                            ServiceId::Wallet,
                            ServiceId::Swap(swap_id),
                            BusMsg::Ctl(CtlMsg::Tx(Tx::Refund(final_refund_tx))),
                        )?;
                    }

                    {
                        let buy_proc_sig =
                            PeerMsg::BuyProcedureSignature(adaptor_buy.clone().unwrap());
                        endpoints.send_to(
                            ServiceBus::Msg,
                            ServiceId::Wallet,
                            ServiceId::Swap(swap_id),
                            BusMsg::P2p(buy_proc_sig),
                        )?;
                    }
                } else {
                    error!("{} | Unknown wallet and swap id", swap_id.swap_id(),);
                }
            }

            PeerMsg::CoreArbitratingSetup(core_arbitrating_setup) => {
                if let Wallet::Alice(AliceState {
                    alice,
                    local_params,
                    key_manager,
                    pub_offer,
                    remote_params: Some(bob_parameters),
                    core_arb_setup,         // None
                    alice_cancel_signature, // None
                    adaptor_refund,         // None
                    local_trade_role,
                    remote_commit,
                    remote_proof,
                }) = self
                {
                    if core_arb_setup.is_some() {
                        error!("{} | core_arb_txs already set for alice", swap_id.swap_id(),);
                        return Ok(());
                    }
                    if alice_cancel_signature.is_some() {
                        error!(
                            "{} | alice_cancel_sig already set for alice",
                            swap_id.swap_id(),
                        );
                        return Ok(());
                    }
                    *core_arb_setup = Some(core_arbitrating_setup.clone());
                    let core_arb_txs = core_arbitrating_setup.into_arbitrating_tx();
                    let signed_adaptor_refund = alice.sign_adaptor_refund(
                        key_manager,
                        local_params,
                        bob_parameters,
                        &core_arb_txs,
                        pub_offer.to_arbitrating_params(),
                    )?;
                    *adaptor_refund = Some(signed_adaptor_refund.clone());
                    let cosigned_arb_cancel = alice.cosign_arbitrating_cancel(
                        key_manager,
                        local_params,
                        bob_parameters,
                        &core_arb_txs,
                        pub_offer.to_arbitrating_params(),
                    )?;
                    let refund_proc_signatures = RefundProcedureSignatures {
                        swap_id,
                        cancel_sig: cosigned_arb_cancel,
                        refund_adaptor_sig: signed_adaptor_refund,
                    };
                    *alice_cancel_signature = Some(refund_proc_signatures.cancel_sig);

                    debug!("checkpointing alice pre lock.");
                    checkpoint_send(
                        endpoints,
                        swap_id,
                        ServiceId::Wallet,
                        ServiceId::Database,
                        CheckpointState::CheckpointWallet(CheckpointWallet {
                            xmr_addr: *self
                                .xmr_addrs
                                .get(&swap_id)
                                .expect("checked at start of swap"),
                            wallet: self.clone(),
                        }),
                    )?;

                    // NOTE: if this is the right spot for the Ctl message, it should also be replayed upon state recovery
                    {
                        // cancel
                        let partial_cancel_tx = core_arb_setup.as_ref().unwrap().cancel.clone();
                        let mut cancel_tx = CancelTx::from_partial(partial_cancel_tx);
                        cancel_tx
                            .add_witness(local_params.cancel, alice_cancel_signature.unwrap())?;
                        cancel_tx.add_witness(
                            bob_parameters.cancel,
                            core_arb_setup.as_ref().unwrap().cancel_sig,
                        )?;
                        let finalized_cancel_tx =
                            Broadcastable::<bitcoin::Transaction>::finalize_and_extract(
                                &mut cancel_tx,
                            )?;
                        endpoints.send_to(
                            ServiceBus::Ctl,
                            ServiceId::Wallet,
                            ServiceId::Swap(swap_id),
                            BusMsg::Ctl(CtlMsg::Tx(Tx::Cancel(finalized_cancel_tx))),
                        )?;
                    }
                    // NOTE: if this is the right spot for the Ctl message, it should also be replayed upon state recovery
                    {
                        let FullySignedPunish { punish, punish_sig } = alice.fully_sign_punish(
                            key_manager,
                            local_params,
                            bob_parameters,
                            &core_arb_txs,
                            pub_offer.to_arbitrating_params(),
                        )?;

                        let mut punish_tx = PunishTx::from_partial(punish);
                        punish_tx.add_witness(
                            local_params.punish.expect("Alice has punish key"),
                            punish_sig,
                        )?;
                        let tx = Broadcastable::<bitcoin::Transaction>::finalize_and_extract(
                            &mut punish_tx,
                        )?;
                        endpoints.send_to(
                            ServiceBus::Ctl,
                            ServiceId::Wallet,
                            ServiceId::Swap(swap_id),
                            BusMsg::Ctl(CtlMsg::Tx(Tx::Punish(tx))),
                        )?;
                    }

                    let refund_proc_signatures =
                        PeerMsg::RefundProcedureSignatures(refund_proc_signatures);

                    endpoints.send_to(
                        ServiceBus::Msg,
                        ServiceId::Wallet,
                        ServiceId::Swap(swap_id),
                        BusMsg::P2p(refund_proc_signatures),
                    )?
                } else {
                    error!("{} | only Wallet::Alice", swap_id.swap_id(),);
                }
            }

            PeerMsg::BuyProcedureSignature(buy_proc_sig) => {
                let BuyProcedureSignature { swap_id, .. } = buy_proc_sig;
                trace!("wallet received buyproceduresignature");
                debug!("checkpointing alice pre buy sig.");
                if let Wallet::Alice(state) = self {
                    checkpoint_send(
                        endpoints,
                        swap_id,
                        ServiceId::Wallet,
                        ServiceId::Database,
                        CheckpointState::CheckpointWallet(CheckpointWallet {
                            xmr_addr: *self
                                .xmr_addrs
                                .get(&swap_id)
                                .expect("checked at start of swap"),
                            wallet: self.clone(),
                        }),
                    )?;
                } else {
                    error!("{} | Unknown wallet and swap id", swap_id.swap_id(),);
                };

                if let Wallet::Alice(AliceState {
                    alice,
                    local_params: alice_params,
                    key_manager,
                    pub_offer,
                    remote_params: Some(bob_parameters),
                    core_arb_setup: Some(core_arb_setup),
                    alice_cancel_signature: Some(alice_cancel_sig),
                    ..
                }) = self
                {
                    let core_arb_txs = core_arb_setup.clone().into_arbitrating_tx();

                    // cancel
                    let tx = core_arb_setup.cancel.clone();
                    let mut cancel_tx = CancelTx::from_partial(tx);
                    cancel_tx.add_witness(alice_params.cancel, *alice_cancel_sig)?;
                    cancel_tx.add_witness(bob_parameters.cancel, core_arb_setup.cancel_sig)?;
                    let finalized_cancel_tx =
                        Broadcastable::<bitcoin::Transaction>::finalize_and_extract(
                            &mut cancel_tx,
                        )?;
                    endpoints.send_to(
                        ServiceBus::Ctl,
                        ServiceId::Wallet,
                        ServiceId::Swap(swap_id),
                        BusMsg::Ctl(CtlMsg::Tx(Tx::Cancel(finalized_cancel_tx))),
                    )?;

                    // buy
                    let mut buy_tx = BuyTx::from_partial(buy_proc_sig.buy.clone());
                    alice.validate_adaptor_buy(
                        key_manager,
                        alice_params,
                        bob_parameters,
                        &core_arb_txs,
                        pub_offer.to_arbitrating_params(),
                        &buy_proc_sig,
                    )?;
                    let TxSignatures { sig, adapted_sig } = alice.fully_sign_buy(
                        key_manager,
                        alice_params,
                        bob_parameters,
                        &core_arb_txs,
                        pub_offer.to_arbitrating_params(),
                        &buy_proc_sig,
                    )?;
                    buy_tx.add_witness(key_manager.get_pubkey(ArbitratingKeyId::Buy)?, sig)?;
                    buy_tx.add_witness(bob_parameters.buy, adapted_sig)?;
                    let tx =
                        Broadcastable::<bitcoin::Transaction>::finalize_and_extract(&mut buy_tx)?;
                    trace!("wallet sends fullysignedbuy");
                    endpoints.send_to(
                        ServiceBus::Ctl,
                        ServiceId::Wallet,
                        ServiceId::Swap(swap_id),
                        BusMsg::Ctl(CtlMsg::Tx(Tx::Buy(tx))),
                    )?;

                    // buy_adaptor_sig
                } else {
                    error!("{} | could not get alice's wallet", swap_id.swap_id(),)
                }
            }
            req => {
                error!("PeerMsg {} is not supported by wallet", req.to_string());
            }
        }
        Ok(())
    }
}
