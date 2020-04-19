use crate::DynResult;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;
use tokio::net::UdpSocket;

async fn random_localaddr(min_port: u16, max_port: u16) -> DynResult<SocketAddr> {
    let ip = local_network_ip().await.unwrap();
    let port = random_port(min_port, max_port);

    let addr = SocketAddr::from((ip, port));
    Ok(addr)
}

async fn local_network_ip() -> DynResult<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 40000))
        .await
        .unwrap();
    socket
        .connect((Ipv4Addr::new(8, 8, 8, 8), 4000))
        .await
        .unwrap();
    let got_addr = socket.local_addr().unwrap();
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
    pub async fn request_any(
        local_addr: SocketAddrV4,
        args: IgdArgs,
        description: &str,
    ) -> DynResult<IgdMapping> {
        let gateway: igd::aio::Gateway = igd::aio::search_gateway(args.clone().search_args)
            .await
            .unwrap();
        let lease_duration = args.lease_duration.as_secs() as u32;
        let public_addr = gateway
            .get_any_address(args.protocol, local_addr, lease_duration, description)
            .await
            .unwrap();
        Ok(IgdMapping {
            local_addr: local_addr.into(),
            public_addr: public_addr.into(),
            args,
        })
    }

    async fn close_inner(&mut self) -> DynResult<()> {
        let gateway = igd::aio::search_gateway(igd::SearchOptions {
            ..self.args.search_args
        })
        .await
        .unwrap();
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

async fn request_public_if_needed(
    local_addr: SocketAddr,
    args: IgdArgs,
) -> DynResult<Option<IgdMapping>> {
    match local_addr {
        SocketAddr::V4(addr) => {
            let ip = addr.ip();
            let is_valid = !ip.is_loopback() && !ip.is_broadcast() && !ip.is_unspecified();
            if !is_valid {
                return Err(
                    format!("Error: got invalid local address {}:{}", ip, addr.port()).into(),
                );
            }
            if !ip.is_private() {
                return Ok(None);
            }

            let mapped = IgdMapping::request_any(
                addr,
                args,
                &format!("MediaSync Mapping for local ip {}:{}", ip, addr.port()),
            )
            .await?;
            Ok(Some(mapped))
        }
        SocketAddr::V6(addr) => Err(format!(
            "Error: IPv6 address {}:{} is not yet supported.",
            *addr.ip(),
            addr.port()
        )
        .into()),
    }
}

pub mod udp {
    use super::{random_localaddr, IgdArgs, IgdMapping};
    use crate::DynResult;
    use std::net::SocketAddr;
    use tokio::net::UdpSocket;
    #[derive(Clone, Eq, PartialEq)]
    pub enum PublicAddr {
        Igd(IgdMapping),
        Raw(SocketAddr),
    }

    impl From<IgdMapping> for PublicAddr {
        fn from(mapping: IgdMapping) -> PublicAddr {
            PublicAddr::Igd(mapping)
        }
    }

    impl PublicAddr {
        pub fn addr(&self) -> SocketAddr {
            match self {
                PublicAddr::Igd(mapping) => mapping.public_addr,
                PublicAddr::Raw(addr) => *addr,
            }
        }
    }
    impl PublicAddr {
        pub async fn request_public(local_addr: SocketAddr) -> DynResult<PublicAddr> {
            let mut args = IgdArgs::default();
            args.protocol = igd::PortMappingProtocol::UDP;
            if let Some(public) = super::request_public_if_needed(local_addr, args).await? {
                Ok(PublicAddr::Igd(public))
            } else {
                Ok(PublicAddr::Raw(local_addr))
            }
        }
    }
    #[allow(dead_code)]
    pub async fn random_listener(min_port: u16, max_port: u16) -> DynResult<UdpSocket> {
        let addr = random_localaddr(min_port, max_port).await.unwrap();
        let listener = UdpSocket::bind(addr).await.unwrap();
        Ok(listener)
    }
}

pub mod tcp {
    use super::{random_localaddr, IgdArgs, IgdMapping};
    use crate::DynResult;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    #[derive(Clone, Eq, PartialEq)]
    pub enum PublicAddr {
        Igd(IgdMapping),
        Raw(SocketAddr),
    }

    impl From<IgdMapping> for PublicAddr {
        fn from(mapping: IgdMapping) -> PublicAddr {
            PublicAddr::Igd(mapping)
        }
    }

    impl PublicAddr {
        pub fn addr(&self) -> SocketAddr {
            match self {
                PublicAddr::Igd(mapping) => mapping.public_addr,
                PublicAddr::Raw(addr) => *addr,
            }
        }
    }
    impl PublicAddr {
        pub async fn request_public(local_addr: SocketAddr) -> DynResult<PublicAddr> {
            let mut args = IgdArgs::default();
            args.protocol = igd::PortMappingProtocol::TCP;
            if let Some(public) = super::request_public_if_needed(local_addr, args).await? {
                Ok(PublicAddr::Igd(public))
            } else {
                Ok(PublicAddr::Raw(local_addr))
            }
        }
    }
    pub async fn random_listener(min_port: u16, max_port: u16) -> DynResult<TcpListener> {
        let addr = random_localaddr(min_port, max_port).await.unwrap();
        let listener = TcpListener::bind(addr).await.unwrap();
        Ok(listener)
    }
}

pub use tcp::*;
