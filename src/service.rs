// Copyright 2020-2022 Farcaster Devs & LNP/BP Standards Association
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::bus::ctl::CtlMsg;
use crate::bus::info::InfoMsg;
use crate::bus::BusMsg;
use crate::bus::{Failure, Progress, ServiceBus};
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use bitcoin::hashes::hex::{self, ToHex};
use colored::Colorize;
use internet2::addr::NodeId;
use internet2::{
    addr::{NodeAddr, ServiceAddr},
    zeromq,
    zeromq::ZmqSocketType,
};
use lazy_static::lazy_static;
use microservices::esb;
#[cfg(feature = "node")]
use microservices::node::TryService;
use strict_encoding::{strict_deserialize, strict_serialize};
use strict_encoding::{StrictDecode, StrictEncode};

use farcaster_core::{
    blockchain::{Blockchain, Network},
    swap::SwapId,
};

use crate::opts::Opts;
use crate::Error;

lazy_static! {
    pub static ref ZMQ_CONTEXT: zmq::Context = zmq::Context::new();
}

#[derive(
    Wrapper,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Debug,
    From,
    Default,
    StrictEncode,
    StrictDecode,
)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate")
)]
pub struct ClientName([u8; 32]);

impl Display for ClientName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            write!(
                f,
                "{}..{}",
                self.0[..4].to_hex(),
                self.0[(self.0.len() - 4)..].to_hex()
            )
        } else {
            f.write_str(&String::from_utf8_lossy(&self.0))
        }
    }
}

impl FromStr for ClientName {
    type Err = hex::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut me = Self::default();
        if s.len() > 32 {
            me.0.copy_from_slice(&s.as_bytes()[0..32]);
        } else {
            let mut me = Self::default();
            me.0[0..s.len()].copy_from_slice(s.as_bytes());
        }
        Ok(me)
    }
}

#[derive(
    Clone, Copy, Debug, Display, StrictEncode, StrictDecode, Eq, PartialEq, Ord, PartialOrd, Hash,
)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate")
)]
#[display(Debug)]
pub struct Counter(u32);

impl Counter {
    pub fn new() -> Self {
        Counter(0)
    }

    pub fn increment(&mut self) {
        self.0 = self.0.overflowing_add(1).0;
    }

    pub fn count(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Hash)]
pub struct ServiceConfig {
    /// ZMQ socket for peer-to-peer network message bus
    pub msg_endpoint: ServiceAddr,

    /// ZMQ socket for internal service control bus
    pub ctl_endpoint: ServiceAddr,

    /// ZMQ socket for internal info service bus
    pub info_endpoint: ServiceAddr,

    /// ZMQ socket for syncer events bus
    pub sync_endpoint: ServiceAddr,
}

#[cfg(feature = "shell")]
impl From<Opts> for ServiceConfig {
    fn from(opts: Opts) -> Self {
        ServiceConfig {
            msg_endpoint: opts.msg_socket,
            ctl_endpoint: opts.ctl_socket,
            info_endpoint: opts.info_socket,
            sync_endpoint: opts.sync_socket,
        }
    }
}

/// Identifiers of daemons participating in Farcaster Node
#[derive(Clone, PartialEq, Eq, Hash, Debug, Display, From, StrictEncode, StrictDecode)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate")
)]
pub enum ServiceId {
    #[display("loopback")]
    Loopback,

    #[display("farcasterd")]
    Farcasterd,

    #[display("peerd<{0} {1}>")]
    Peer(u128, NodeAddr),

    #[display("swap<{0}>")]
    #[from]
    Swap(SwapId),

    #[display("client<{0}>")]
    Client(u64),

    #[display("{0} ({1}) syncer")]
    Syncer(Blockchain, Network),

    #[display("walletd")]
    Wallet,

    #[display("grpcd")]
    Grpcd,

    #[display("grpcd_client<{0}>")]
    GrpcdClient(u32),

    #[display("databased")]
    Database,

    #[display("other<{0}>")]
    Other(ClientName),
}

impl ServiceId {
    pub fn router() -> ServiceId {
        ServiceId::Farcasterd
    }

    pub fn client() -> ServiceId {
        use bitcoin::secp256k1::rand;
        ServiceId::Client(rand::random())
    }

    pub fn node_id(&self) -> Option<NodeId> {
        if let ServiceId::Peer(_, addr) = self {
            Some(addr.id)
        } else {
            None
        }
    }

    pub fn node_addr(&self) -> Option<NodeAddr> {
        if let ServiceId::Peer(_, addr) = self {
            Some(*addr)
        } else {
            None
        }
    }

    pub fn dummy_peer_service_id(node_addr: NodeAddr) -> ServiceId {
        ServiceId::Peer(0, node_addr)
    }
}

impl esb::ServiceAddress for ServiceId {}

impl From<ServiceId> for Vec<u8> {
    fn from(daemon_id: ServiceId) -> Self {
        strict_serialize(&daemon_id).expect("Memory-based encoding does not fail")
    }
}

impl From<Vec<u8>> for ServiceId {
    fn from(vec: Vec<u8>) -> Self {
        strict_deserialize(&vec).unwrap_or_else(|_| {
            ServiceId::Other(
                ClientName::from_str(&String::from_utf8_lossy(&vec))
                    .expect("ClientName conversion never fails"),
            )
        })
    }
}

pub struct Service<Runtime>
where
    Runtime: esb::Handler<ServiceBus, Request = BusMsg>,
    esb::Error<ServiceId>: From<Runtime::Error>,
{
    esb: esb::Controller<ServiceBus, BusMsg, Runtime>,
    broker: bool,
}

impl<Runtime> Service<Runtime>
where
    Runtime: esb::Handler<ServiceBus, Request = BusMsg>,
    esb::Error<ServiceId>: From<Runtime::Error>,
{
    #[cfg(feature = "node")]
    pub fn run(config: ServiceConfig, runtime: Runtime, broker: bool) -> Result<(), Error> {
        let service = Self::with(config, runtime, broker)?;
        service.run_loop()?;
        unreachable!()
    }

    fn with(
        config: ServiceConfig,
        runtime: Runtime,
        broker: bool,
    ) -> Result<Self, esb::Error<ServiceId>> {
        let router = if !broker {
            Some(ServiceId::router())
        } else {
            None
        };
        let api_type = if broker {
            ZmqSocketType::RouterBind
        } else {
            ZmqSocketType::RouterConnect
        };
        let services = map! {
            ServiceBus::Msg => esb::BusConfig::with_addr(
                config.msg_endpoint,
                api_type,
                router.clone()
            ),
            ServiceBus::Ctl => esb::BusConfig::with_addr(
                config.ctl_endpoint,
                api_type,
                router.clone()
            ),
            ServiceBus::Info => esb::BusConfig::with_addr(
                config.info_endpoint,
                api_type,
                router.clone()
            ),
            ServiceBus::Sync => esb::BusConfig::with_addr(
                config.sync_endpoint,
                api_type,
                router
            )
        };

        let esb = esb::Controller::with(services, runtime)?;
        Ok(Self { esb, broker })
    }

    pub fn broker(config: ServiceConfig, runtime: Runtime) -> Result<Self, esb::Error<ServiceId>> {
        Self::with(config, runtime, true)
    }

    #[allow(clippy::self_named_constructors)]
    pub fn service(config: ServiceConfig, runtime: Runtime) -> Result<Self, esb::Error<ServiceId>> {
        Self::with(config, runtime, false)
    }

    pub fn is_broker(&self) -> bool {
        self.broker
    }

    pub fn add_bridge_service_bus(
        &mut self,
        socket: zmq::Socket,
    ) -> Result<(), esb::Error<ServiceId>> {
        self.esb.add_service_bus(
            ServiceBus::Bridge,
            esb::BusConfig {
                // apparently this type is eventually ignored
                api_type: ZmqSocketType::Push,
                carrier: zeromq::Carrier::Socket(socket),
                router: None,
                queued: true,
                topic: None,
            },
        )
    }

    #[cfg(feature = "node")]
    pub fn run_loop(mut self) -> Result<(), Error> {
        let identity = self.esb.handler().identity();
        if !self.is_broker() {
            std::thread::sleep(core::time::Duration::from_secs(1));
            self.esb.send_to(
                ServiceBus::Ctl,
                ServiceId::Farcasterd,
                BusMsg::Ctl(CtlMsg::Hello),
            )?;
        } else if identity != ServiceId::Farcasterd {
            warn!(
                "Not saying hello to Farcasterd: service {} is broker",
                identity
            );
        }

        debug!(
            "New service {} with PID {} started",
            identity,
            std::process::id()
        );

        self.esb.run_or_panic(&identity.to_string());

        unreachable!()
    }
}

pub type Endpoints = esb::EndpointList<ServiceBus>;

pub trait TryToServiceId {
    fn try_to_service_id(&self) -> Option<ServiceId>;
}

impl TryToServiceId for ServiceId {
    fn try_to_service_id(&self) -> Option<ServiceId> {
        Some(self.clone())
    }
}

impl TryToServiceId for &Option<ServiceId> {
    fn try_to_service_id(&self) -> Option<ServiceId> {
        (*self).clone()
    }
}

impl TryToServiceId for Option<ServiceId> {
    fn try_to_service_id(&self) -> Option<ServiceId> {
        self.clone()
    }
}

pub trait Reporter
where
    Self: esb::Handler<ServiceBus>,
    esb::Error<ServiceId>: From<Self::Error>,
{
    fn report_to(&self) -> Option<ServiceId>;

    fn report_success_message(
        &mut self,
        endpoints: &mut Endpoints,
        msg: Option<impl ToString>,
    ) -> Result<(), Error> {
        if let Some(dest) = self.report_to() {
            endpoints.send_to(
                ServiceBus::Ctl,
                self.identity(),
                dest,
                BusMsg::Ctl(CtlMsg::Success(msg.map(|m| m.to_string()).into())),
            )?;
        }
        Ok(())
    }

    fn report_progress(
        &mut self,
        endpoints: &mut Endpoints,
        progress: Progress,
    ) -> Result<(), Error> {
        if let Some(dest) = self.report_to() {
            endpoints.send_to(
                ServiceBus::Ctl,
                self.identity(),
                dest,
                BusMsg::Ctl(CtlMsg::Progress(progress)),
            )?;
        }
        Ok(())
    }

    fn report_progress_message(
        &mut self,
        endpoints: &mut Endpoints,
        msg: impl ToString,
    ) -> Result<(), Error> {
        if let Some(dest) = self.report_to() {
            endpoints.send_to(
                ServiceBus::Ctl,
                self.identity(),
                dest,
                BusMsg::Ctl(CtlMsg::Progress(Progress::Message(msg.to_string()))),
            )?;
        }
        Ok(())
    }

    fn report_failure(&mut self, endpoints: &mut Endpoints, failure: Failure) -> Error {
        if let Some(dest) = self.report_to() {
            // Even if we fail, we still have to terminate :)
            let _ = endpoints.send_to(
                ServiceBus::Ctl,
                self.identity(),
                dest,
                BusMsg::Ctl(CtlMsg::Failure(failure.clone())),
            );
        }
        Error::Terminate(failure.to_string())
    }
}

pub trait CtlServer
where
    Self: esb::Handler<ServiceBus>,
    esb::Error<ServiceId>: From<Self::Error>,
{
    fn send_ctl(
        &mut self,
        endpoints: &mut Endpoints,
        dest: impl TryToServiceId,
        request: BusMsg,
    ) -> Result<(), Error> {
        if let Some(dest) = dest.try_to_service_id() {
            endpoints.send_to(ServiceBus::Ctl, self.identity(), dest, request)?;
        }
        Ok(())
    }

    fn send_client_ctl(
        &mut self,
        endpoints: &mut Endpoints,
        dest: ServiceId,
        request: CtlMsg,
    ) -> Result<(), Error> {
        let bus = ServiceBus::Ctl;
        if let ServiceId::GrpcdClient(_) = dest {
            endpoints.send_to(bus, dest, ServiceId::Grpcd, BusMsg::Ctl(request))?;
        } else {
            endpoints.send_to(bus, self.identity(), dest, BusMsg::Ctl(request))?;
        }
        Ok(())
    }

    fn send_client_info(
        &mut self,
        endpoints: &mut Endpoints,
        dest: ServiceId,
        request: InfoMsg,
    ) -> Result<(), Error> {
        let bus = ServiceBus::Info;
        if let ServiceId::GrpcdClient(_) = dest {
            endpoints.send_to(bus, dest, ServiceId::Grpcd, BusMsg::Info(request))?;
        } else {
            endpoints.send_to(bus, self.identity(), dest, BusMsg::Info(request))?;
        }
        Ok(())
    }
}

pub trait LogStyle: ToString {
    fn bright_blue_bold(&self) -> colored::ColoredString {
        self.to_string().bold().bright_blue()
    }

    fn bright_blue_italic(&self) -> colored::ColoredString {
        self.to_string().italic().bright_blue()
    }

    fn green_bold(&self) -> colored::ColoredString {
        self.to_string().bold().green()
    }

    fn red_bold(&self) -> colored::ColoredString {
        self.to_string().bold().red()
    }

    fn bright_green_bold(&self) -> colored::ColoredString {
        self.to_string().bold().bright_green()
    }

    fn bright_green_italic(&self) -> colored::ColoredString {
        self.to_string().italic().bright_green()
    }

    fn bright_yellow_italic(&self) -> colored::ColoredString {
        self.to_string().italic().bright_yellow()
    }

    fn bright_yellow_bold(&self) -> colored::ColoredString {
        self.to_string().bold().bright_yellow()
    }

    fn bright_white_italic(&self) -> colored::ColoredString {
        self.to_string().italic().bright_white()
    }

    fn bright_white_bold(&self) -> colored::ColoredString {
        self.to_string().bold().bright_white()
    }

    // Typed log styles
    // This is used to standardize color and fonts across the codebase
    // ----------------

    fn swap_id(&self) -> colored::ColoredString {
        self.to_string().italic().bright_blue()
    }

    fn label(&self) -> colored::ColoredString {
        self.to_string().bold().bright_white()
    }

    fn addr(&self) -> colored::ColoredString {
        self.to_string().bold().bright_yellow()
    }

    fn tx_hash(&self) -> colored::ColoredString {
        self.to_string().italic().bright_yellow()
    }

    fn err(&self) -> colored::ColoredString {
        self.to_string().bold().bright_red()
    }

    fn err_details(&self) -> colored::ColoredString {
        self.to_string().bold().red()
    }
}

impl<T> LogStyle for T where T: ToString {}
