#[cfg(feature = "dnsmapping")]
mod igdutils;
#[cfg(feature = "dnsmapping")]
use self::igdutils::{IgdArgs, IgdMapping};

#[cfg(feature = "stunmapping")]
mod stunutils;
#[cfg(feature = "stunmapping")]
use self::stunutils::{StunMapping, StunMappingError};

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use tokio::net::UdpSocket;

/// A globally-accessible address or address mapping.
pub enum PublicAddr {
    Raw(SocketAddr),

    #[cfg(feature = "dnsmapping")]
    Igd(IgdMapping),

    #[cfg(feature = "stunmapping")]
    Stun(StunMapping),
}

#[derive(Debug)]
pub enum PublicAddrError {
    MachineLocal(SocketAddr),
    InvalidAddress(SocketAddr),
    Io(std::io::Error),
    Ipv6NotYetImplemented(std::net::Ipv6Addr),

    #[cfg(feature = "stunmapping")]
    Stun(StunMappingError),
    #[cfg(feature = "dnsmapping")]
    Igd(igd::Error),
    #[cfg(feature = "dnsmapping")]
    InvalidProtocol(igd::PortMappingProtocol),
}

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
            #[cfg(feature = "dnsmapping")]
            PublicAddrError::Igd(igderror) => {
                f.write_fmt(format_args!("Error making public addr (IGD): {}", igderror))
            }
            #[cfg(feature = "stunmapping")]
            PublicAddrError::Stun(stunerror) => f.write_fmt(format_args!(
                "Error making public addr (STUN): {}",
                stunerror
            )),
            #[cfg(feature = "dnsmapping")]
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

#[cfg(feature = "dnsmapping")]
impl From<igd::Error> for PublicAddrError {
    fn from(inner: igd::Error) -> Self {
        Self::Igd(inner)
    }
}

/// Verifies that `addr` is not categorically unable to be mapped to a publicly
/// facing address.
/// Examples of this are the loopback address and link-local addresses, since
/// these are hard-coded to never leave the machine.
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
            Ok(SocketAddr::new(downcast_ip, addr.port()))
        }
        IpAddr::V6(_ipv6) => {
            //TODO: IPv6 validity checks.
            Ok(addr)
        }
    }
}

impl PublicAddr {
    /// The public IP address and port of this mapping.
    pub fn addr(&self) -> SocketAddr {
        match self {
            PublicAddr::Raw(addr) => *addr,
            #[cfg(feature = "dnsmapping")]
            PublicAddr::Igd(mapping) => mapping.public_addr(),
            #[cfg(feature = "stunmapping")]
            PublicAddr::Stun(mapping) => mapping.public_addr(),
        }
    }

    /// Checks if `addr` is already a publically accessible IP+Port.
    async fn try_already_public(addr: SocketAddr) -> Result<Self, PublicAddrError> {
        let wrapped_ip = if addr.ip().is_unspecified() {
            crate::network::utils::local_network_ip().await?
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

    #[cfg(feature = "dnsmapping")]
    /// Attempts to open a DNS mapping.
    async fn try_igd(
        addr: SocketAddr,
        proto: igd::PortMappingProtocol,
    ) -> Result<Self, PublicAddrError> {
        match addr {
            SocketAddr::V4(inner) => {
                let args = IgdArgs::new().with_protocol(proto);
                let igdres =
                    IgdMapping::request_any(inner, args, "MediaSync Public Address").await?;
                Ok(Self::Igd(igdres))
            }
            SocketAddr::V6(inner) => Err(PublicAddrError::Ipv6NotYetImplemented(*inner.ip())),
        }
    }

    #[cfg(feature = "stunmapping")]
    /// Attempts to open a mapping based on the STUN protocol.
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
        #[cfg(feature = "dnsmapping")]
        let _igd_err = {
            let proto = args.proto();
            let igd_res = Self::try_igd(addr, proto).await;
            match igd_res {
                Ok(ret) => {
                    return Ok(ret);
                }
                Err(e) => {
                    log::info!("Got error from IGD public mapper: {}", e);
                    e
                }
            }
        };
        #[cfg(not(feature = "dnsmapping"))]
        let _igd_err = _noop_err;
        let stun_res = if let OpenPublicArgs::UdpCon(con) = args {
            #[cfg(feature = "stunmapping")]
            {
                Self::try_stun(con).await
            }

            #[cfg(not(feature = "stunmapping"))]
            {
                Err(PublicAddrError::Io(std::io::ErrorKind::Other.into()))
            }
        } else {
            #[cfg(feature = "dnsmapping")]
            {
                Err(PublicAddrError::InvalidProtocol(
                    igd::PortMappingProtocol::TCP,
                ))
            }
            #[cfg(not(feature = "dnsmapping"))]
            {
                Err(PublicAddrError::InvalidAddress(args.addr().unwrap()))
            }
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

/// Arguments for `PublicAddre::request_public`. 
pub enum OpenPublicArgs<'a> {
    /// A TCP-mapped local addres.
    TcpAddr(SocketAddr),
    /// An open UDP-mapped socket.
    UdpCon(&'a mut UdpSocket),
}

impl<'a> OpenPublicArgs<'a> {
    #[cfg(feature = "dnsmapping")]
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
