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

use clap::{AppSettings, Clap, ValueHint};

use bitcoin::hashes::hex::FromHex;
use farcaster_chains::pairs::btcxmr::BtcXmr;
use farcaster_core::{negotiation::PublicOffer, role::NegotiationRole};
use internet2::PartialNodeAddr;
use lnp::ChannelId as SwapId;
use std::str::FromStr;
use crate::peerd::KeyOpts;

/// Lightning peer network channel daemon; part of LNP Node
///
/// The daemon is controlled though ZMQ ctl socket (see `ctl-socket` argument
/// description)
#[derive(Clap, Clone, PartialEq, Eq, Debug)]
#[clap(
    name = "swapd",
    bin_name = "swapd",
    author,
    version,
    setting = AppSettings::ColoredHelp
)]
pub struct Opts {
    /// Node key configuration
    #[clap(flatten)]
    pub key_opts: KeyOpts,

    /// Channel id
    #[clap(parse(try_from_str = SwapId::from_hex))]
    pub channel_id: SwapId,

    /// Public offer to initiate swapd runtime
    #[clap(parse(try_from_str = FromStr::from_str))]
    pub public_offer: PublicOffer<BtcXmr>,

    /// Role of participant
    #[clap(parse(try_from_str = FromStr::from_str))]
    pub negotiation_role: NegotiationRole,

    /// These params can be read also from the configuration file, not just
    /// Command-line args or environment variables
    #[clap(flatten)]
    pub shared: crate::opts::Opts,
}

impl Opts {
    pub fn process(&mut self) {
        self.shared.process();
        self.key_opts.process(&self.shared);
    }
}
