use crate::events;
use crate::events::RemoteEvent;
use crate::DebugError;
use crate::MyResult;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
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

            let mut client_events = Vec::new();
            for stream in self.client_connections.iter_mut() {
                let cur_events = collect_stream_events(stream, &mut self.stream_buffers).unwrap();
                client_events.extend(cur_events);
            }

            let self_events: Vec<_> = self
                .event_channel
                .as_ref()
                .into_iter()
                .flat_map(|channel| channel.try_iter())
                .collect();

            let mut host_events = Vec::new();
            for stream in self.host_connections.iter_mut() {
                let cur_events = collect_stream_events(stream, &mut self.stream_buffers).unwrap();
                host_events.extend(cur_events);
            }

            let relevant_client_events = client_events.clone();
            let relevant_self_events =
                filter_lower_events(self_events.clone(), client_events.clone());
            let relevant_host_events = filter_lower_events(
                host_events,
                client_events.iter().chain(self_events.iter()).cloned(),
            );

            let mut jumped = false;
            for evt in relevant_host_events {
                if let RemoteEvent::Jump(_) = evt {
                    jumped = true;
                }
                self.event_recv.as_ref().unwrap().send(evt.clone()).unwrap();
            }

            for evt in relevant_self_events {
                if let RemoteEvent::Jump(_) = evt {
                    jumped = true;
                } else if let RemoteEvent::Ping(_) = evt {
                    if jumped {
                        continue;
                    }
                }
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
            for evt in relevant_client_events {
                if let RemoteEvent::Jump(_) = evt {
                    jumped = true;
                } else if let RemoteEvent::Ping(_) = evt {
                    if jumped {
                        continue;
                    }
                }
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
) -> MyResult<Vec<RemoteEvent>> {
    match StreamEventIter::from_buffer_map(stream, buffers) {
        Ok(itr) => itr.collect(),
        Err(ref e) if is_disconnection_error(e) => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

fn filter_lower_events(
    cur_events: impl IntoIterator<Item = RemoteEvent>,
    previous_events: impl IntoIterator<Item = RemoteEvent>,
) -> impl Iterator<Item = RemoteEvent> {
    let mut prev_has_playpause = false;
    let mut prev_has_jump = false;
    let mut prev_has_ping = false;
    let mut prev_has_mediaopen = false;
    for prev in previous_events {
        match prev {
            RemoteEvent::Play | RemoteEvent::Pause => {
                prev_has_playpause = true;
            }
            RemoteEvent::Jump(_) => {
                prev_has_jump = true;
            }
            RemoteEvent::Ping(_) => {
                prev_has_ping = true;
            }
            RemoteEvent::MediaOpen(_) => {
                prev_has_mediaopen = true;
            }
            _ => {}
        }
    }
    cur_events.into_iter().filter(move |evt| match evt {
        RemoteEvent::Play | RemoteEvent::Pause => !prev_has_playpause,
        RemoteEvent::Jump(_) => !prev_has_jump,
        RemoteEvent::Ping(_) => !prev_has_jump && !prev_has_ping,
        RemoteEvent::MediaOpen(_) => !prev_has_mediaopen,
        _ => true,
    })
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
