// Copyright 2020-2022 Farcaster Devs & LNP/BP Standards Association
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! This module defines the currently supported blockchains in the node for the arbitrating and
//! the accordant roles. The types defined here are convertible with core `Blockchain` type.

use std::convert::TryFrom;
use std::str::FromStr;

use farcaster_core::{blockchain::Blockchain, consensus};
use strict_encoding::{StrictDecode, StrictEncode};

/// A list of supported arbitrating blockchain
#[derive(
    Debug,
    Clone,
    Copy,
    Hash,
    PartialEq,
    Eq,
    Parser,
    Display,
    Serialize,
    Deserialize,
    StrictEncode,
    StrictDecode,
)]
#[serde(crate = "serde_crate")]
#[display(Debug)]
pub enum ArbitratingBlockchain {
    /// The Bitcoin (BTC) blockchain.
    Bitcoin,
}

impl FromStr for ArbitratingBlockchain {
    type Err = consensus::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Bitcoin" | "bitcoin" | "btc" | "BTC" => Ok(ArbitratingBlockchain::Bitcoin),
            _ => Err(consensus::Error::UnknownType),
        }
    }
}

impl Into<Blockchain> for ArbitratingBlockchain {
    fn into(self) -> Blockchain {
        match self {
            ArbitratingBlockchain::Bitcoin => Blockchain::Bitcoin,
        }
    }
}

impl TryFrom<Blockchain> for ArbitratingBlockchain {
    type Error = consensus::Error;

    fn try_from(blockchain: Blockchain) -> Result<Self, Self::Error> {
        match blockchain {
            Blockchain::Bitcoin => Ok(ArbitratingBlockchain::Bitcoin),
            _ => Err(consensus::Error::TypeMismatch),
        }
    }
}

/// A list of supported accordant blockchain
#[derive(
    Debug,
    Clone,
    Copy,
    Hash,
    PartialEq,
    Eq,
    Parser,
    Display,
    Serialize,
    Deserialize,
    StrictEncode,
    StrictDecode,
)]
#[serde(crate = "serde_crate")]
#[display(Debug)]
pub enum AccordantBlockchain {
    /// The Monero (XMR) blockchain.
    Monero,
    // NOTE: we could in theory add Bitcoin here, but currently the node does not support it
}

impl FromStr for AccordantBlockchain {
    type Err = consensus::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Monero" | "monero" | "xmr" | "XMR" => Ok(AccordantBlockchain::Monero),
            _ => Err(consensus::Error::UnknownType),
        }
    }
}

impl Into<Blockchain> for AccordantBlockchain {
    fn into(self) -> Blockchain {
        match self {
            AccordantBlockchain::Monero => Blockchain::Monero,
        }
    }
}

impl TryFrom<Blockchain> for AccordantBlockchain {
    type Error = consensus::Error;

    fn try_from(blockchain: Blockchain) -> Result<Self, Self::Error> {
        match blockchain {
            Blockchain::Monero => Ok(AccordantBlockchain::Monero),
            _ => Err(consensus::Error::TypeMismatch),
        }
    }
}
