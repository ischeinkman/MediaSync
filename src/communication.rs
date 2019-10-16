use crate::events;
use crate::events::RemoteEvent;
use crate::DebugError;
use crate::MyResult;

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
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
mod networking;
pub use networking::*;

impl<T> DebugError for std::sync::PoisonError<T> {
    fn message(self) -> String {
        format!("std::sync::PoisonError : {:?}", self)
    }
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
        match self.message_channel.send(ConnectionMessage::Ping) {
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
        let listener = random_listener(min_port, max_port)?;
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

    fn check_new_connections(&mut self) -> MyResult<Option<SocketAddr>> {
        match self.listener.accept() {
            Ok((stream, addr)) => {
                stream.set_nonblocking(true)?;
                self.host_connections.push(stream);
                self.server_addresses.write().unwrap().push(addr);
                Ok(Some(addr))
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    fn process_connection_message(&mut self) -> MyResult<()> {
        let local_addr = self.listener.local_addr()?;
        let channel = self
            .message_channel
            .as_ref()
            .ok_or_else(|| "No message channel found for comms thread!")?;
        loop {
            let msg = channel.try_recv();
            match msg {
                Ok(ConnectionMessage::ConnectRemote(addr)) => {
                    println!("Trying to connect to addr {:?}", addr);
                    let connection = std::net::TcpStream::connect(addr).unwrap();
                    connection.set_nonblocking(true).unwrap();
                    self.client_connections.push(connection);
                    self.client_addresses.write().unwrap().push(addr);
                }
                Ok(ConnectionMessage::OpenPublic) => {
                    let mut addr_lock = self.public_addr.write().unwrap();
                    if addr_lock.is_none() {
                        let new_public_addr = PublicAddr::request_public(local_addr)?;
                        *addr_lock = Some(new_public_addr);
                    }
                }
                Ok(ConnectionMessage::Ping) => {}
                Err(mpsc::TryRecvError::Empty) => {
                    break Ok(());
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    break Err(mpsc::TryRecvError::Disconnected.into());
                }
            }
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
            if let Err(e) = self.check_new_connections() {
                crate::debug_print(format!("Comms thread dying due to error:  {:?}", e));
                MyResult::<()>::Err(e).unwrap();
            }
            if let Err(e) = self.process_connection_message() {
                crate::debug_print(format!("Comms thread dying due to error:  {:?}", e));
                MyResult::<()>::Err(e).unwrap();
            }
            let mut raw_events = Vec::new();
            for stream in self.client_connections.iter_mut() {
                let cur_events = collect_stream_events(stream, &mut self.stream_buffers).unwrap();
                raw_events.push((stream.peer_addr().unwrap(), cur_events));
            }

            let mut self_events = PlayerEventGroup::new();
            for evt in self
                .event_channel
                .as_ref()
                .into_iter()
                .flat_map(|channel| channel.try_iter())
            {
                self_events = self_events.with_event(evt);
            }
            let my_ip = if let Some(ref addr) = self.public_addr.read().unwrap().as_ref() {
                addr.addr()
            } else {
                self.listener.local_addr().unwrap()
            };
            for stream in self.host_connections.iter_mut() {
                let cur_events = collect_stream_events(stream, &mut self.stream_buffers).unwrap();
                raw_events.push((stream.peer_addr().unwrap(), cur_events));
            }

            let mut rectified_self = self_events.clone();
            let mut cascaded_remotes = raw_events
                .first()
                .cloned()
                .map(|(_, evt)| evt)
                .unwrap_or_default();

            for (addr, group) in raw_events.iter_mut() {
                *group = group.clone().rectify(*addr, &rectified_self, my_ip);
                rectified_self = rectified_self.rectify(my_ip, group, *addr);
                cascaded_remotes = cascaded_remotes.cascade(my_ip, group, *addr);
            }
            for (addr, group) in raw_events.iter_mut() {
                *group = group.clone().rectify(*addr, &rectified_self, my_ip);
                rectified_self = rectified_self.rectify(my_ip, group, *addr);
                cascaded_remotes = cascaded_remotes.cascade(my_ip, group, *addr);
            }

            for evt in rectified_self.into_events() {
                let event_blocks = evt.as_blocks();
                for block in event_blocks {
                    let all_streams = self
                        .client_connections
                        .iter_mut()
                        .chain(self.host_connections.iter_mut());
                    let valid_streams =
                        all_streams.filter(|stream| validate_connection(stream).unwrap().is_some());
                    for stream in valid_streams {
                        stream.write_all(block.as_bytes()).unwrap();
                    }
                }
            }
            for evt in cascaded_remotes.into_events() {
                self.event_recv.as_ref().unwrap().send(evt.clone()).unwrap();
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

struct StreamEventIter<'a, 'b> {
    stream: &'a mut TcpStream,
    buffer: &'b mut MessageBuffer,
}

impl<'a, 'b> StreamEventIter<'a, 'b> {
    pub fn from_buffer_map(
        stream: &'a mut TcpStream,
        buffers: &'b mut HashMap<SocketAddr, MessageBuffer>,
    ) -> io::Result<Self> {
        let addr = stream.peer_addr()?;
        let buffer = buffers.entry(addr).or_insert_with(MessageBuffer::new);
        Ok(Self { stream, buffer })
    }
}

impl<'a, 'b> Iterator for StreamEventIter<'a, 'b> {
    type Item = MyResult<RemoteEvent>;
    fn next(&mut self) -> Option<MyResult<RemoteEvent>> {
        match check_stream_events(self.stream, self.buffer) {
            Ok(Some(evt)) => Some(Ok(evt)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

fn collect_stream_events(
    stream: &mut TcpStream,
    buffers: &mut HashMap<SocketAddr, MessageBuffer>,
) -> MyResult<PlayerEventGroup> {
    match StreamEventIter::from_buffer_map(stream, buffers) {
        Ok(itr) => {
            let mut retvl = PlayerEventGroup::new();
            for evt in itr {
                retvl = retvl.with_event(evt?);
            }
            Ok(retvl)
        }
        Err(ref e) if is_disconnection_error(e) => Ok(PlayerEventGroup::new()),
        Err(e) => Err(e.into()),
    }
}

fn validate_connection(stream: &TcpStream) -> MyResult<Option<SocketAddr>> {
    match stream.peer_addr() {
        Ok(addr) => Ok(Some(addr)),
        Err(ref e) if is_disconnection_error(e) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn is_disconnection_error(e: &io::Error) -> bool {
    match e.kind() {
        io::ErrorKind::NotConnected => true,
        io::ErrorKind::ConnectionAborted => true,
        io::ErrorKind::ConnectionReset => true,
        _ => false,
    }
}

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct PlayerEventGroup {
    media_open_event: Option<RemoteEvent>,
    is_paused: Option<bool>,
    did_jump: bool,
    position: Option<Duration>,
}

impl Default for PlayerEventGroup {
    fn default() -> Self {
        PlayerEventGroup::new()
    }
}

impl PlayerEventGroup {
    pub fn new() -> Self {
        PlayerEventGroup {
            media_open_event: None,
            is_paused: None,
            did_jump: false,
            position: None,
        }
    }

    pub fn with_event(mut self, event: RemoteEvent) -> Self {
        if let Some(RemoteEvent::MediaOpen(_)) = self.media_open_event {
            return self;
        }
        match event {
            RemoteEvent::Pause => self.is_paused = Some(true),
            RemoteEvent::Play => self.is_paused = Some(false),
            RemoteEvent::Jump(dt) => {
                self.position = Some(dt);
                self.did_jump = true;
            }
            RemoteEvent::MediaOpen(_) => {
                return PlayerEventGroup::new().with_event(event);
            }
            RemoteEvent::MediaOpenOkay(_)
            | RemoteEvent::RequestTransfer(_)
            | RemoteEvent::RespondTransfer { .. } => {
                self.media_open_event = Some(event);
            }
            RemoteEvent::Ping(png) => {
                self.position.get_or_insert(png.time());
            }
            RemoteEvent::Shutdown => {
                return PlayerEventGroup::new();
            }
        }
        self
    }

    pub fn into_events(self) -> impl Iterator<Item = RemoteEvent> {
        let status_event = self
            .is_paused
            .map(|paused| {
                if paused {
                    RemoteEvent::Pause
                } else {
                    RemoteEvent::Play
                }
            })
            .into_iter();
        let media_event = self.media_open_event.into_iter();
        let position_event = if self.did_jump {
            let pos = self.position.unwrap_or(Duration::from_micros(0));
            Some(RemoteEvent::Jump(pos)).into_iter()
        } else {
            self.position
                .map(|p| RemoteEvent::Ping(p.into()))
                .into_iter()
        };
        media_event.chain(position_event).chain(status_event)
    }

    pub fn cascade(
        self,
        self_addr: SocketAddr,
        other: &PlayerEventGroup,
        other_addr: SocketAddr,
    ) -> PlayerEventGroup {
        let rectified_self = self.rectify(self_addr, other, other_addr);
        PlayerEventGroup {
            media_open_event: rectified_self.media_open_event,
            is_paused: rectified_self.is_paused.or(other.is_paused),
            did_jump: rectified_self.did_jump || other.did_jump,
            position: rectified_self.position.or(other.position),
        }
    }

    pub fn rectify(
        self,
        self_addr: SocketAddr,
        other: &PlayerEventGroup,
        other_addr: SocketAddr,
    ) -> PlayerEventGroup {
        let self_public = match self_addr.ip() {
            IpAddr::V4(ip) => !ip.is_private(),
            IpAddr::V6(_) => true,
        };
        let other_public = match other_addr.ip() {
            IpAddr::V4(ip) => !ip.is_private(),
            IpAddr::V6(_) => true,
        };
        let self_has_priority = if self_public && !other_public {
            true
        } else if !self_public && other_public {
            false
        } else {
            let self_bytes: u128 = match self_addr.ip() {
                IpAddr::V4(ip) => ip.to_ipv6_compatible().into(),
                IpAddr::V6(ip) => ip.into(),
            };
            let other_bytes: u128 = match other_addr.ip() {
                IpAddr::V4(ip) => ip.to_ipv6_compatible().into(),
                IpAddr::V6(ip) => ip.into(),
            };
            if self_bytes != other_bytes {
                self_bytes > other_bytes
            } else {
                self_addr.port() > other_addr.port()
            }
        };

        match (
            self.media_open_event.as_ref(),
            other.media_open_event.as_ref(),
        ) {
            (Some(RemoteEvent::MediaOpen(ref self_url)), Some(RemoteEvent::MediaOpen(_))) => {
                if self_has_priority {
                    return PlayerEventGroup::new()
                        .with_event(RemoteEvent::MediaOpen(self_url.clone()));
                } else {
                    return PlayerEventGroup::new();
                }
            }
            (Some(RemoteEvent::MediaOpen(ref url)), _) => {
                return PlayerEventGroup::new().with_event(RemoteEvent::MediaOpen(url.to_owned()));
            }
            (None, Some(RemoteEvent::MediaOpen(_))) => {
                return PlayerEventGroup::new();
            }
            _ => {}
        }

        let mut retvl = PlayerEventGroup::new();
        retvl.media_open_event = self.media_open_event;
        retvl.did_jump = self.did_jump;
        let use_self_position = if self.did_jump == other.did_jump {
            self_has_priority
        } else {
            self.did_jump
        };

        retvl.position = if use_self_position {
            self.position
        } else {
            None
        };

        retvl.is_paused = if self_has_priority {
            self.is_paused
        } else {
            None
        };
        retvl
    }
}


mod test {
    use super::*;
    #[test]
    fn test_ip_priority() {
        let ip_a = SocketAddr::from(([192, 168, 1, 32], 49111));
        let events_a = PlayerEventGroup::new().with_event(RemoteEvent::Ping(Duration::from_millis(10_000).into())).with_event(RemoteEvent::Pause);
        let ip_b = SocketAddr::from(([128, 14, 1, 32], 4));
        let events_b = PlayerEventGroup::new().with_event(RemoteEvent::Ping(Duration::from_millis(1_000).into())).with_event(RemoteEvent::Play);

        let rectified_b = events_b.clone().rectify(ip_b,&events_a, ip_a);
        let rectified_a = events_a.clone().rectify(ip_a, &events_b, ip_b);
        assert_eq!(Vec::<RemoteEvent>::new(), rectified_a.into_events().collect::<Vec<_>>());
        assert_eq!(events_b, rectified_b);
    }

    #[test]
    fn test_jump_priority() {
        let ip_a = SocketAddr::from(([192, 168, 1, 32], 49111));
        let events_a = PlayerEventGroup::new().with_event(RemoteEvent::Jump(Duration::from_millis(10_000).into())).with_event(RemoteEvent::Pause);
        let ip_b = SocketAddr::from(([128, 14, 1, 32], 4));
        let events_b = PlayerEventGroup::new().with_event(RemoteEvent::Ping(Duration::from_millis(1_000).into())).with_event(RemoteEvent::Play);

        let rectified_b = events_b.clone().rectify(ip_b,&events_a, ip_a);
        let rectified_a = events_a.clone().rectify(ip_a, &events_b, ip_b);
        assert_eq!(vec![RemoteEvent::Jump(Duration::from_millis(10_000))], rectified_a.into_events().collect::<Vec<_>>());
        assert_eq!(vec![RemoteEvent::Play], rectified_b.into_events().collect::<Vec<_>>());

        let cascaded_a = events_a.clone().cascade(ip_a, &events_b, ip_b);
        let cascaded_b = events_b.clone().cascade(ip_b, &events_a, ip_a);
        assert_eq!(cascaded_a, cascaded_b);
        assert_eq!(vec![RemoteEvent::Jump(Duration::from_millis(10_000)), RemoteEvent::Play], cascaded_a.into_events().collect::<Vec<_>>());

    }
}