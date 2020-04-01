use crate::DynResult;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;
use tokio::net::{TcpListener, UdpSocket};

pub async fn random_listener(min_port: u16, max_port: u16) -> DynResult<TcpListener> {
    let ip = local_network_ip().await?;
    let port = random_port(min_port, max_port);

    let addr = SocketAddr::from((ip, port));
    let listener = TcpListener::bind(addr).await?;
    Ok(listener)
}

async fn local_network_ip() -> DynResult<IpAddr> {
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
        let gateway = igd::aio::search_gateway(args.clone().search_args).await?;
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

    async fn close_inner(&mut self) -> DynResult<()> {
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
        futures::executor::block_on(self.close_inner()).unwrap();
    }
}

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
    pub async fn request_public(local_addr: SocketAddr) -> DynResult<PublicAddr> {
        match local_addr {
            SocketAddr::V4(addr) => {
                let ip = addr.ip();
                if ip.is_loopback() || ip.is_broadcast() || ip.is_unspecified() {
                    Err(format!("Error: got invalid local address {}:{}", ip, addr.port()).into())
                } else if ip.is_private() {
                    let mapped = IgdMapping::request_any(
                        addr,
                        IgdArgs::default(),
                        &format!("IlSync Mapping for local ip {}:{}", ip, addr.port()),
                    )
                    .await?;
                    Ok(PublicAddr::Igd(mapped))
                } else {
                    Ok(PublicAddr::Raw(local_addr))
                }
            }
            SocketAddr::V6(addr) => Err(format!(
                "Error: IPv6 address {}:{} is not yet supported.",
                *addr.ip(),
                addr.port()
            )
            .into()),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        match self {
            PublicAddr::Igd(mapping) => mapping.public_addr,
            PublicAddr::Raw(addr) => *addr,
        }
    }
}
