use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;
use tokio::net::UdpSocket;

pub async fn random_localaddr(min_port: u16, max_port: u16) -> Result<SocketAddr, std::io::Error> {
    let ip = local_network_ip().await?;
    let port = random_port(min_port, max_port);

    let addr = SocketAddr::from((ip, port));
    Ok(addr)
}

async fn local_network_ip() -> Result<IpAddr, std::io::Error> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 40000)).await?;
    socket.connect((Ipv4Addr::new(8, 8, 8, 8), 4000)).await?;
    let got_addr = socket.local_addr()?;
    Ok(got_addr.ip())
}

fn random_port(from: u16, to: u16) -> u16 {
    let valid_range = to - from;
    let info: u16 = rand::random();
    let offset = info % valid_range;
    from + offset
}

pub struct IgdArgs {
    pub search_args: igd::SearchOptions,
    pub lease_duration: Duration,
    pub protocol: igd::PortMappingProtocol,
}

impl IgdArgs {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_protocol(mut self, proto: igd::PortMappingProtocol) -> Self {
        self.protocol = proto;
        self
    }
}

impl Default for IgdArgs {
    fn default() -> Self {
        IgdArgs {
            protocol: igd::PortMappingProtocol::TCP,
            lease_duration: Duration::from_secs(60 * 60 * 2),
            search_args: Default::default(),
        }
    }
}

impl Clone for IgdArgs {
    fn clone(&self) -> Self {
        IgdArgs {
            search_args: igd::SearchOptions { ..self.search_args },
            lease_duration: self.lease_duration,
            protocol: self.protocol,
        }
    }
}

impl PartialEq for IgdArgs {
    fn eq(&self, other: &IgdArgs) -> bool {
        self.lease_duration == other.lease_duration
            && self.protocol == other.protocol
            && self.search_args.bind_addr == other.search_args.bind_addr
            && self.search_args.broadcast_address == other.search_args.broadcast_address
            && self.search_args.timeout == other.search_args.timeout
    }
}

impl Eq for IgdArgs {}

#[derive(Clone, Eq, PartialEq)]
pub struct IgdMapping {
    local_addr: SocketAddr,
    public_addr: SocketAddr,
    args: IgdArgs,
}

impl IgdMapping {
    async fn request_any(
        local_addr: SocketAddrV4,
        args: IgdArgs,
        description: &str,
    ) -> Result<Self, igd::Error> {
        let gateway: igd::aio::Gateway = igd::aio::search_gateway(args.clone().search_args).await?;
        let lease_duration = args.lease_duration.as_secs() as u32;
        let public_addr = gateway
            .get_any_address(args.protocol, local_addr, lease_duration, description)
            .await?;
        Ok(IgdMapping {
            local_addr: local_addr.into(),
            public_addr: public_addr.into(),
            args,
        })
    }

    async fn close_inner(&mut self) -> Result<(), igd::Error> {
        let gateway = igd::aio::search_gateway(igd::SearchOptions {
            ..self.args.search_args
        })
        .await?;
        match gateway
            .remove_port(self.args.protocol, self.public_addr.port())
            .await
        {
            Ok(()) => Ok(()),
            Err(igd::RemovePortError::NoSuchPortMapping) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
impl Drop for IgdMapping {
    fn drop(&mut self) {
        crate::utils::block_on(self.close_inner()).unwrap();
    }
}

use crate::network::stun::{StunMapping, StunMappingError};
pub enum PublicAddr {
    Igd(IgdMapping),
    Raw(SocketAddr),
    Stun(StunMapping),
}

#[derive(Debug)]
pub enum PublicAddrError {
    MachineLocal(SocketAddr),
    InvalidAddress(SocketAddr),
    Io(std::io::Error),
    Ipv6NotYetImplemented(std::net::Ipv6Addr),
    Igd(igd::Error),
    Stun(StunMappingError),
    InvalidProtocol(igd::PortMappingProtocol),
}
use std::fmt;
impl fmt::Display for PublicAddrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PublicAddrError::MachineLocal(addr) => f.write_fmt(format_args!(
                "Error making public addr: {} is not an externally visible address.",
                addr
            )),
            PublicAddrError::Io(ioerr) => {
                f.write_fmt(format_args!("Error making public addr (IO): {}", ioerr))
            }
            PublicAddrError::InvalidAddress(addr) => f.write_fmt(format_args!(
                "Error making public addr: {} is not a valid address.",
                addr
            )),
            PublicAddrError::Ipv6NotYetImplemented(addr) => f.write_fmt(format_args!(
                "Ipv6 is not yet supported for public addrs: {}",
                addr
            )),
            PublicAddrError::Igd(igderror) => {
                f.write_fmt(format_args!("Error making public addr (IGD): {}", igderror))
            }
            PublicAddrError::Stun(stunerror) => f.write_fmt(format_args!(
                "Error making public addr (STUN): {}",
                stunerror
            )),
            PublicAddrError::InvalidProtocol(proto) => {
                let protostr = match proto {
                    igd::PortMappingProtocol::TCP => "TCP",
                    igd::PortMappingProtocol::UDP => "UDP",
                };
                f.write_fmt(format_args!(
                    "Error making public addr: {} is not a valid protocol for this method.",
                    protostr
                ))
            }
        }
    }
}
impl std::error::Error for PublicAddrError {}
impl From<std::io::Error> for PublicAddrError {
    fn from(inner: std::io::Error) -> Self {
        Self::Io(inner)
    }
}
impl From<igd::Error> for PublicAddrError {
    fn from(inner: igd::Error) -> Self {
        Self::Igd(inner)
    }
}

fn filter_clean_addr(addr: SocketAddr) -> Result<SocketAddr, PublicAddrError> {
    if addr.ip().is_loopback() {
        return Err(PublicAddrError::MachineLocal(addr));
    }
    let downcast_ip = match addr.ip() {
        IpAddr::V4(inner) => IpAddr::V4(inner),
        IpAddr::V6(inner) => match inner.to_ipv4() {
            Some(ninner) => IpAddr::V4(ninner),
            None => IpAddr::V6(inner),
        },
    };
    match downcast_ip {
        IpAddr::V4(ipv4) => {
            if ipv4.is_link_local() {
                return Err(PublicAddrError::MachineLocal(addr));
            }
            if ipv4.is_broadcast() || ipv4.is_documentation() {
                return Err(PublicAddrError::InvalidAddress(addr));
            }
            return Ok(SocketAddr::new(downcast_ip, addr.port()));
        }
        IpAddr::V6(_ipv6) => {
            //TODO: IPv6 validity checks.
            return Ok(addr);
        }
    }
}

impl PublicAddr {
    pub fn addr(&self) -> SocketAddr {
        match self {
            PublicAddr::Raw(addr) => *addr,
            PublicAddr::Igd(mapping) => mapping.public_addr,
            PublicAddr::Stun(mapping) => mapping.public_addr(),
        }
    }
    async fn try_already_public(addr: SocketAddr) -> Result<Self, PublicAddrError> {
        let wrapped_ip = if addr.ip().is_unspecified() {
            local_network_ip().await?
        } else {
            addr.ip()
        };
        if wrapped_ip.is_unspecified() {
            return Err(PublicAddrError::InvalidAddress(addr));
        }
        match wrapped_ip {
            IpAddr::V4(inner) => {
                if inner.is_private() {
                    Err(PublicAddrError::InvalidAddress(SocketAddr::new(
                        wrapped_ip,
                        addr.port(),
                    )))
                } else {
                    Ok(Self::Raw(addr))
                }
            }
            IpAddr::V6(inner) => Err(PublicAddrError::Ipv6NotYetImplemented(inner)),
        }
    }
    async fn try_igd(
        addr: SocketAddr,
        proto: igd::PortMappingProtocol,
    ) -> Result<Self, PublicAddrError> {
        match addr {
            SocketAddr::V4(inner) => {
                let args = IgdArgs::new().with_protocol(proto);
                let igdres =
                    IgdMapping::request_any(inner, args, "MediaSync Public Address").await?;
                return Ok(Self::Igd(igdres));
            }
            SocketAddr::V6(inner) => Err(PublicAddrError::Ipv6NotYetImplemented(*inner.ip())),
        }
    }
    async fn try_stun(con: &mut UdpSocket) -> Result<Self, PublicAddrError> {
        match StunMapping::get_mapping(con).await {
            Ok(mapping) => Ok(Self::Stun(mapping)),
            Err(e) => Err(PublicAddrError::Stun(e)),
        }
    }

    pub async fn request_public<'a>(
        args: impl Into<OpenPublicArgs<'a>>,
    ) -> Result<Self, PublicAddrError> {
        let args = args.into();
        let rawaddr = args.addr()?;
        let addr = filter_clean_addr(rawaddr)?;
        let noop_res = Self::try_already_public(addr).await;
        let _noop_err = match noop_res {
            Ok(ret) => {
                return Ok(ret);
            }
            Err(e) => {
                log::info!("Got error from NOOP public mapper: {}", e);
                e
            }
        };
        let proto = args.proto();
        let igd_res = Self::try_igd(addr, proto).await;
        let _igd_err = match igd_res {
            Ok(ret) => {
                return Ok(ret);
            }
            Err(e) => {
                log::info!("Got error from IGD public mapper: {}", e);
                e
            }
        };
        let stun_res = if let OpenPublicArgs::UdpCon(con) = args {
            Self::try_stun(con).await
        } else {
            Err(PublicAddrError::InvalidProtocol(
                igd::PortMappingProtocol::TCP,
            ))
        };
        let _stun_err = match stun_res {
            Ok(ret) => {
                return Ok(ret);
            }
            Err(e) => {
                log::info!("Got error from STUN public mapper: {}", e);
                e
            }
        };
        Err(_igd_err)
    }
}

pub enum OpenPublicArgs<'a> {
    TcpAddr(SocketAddr),
    UdpCon(&'a mut UdpSocket),
}

impl<'a> OpenPublicArgs<'a> {
    pub fn proto(&self) -> igd::PortMappingProtocol {
        match self {
            Self::TcpAddr(_) => igd::PortMappingProtocol::TCP,
            Self::UdpCon(_) => igd::PortMappingProtocol::UDP,
        }
    }
    pub fn addr(&self) -> Result<SocketAddr, std::io::Error> {
        match self {
            Self::TcpAddr(inner) => Ok(*inner),
            Self::UdpCon(con) => con.local_addr(),
        }
    }
}

impl<'a> From<&'a mut UdpSocket> for OpenPublicArgs<'a> {
    fn from(inner: &'a mut UdpSocket) -> Self {
        Self::UdpCon(inner)
    }
}

impl<'a> From<SocketAddr> for OpenPublicArgs<'a> {
    fn from(inner: SocketAddr) -> Self {
        Self::TcpAddr(inner)
    }
}
