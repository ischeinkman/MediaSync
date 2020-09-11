use std::net::{SocketAddr, SocketAddrV4};
use std::time::Duration;

/// Argumeents for setting up a DNS mapping.
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

/// A DNS-based mapping from a local network IP+Port to a publicly-accessible IP+Port.
#[derive(Clone, Eq, PartialEq)]
pub struct IgdMapping {
    local_addr: SocketAddr,
    public_addr: SocketAddr,
    args: IgdArgs,
}

impl IgdMapping {
    /// Requests a new mapping between the local address `local_addr` to *any* publicly visible
    /// IP and port. 
    pub async fn request_any(
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

    /// The publicly visible ip and port of this mapping.
    pub fn public_addr(&self) -> SocketAddr {
        self.public_addr
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
