use crate::events;
use crate::events::RemoteEvent;
use crate::DebugError;
use crate::MyResult;
use igd;
use rand;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

mod ipcodec;
pub use ipcodec::*;
mod filetransfer;
pub use filetransfer::*;

impl<T> DebugError for std::sync::PoisonError<T> {
    fn message(self) -> String {
        format!("std::sync::PoisonError : {:?}", self)
    }
}

fn local_network_ip() -> MyResult<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 40000))?;
    socket.connect((Ipv4Addr::new(8, 8, 8, 8), 4000))?;
    let got_addr = socket.local_addr()?;
    if let IpAddr::V4(ip) = got_addr.ip() {
        Ok(ip)
    } else {
        Err(format!("Error: got invalid IP: {:?}", got_addr.ip()).into())
    }
}

fn random_port(from: u16, to: u16) -> u16 {
    let valid_range = to - from;
    let info: u16 = rand::random();
    let offset = info % valid_range;
    from + offset
}

struct MessageBuffer {
    parser: events::BlockParser,
    bytes: [u8; 32],
    bytes_read: usize,
}

impl MessageBuffer {
    pub fn new() -> MessageBuffer {
        MessageBuffer {
            parser: events::BlockParser::new(),
            bytes: [0; 32],
            bytes_read: 0,
        }
    }

    pub fn empty_space_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[self.bytes_read..]
    }
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
    pub fn request_any(
        local_addr: SocketAddrV4,
        args: IgdArgs,
        description: &str,
    ) -> MyResult<IgdMapping> {
        let gateway = igd::search_gateway(args.clone().search_args)?;
        let lease_duration = args.lease_duration.as_secs() as u32;
        let public_addr =
            gateway.get_any_address(args.protocol, local_addr, lease_duration, description)?;
        Ok(IgdMapping {
            local_addr: local_addr.into(),
            public_addr: public_addr.into(),
            args,
        })
    }

    fn close_inner(&mut self) -> MyResult<()> {
        let gateway = igd::search_gateway(igd::SearchOptions {
            ..self.args.search_args
        })?;
        match gateway.remove_port(self.args.protocol, self.public_addr.port()) {
            Ok(()) => Ok(()),
            Err(igd::RemovePortError::NoSuchPortMapping) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn close(mut self) -> MyResult<()> {
        self.close_inner()
    }
}

impl Drop for IgdMapping {
    fn drop(&mut self) {
        self.close_inner().unwrap();
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
    pub fn request_public(local_addr: SocketAddr) -> MyResult<PublicAddr> {
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
                    )?;
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

pub struct ConnectionsThreadHandle {
    local_addr: SocketAddr,
    public_addr: Arc<RwLock<Option<PublicAddr>>>,
    client_addresses: Arc<RwLock<Vec<SocketAddr>>>,
    server_addresses: Arc<RwLock<Vec<SocketAddr>>>,

    message_channel: Sender<ConnectionMessage>,
    event_channel: Sender<RemoteEvent>,
    event_recv: Arc<Mutex<Receiver<RemoteEvent>>>,

    handle: Arc<JoinHandle<()>>,
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum ConnectionMessage {
    ConnectRemote(SocketAddr),
    OpenPublic,
    Ping, 
}

impl ConnectionsThreadHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
    pub fn open_public(&mut self) -> MyResult<()> {
        self.message_channel
            .send(ConnectionMessage::OpenPublic)
            .map_err(|e| e.into())
    }
    pub fn public_addr(&self) -> Option<SocketAddr> {
        let locked = self.public_addr.read();
        locked
            .as_ref()
            .ok()
            .and_then(|l| l.as_ref())
            .map(|l| l.addr())
    }
    pub fn send_event(&mut self, event: RemoteEvent) -> MyResult<()> {
        self.event_channel.send(event).map_err(|e| e.into())
    }

    pub fn check_events(&mut self) -> MyResult<Vec<RemoteEvent>> {
        let lock = self
            .event_recv
            .lock()
            .map_err(|e| format!("Mutex Lock Error: {:?}", e))?;
        let iter = lock.try_iter();
        let retvl = iter.collect();
        Ok(retvl)
    }

    pub fn open_connection(&mut self, addr: SocketAddr) -> MyResult<()> {
        assert!(!self.has_paniced());
        self.message_channel
            .send(ConnectionMessage::ConnectRemote(addr))
            .map_err(|e| e.into())
    }

    pub fn connection_addresses(&self) -> MyResult<Vec<SocketAddr>> {
        let client_addr_iter = self
            .client_addresses
            .read()
            .map_err(DebugError::into_myerror)?;
        let server_addr_iter = self
            .server_addresses
            .read()
            .map_err(DebugError::into_myerror)?;
        let retvl = client_addr_iter
            .iter()
            .copied()
            .chain(server_addr_iter.iter().copied())
            .collect();
        Ok(retvl)
    }

    pub fn has_paniced(&self) -> bool {
       match  self.message_channel.send(ConnectionMessage::Ping) {
           Ok(()) => false, 
           Err(std::sync::mpsc::SendError(_)) => true,
       }
    }
}

impl Clone for ConnectionsThreadHandle {
    fn clone(&self) -> ConnectionsThreadHandle {
        ConnectionsThreadHandle {
            local_addr: self.local_addr,
            public_addr: Arc::clone(&self.public_addr),
            client_addresses: Arc::clone(&self.client_addresses),
            server_addresses: Arc::clone(&self.server_addresses),

            message_channel: self.message_channel.clone(),
            event_channel: self.event_channel.clone(),
            event_recv: Arc::clone(&self.event_recv),
            handle: Arc::clone(&self.handle),
        }
    }
}

pub struct ConnnectionThread {
    listener: TcpListener,

    public_addr: Arc<RwLock<Option<PublicAddr>>>,
    client_addresses: Arc<RwLock<Vec<SocketAddr>>>,
    server_addresses: Arc<RwLock<Vec<SocketAddr>>>,

    message_channel: Option<Receiver<ConnectionMessage>>,
    event_channel: Option<Receiver<RemoteEvent>>,
    event_recv: Option<Sender<RemoteEvent>>,

    client_connections: Vec<TcpStream>,
    host_connections: Vec<TcpStream>,

    stream_buffers: HashMap<SocketAddr, MessageBuffer>,
}
impl ConnnectionThread {
    pub fn open(min_port: u16, max_port: u16) -> MyResult<Self> {
        let ip = local_network_ip()?;
        let port = random_port(min_port, max_port);
        let addr = SocketAddrV4::new(ip, port);
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        Ok(ConnnectionThread::new(listener))
    }
    pub fn new(listener: TcpListener) -> ConnnectionThread {
        ConnnectionThread {
            listener,
            public_addr: Arc::new(RwLock::new(None)),
            client_addresses: Arc::new(RwLock::new(Vec::new())),
            server_addresses: Arc::new(RwLock::new(Vec::new())),
        
            message_channel: None,
            event_channel: None,
            event_recv: None,

            client_connections: Vec::new(),
            host_connections: Vec::new(),

            stream_buffers: HashMap::new(),
        }
    }
    pub fn start(mut self) -> MyResult<ConnectionsThreadHandle> {
        let local_addr = self.listener.local_addr()?;
        let (message_channel_sender, message_channel_reciever) = mpsc::channel();
        self.message_channel = Some(message_channel_reciever);

        let (event_channel_sender, event_channel_reciever) = mpsc::channel();
        self.event_channel = Some(event_channel_reciever);

        let (event_broadcast_sender, event_broadcast_reciever) = mpsc::channel();
        self.event_recv = Some(event_broadcast_sender);
        let public_addr = Arc::clone(&self.public_addr);
        let client_addresses = Arc::clone(&self.client_addresses);
        let server_addresses = Arc::clone(&self.server_addresses);
        let handle = thread::spawn(move || loop {
            match self.listener.accept() {
                Ok((stream, addr)) => {
                    stream.set_nonblocking(true).unwrap();
                    self.host_connections.push(stream);
                    self.server_addresses.write().unwrap().push(addr);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    crate::debug_print(format!("Comms thread dying due to error:  {:?}", e));
                    Result::<(), std::io::Error>::Err(e).unwrap();
                }
            }
            match self.message_channel.as_ref().unwrap().try_recv() {
                Ok(ConnectionMessage::ConnectRemote(addr)) => {
                    println!("Trying to connect to addr {:?}", addr);
                    let connection = std::net::TcpStream::connect(addr).unwrap();
                    self.client_connections.push(connection);
                    self.client_addresses.write().unwrap().push(addr);
                }
                Ok(ConnectionMessage::OpenPublic) => {
                    let mut addr_lock = self.public_addr.write().unwrap();
                    if !addr_lock.is_some() {
                        let new_public_addr = {
                            let addr = match local_addr {
                                        SocketAddr::V4(ipprt) => Ok(ipprt),
                                        SocketAddr::V6(ipprt) => {
                                            Err(format!("Error: IPv6 address {:?} does not yet support public mappings.", ipprt).into())
                                        }
                                    };
                            let mapped = addr.and_then(|a| PublicAddr::request_public(a.into()));
                            mapped.unwrap()
                        };
                        *addr_lock = Some(new_public_addr);
                    }
                }
                Ok(ConnectionMessage::Ping) => {}
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    crate::debug_print(format!("Comms thread dying due to error:  {:?}", mpsc::TryRecvError::Disconnected));
                    Result::<(), _>::Err(mpsc::TryRecvError::Disconnected).unwrap();
                }
            }

            if !self.client_connections.is_empty() || !self.host_connections.is_empty() {}

            let mut valid_streams: Vec<_> = self
                .client_connections
                .iter_mut()
                .chain(self.host_connections.iter_mut())
                .filter(|con| con.peer_addr().is_ok())
                .collect();

            for stream in &mut valid_streams {
                let addr = match stream.peer_addr() {
                    Ok(addr) => addr,
                    Err(ref e) if e.kind() == std::io::ErrorKind::NotConnected => {
                        continue;
                    }
                    Err(e) => {
                        crate::debug_print(format!("Comms thread dying due to error:  {:?}", mpsc::TryRecvError::Disconnected));
                        Err(e).unwrap()
                    }
                };
                let buffer = self
                    .stream_buffers
                    .entry(addr)
                    .or_insert_with(MessageBuffer::new);
                while let Some(evt) = check_stream_events(stream, buffer).unwrap() {
                    self.event_recv.as_mut().unwrap().send(evt).unwrap();
                }
            }
            for evt in self.event_channel.as_ref().unwrap().try_iter() {
                let event_blocks = evt.as_blocks();
                for stream in valid_streams.iter_mut() {
                    for block in event_blocks.iter() {
                        stream.write_all(block.as_bytes()).unwrap();
                    }
                }
            }
            thread::sleep(Duration::from_millis(20));
        });
        let retvl = ConnectionsThreadHandle {
            local_addr,
            public_addr,
            client_addresses,
            server_addresses,

            message_channel: message_channel_sender,
            event_channel: event_channel_sender,
            event_recv: Arc::new(Mutex::new(event_broadcast_reciever)),

            handle: Arc::new(handle),
        };
        Ok(retvl)
    }
}
fn check_stream_events(
    stream: &mut TcpStream,
    buffer: &mut MessageBuffer,
) -> MyResult<Option<RemoteEvent>> {
    let bytes_read = match stream.read(buffer.empty_space_mut()) {
        Ok(b) => b,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                0
            } else {
                return Err(e.into());
            }
        }
    };
    buffer.bytes_read += bytes_read;
    if buffer.bytes_read >= 32 {
        buffer.bytes_read = 0;
        let moved_bytes = std::mem::replace(&mut buffer.bytes, [0; 32]);
        let block = events::RawBlock::from_data(moved_bytes);
        buffer.parser.parse_next(block)
    } else {
        Ok(None)
    }
}
