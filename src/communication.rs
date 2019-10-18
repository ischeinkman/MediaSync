use crate::events;
use crate::events::RemoteEvent;
use crate::DebugError;
use crate::MyResult;

use crossbeam::channel::SendError;
use crossbeam::channel::{self, Receiver, Sender};
use crossbeam::sync::ShardedLock;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
mod ipcodec;
pub use ipcodec::*;
mod filetransfer;
pub use filetransfer::*;
mod networking;
pub use networking::*;

mod eventgroup;
use eventgroup::{priority_ordering, PlayerEventGroup};

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
    public_addr: Arc<ShardedLock<Option<SocketAddr>>>,
    client_addresses: Arc<ShardedLock<Vec<SocketAddr>>>,
    server_addresses: Arc<ShardedLock<Vec<SocketAddr>>>,

    message_channel: Sender<ConnectionMessage>,
    event_channel: Sender<RemoteEvent>,
    event_recv: Receiver<RemoteEvent>,

    background_thread: Arc<ShardedLock<Option<JoinHandle<()>>>>,
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
        self.public_addr.read().ok().and_then(|locked| *locked)
    }
    pub fn send_event(&mut self, event: RemoteEvent) -> MyResult<()> {
        self.event_channel.send(event).map_err(|e| e.into())
    }

    pub fn check_events(&mut self) -> MyResult<Vec<RemoteEvent>> {
        let retvl = self.event_recv.try_iter().collect();
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
            Err(SendError(_)) => true,
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
            event_recv: self.event_recv.clone(),
            background_thread: Arc::clone(&self.background_thread),
        }
    }
}

pub struct ConnnectionThread {
    listener: TcpListener,

    public_addr: Option<PublicAddr>,

    handle_info: Option<ConnectionsThreadHandle>,

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
            public_addr: None,
            handle_info: None,
            message_channel: None,
            event_channel: None,
            event_recv: None,

            client_connections: Vec::new(),
            host_connections: Vec::new(),

            stream_buffers: HashMap::new(),
        }
    }

    fn check_new_connections(&mut self) -> MyResult<Option<SocketAddr>> {
        self.listener.set_nonblocking(true)?;
        match self.listener.accept() {
            Ok((stream, addr)) => {
                stream.set_nonblocking(true)?;
                self.host_connections.push(stream);
                if let Some(handle) = self.handle_info.as_ref() {
                    handle.server_addresses.write().unwrap().push(addr);
                }
                Ok(Some(addr))
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    fn process_connection_messages(&mut self) -> MyResult<()> {
        let channel = self
            .message_channel
            .as_ref()
            .cloned()
            .ok_or_else(|| "No message channel found for comms thread!")?;
        for evt in channel.try_iter() {
            self.process_connection_message(evt)?;
        }
        Ok(())
    }
    fn process_connection_message(&mut self, msg: ConnectionMessage) -> MyResult<()> {
        let local_addr = self.listener.local_addr()?;
        match msg {
            ConnectionMessage::ConnectRemote(addr) => {
                let connection = std::net::TcpStream::connect(addr).unwrap();
                connection.set_nonblocking(true).unwrap();
                self.client_connections.push(connection);
                if let Some(handle) = self.handle_info.as_ref() {
                    handle.client_addresses.write().unwrap().push(addr);
                }
            }
            ConnectionMessage::OpenPublic => {
                if self.public_addr.is_none() {
                    let new_public_addr = PublicAddr::request_public(local_addr)?;
                    if let Some(handle) = self.handle_info.as_ref() {
                        *handle.public_addr.write().unwrap() = new_public_addr.addr().into();
                    }

                    self.public_addr = Some(new_public_addr);
                }
            }
            ConnectionMessage::Ping => {}
        }
        Ok(())
    }
    fn check_remote_events(&mut self) -> MyResult<Vec<(SocketAddr, PlayerEventGroup)>> {
        let mut retvl = Vec::new();
        for stream in self
            .host_connections
            .iter_mut()
            .chain(self.client_connections.iter_mut())
        {
            let events = collect_stream_events(stream, &mut self.stream_buffers);
            match events {
                Ok(evts) => {
                    retvl.push((stream.peer_addr().unwrap(), evts));
                }
                Err(e) => {
                    if e.downcast_ref::<io::Error>()
                        .map_or(false, is_disconnection_error)
                    {
                        //TODO: remove bad addresses.
                    } else {
                        return Err(e);
                    }
                }
            }
        }
        retvl.sort_by(|(addr_a, _), (addr_b, _)| priority_ordering(*addr_a, *addr_b));
        Ok(retvl)
    }
    fn check_self_events(&mut self) -> MyResult<PlayerEventGroup> {
        let mut retvl = PlayerEventGroup::new();
        let event_iter = self
            .event_channel
            .as_ref()
            .into_iter()
            .flat_map(|recv| recv.try_iter());
        for evt in event_iter {
            retvl = retvl.with_event(evt);
        }
        Ok(retvl)
    }
    fn broadcast_events(&mut self, events: PlayerEventGroup) -> MyResult<()> {
        let events: Vec<_> = events.into_events().collect();
        for evt in events {
            let event_blocks = evt.as_blocks();
            for block in event_blocks {
                let all_streams = self
                    .client_connections
                    .iter_mut()
                    .chain(self.host_connections.iter_mut());
                let valid_streams =
                    all_streams.filter_map(|stream| match validate_connection(stream) {
                        Ok(Some(_)) => Some(Ok(stream)),
                        Ok(None) => None,
                        Err(e) => Some(Err(e)),
                    });
                for stream in valid_streams {
                    let stream = stream?;
                    stream.write_all(block.as_bytes())?;
                }
            }
        }
        Ok(())
    }
    fn tick(&mut self) {
        if let Err(e) = self.check_new_connections() {
            MyResult::<()>::Err(e).unwrap();
        }
        if let Err(e) = self.process_connection_messages() {
            MyResult::<()>::Err(e).unwrap();
        }
        let self_events = self.check_self_events().unwrap();
        let my_ip = if let Some(ref addr) = self.public_addr.as_ref() {
            addr.addr()
        } else {
            self.listener.local_addr().unwrap()
        };

        let mut raw_events = self.check_remote_events().unwrap();

        let mut rectified_self = self_events;
        let mut cascaded_remotes = raw_events
            .iter()
            .next()
            .map(|(_, evt)| evt.clone())
            .unwrap_or_default();

        for (addr, group) in raw_events.iter_mut() {
            *group = group.clone().rectify(*addr, &rectified_self, my_ip);
            rectified_self = rectified_self.rectify(my_ip, group, *addr);
            cascaded_remotes = cascaded_remotes.cascade(my_ip, group, *addr);
        }

        let fake_addr = SocketAddr::from((my_ip.ip(), my_ip.port() - 1));
        cascaded_remotes = cascaded_remotes.rectify(fake_addr, &rectified_self, my_ip);

        self.broadcast_events(rectified_self).unwrap();

        let events_to_send: Vec<_> = cascaded_remotes.into_events().collect();
        for evt in events_to_send {
            self.event_recv.as_ref().unwrap().send(evt).unwrap();
        }
    }

    fn new_handle(&mut self) -> MyResult<ConnectionsThreadHandle> {
        if let Some(existing) = self.handle_info.as_ref().cloned() {
            Ok(existing)
        } else {
            if self.event_channel.is_some()
                || self.event_recv.is_some()
                || self.message_channel.is_some()
            {
                return Err(
                    "Error: trying to initiate thread, but already have message channels!".into(),
                );
            }
            let local_addr = self.listener.local_addr()?;
            let (message_channel_sender, message_channel_reciever) = channel::unbounded();
            self.message_channel = Some(message_channel_reciever);

            let (event_channel_sender, event_channel_reciever) = channel::unbounded();
            self.event_channel = Some(event_channel_reciever);

            let (event_broadcast_sender, event_broadcast_reciever) = channel::unbounded();
            self.event_recv = Some(event_broadcast_sender);

            let existing_client_addresses: Vec<_> = self
                .client_connections
                .iter()
                .flat_map(|con| con.peer_addr().ok().into_iter())
                .collect();
            let client_addresses = Arc::new(ShardedLock::new(existing_client_addresses));
            let existing_server_addresses: Vec<_> = self
                .host_connections
                .iter()
                .flat_map(|con| con.peer_addr().ok().into_iter())
                .collect();
            let server_addresses = Arc::new(ShardedLock::new(existing_server_addresses));
            let public_addr = Arc::new(ShardedLock::new(None));
            let retvl = ConnectionsThreadHandle {
                local_addr,
                public_addr,
                client_addresses,
                server_addresses,

                message_channel: message_channel_sender,
                event_channel: event_channel_sender,
                event_recv: event_broadcast_reciever,

                background_thread: Arc::new(ShardedLock::new(None)),
            };
            self.handle_info = Some(retvl.clone());

            Ok(retvl)
        }
    }

    pub fn start_sync(&mut self) -> MyResult<ConnectionsThreadHandle> {
        self.new_handle()
    }
    pub fn start(mut self) -> MyResult<ConnectionsThreadHandle> {
        let local_handle = self.start_sync()?;
        let thread_handle = thread::Builder::new()
            .name("Connections Thread".to_owned())
            .spawn(move || loop {
                self.tick();
                thread::yield_now();
            })?;
        *local_handle.background_thread.write().unwrap() = Some(thread_handle);
        Ok(local_handle)
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
        Ok(itr) => itr.collect(),
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

#[cfg(test)]
mod test {
    use super::*;
    use std::net::Ipv4Addr;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_test_thread(
        addr: impl Into<SocketAddr>,
    ) -> (ConnectionsThreadHandle, ConnnectionThread) {
        let addr = addr.into();
        let listener = loop {
            match TcpListener::bind(addr) {
                Ok(l) => break l,
                Err(ref e) if e.kind() == std::io::ErrorKind::AddrInUse => {}
                e => break e.unwrap(),
            }
        };
        let mut thread = ConnnectionThread::new(listener);
        let handle = thread.start_sync().unwrap();
        (handle, thread)
    }

    fn make_connected_threads() -> (
        (ConnectionsThreadHandle, ConnnectionThread),
        (ConnectionsThreadHandle, ConnnectionThread),
    ) {
        let host_addr = (Ipv4Addr::LOCALHOST, 30000);
        let client_addr = (Ipv4Addr::LOCALHOST, 30001);
        let (host_handle, mut host_thread) = make_test_thread(host_addr);
        let (mut client_handle, mut client_thread) = make_test_thread(client_addr);

        client_handle.open_connection(host_addr.into()).unwrap();

        client_thread.tick();
        host_thread.tick();

        ((client_handle, client_thread), (host_handle, host_thread))
    }
    #[test]
    fn test_network_sync() {
        let ((mut client_handle, mut client_thread), (mut host_handle, mut host_thread)) =
            make_connected_threads();

        assert_eq!(
            client_handle.connection_addresses().unwrap(),
            vec![host_handle.local_addr()]
        );
        assert_eq!(host_handle.connection_addresses().unwrap().len(), 1);

        client_handle.send_event(RemoteEvent::Play).unwrap();
        client_thread.tick();
        host_thread.tick();
        let gotten = host_handle.check_events().unwrap();
        assert_eq!(gotten, vec![RemoteEvent::Play]);

        host_handle.send_event(RemoteEvent::Play).unwrap();
        host_thread.tick();
        client_thread.tick();
        let gotten = client_handle.check_events().unwrap();
        assert_eq!(gotten, vec![RemoteEvent::Play]);
    }

    #[test]
    fn test_event_ip_priority() {
        let ((mut client_handle, mut client_thread), (mut host_handle, mut host_thread)) =
            make_connected_threads();
        let calc_a = priority_ordering(client_handle.local_addr(), host_handle.local_addr());
        let calc_b = priority_ordering(
            host_handle.connection_addresses().unwrap()[0],
            host_handle.local_addr(),
        );
        let calc_c = priority_ordering(
            client_handle.local_addr(),
            client_handle.connection_addresses().unwrap()[0],
        );

        assert_eq!(calc_a, calc_b);
        assert_eq!(calc_b, calc_c);

        let host_has_priority = calc_a == std::cmp::Ordering::Less;

        client_handle
            .send_event(RemoteEvent::Jump(Duration::from_millis(3000)))
            .unwrap();
        client_thread.tick();
        host_handle
            .send_event(RemoteEvent::Jump(Duration::from_millis(2000)))
            .unwrap();
        host_thread.tick();

        client_thread.tick();
        host_thread.tick();
        client_thread.tick();
        host_thread.tick();

        if host_has_priority {
            assert_eq!(host_handle.check_events().unwrap(), vec![]);
            assert_eq!(
                client_handle.check_events().unwrap(),
                vec![RemoteEvent::Jump(Duration::from_millis(2000))]
            );
        } else {
            assert_eq!(client_handle.check_events().unwrap(), vec![]);
            assert_eq!(
                host_handle.check_events().unwrap(),
                vec![RemoteEvent::Jump(Duration::from_millis(3000))]
            );
        }
    }

    #[test]
    fn test_jump_event_priority() {
        let ((mut client_handle, mut client_thread), (mut host_handle, mut host_thread)) =
            make_connected_threads();
        let calc_a = priority_ordering(client_handle.local_addr(), host_handle.local_addr());
        let calc_b = priority_ordering(
            host_handle.connection_addresses().unwrap()[0],
            host_handle.local_addr(),
        );
        let calc_c = priority_ordering(
            client_handle.local_addr(),
            client_handle.connection_addresses().unwrap()[0],
        );

        assert_eq!(calc_a, calc_b);
        assert_eq!(calc_b, calc_c);

        let host_has_priority = calc_a == std::cmp::Ordering::Less;

        host_thread.tick();
        client_thread.tick();

        let jump_evt = RemoteEvent::Jump(Duration::from_millis(3000));
        let ping_evt = RemoteEvent::Ping {
            payload: Duration::from_millis(3000).into(),
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap(),
        };
        if host_has_priority {
            client_handle.send_event(jump_evt.clone()).unwrap();
            host_handle.send_event(ping_evt.clone()).unwrap();
            host_thread.tick();
            client_thread.tick();
            host_thread.tick();
            client_thread.tick();

            assert_eq!(client_handle.check_events().unwrap(), vec![]);
            assert_eq!(host_handle.check_events().unwrap(), vec![jump_evt]);
        } else {
            client_handle.send_event(ping_evt.clone()).unwrap();
            host_handle.send_event(jump_evt.clone()).unwrap();
            host_thread.tick();
            client_thread.tick();
            host_thread.tick();
            client_thread.tick();

            assert_eq!(client_handle.check_events().unwrap(), vec![jump_evt]);
            assert_eq!(host_handle.check_events().unwrap(), vec![]);
        }
    }
    #[test]
    fn test_open_overwrite() {
        let ((mut client_handle, mut client_thread), (mut host_handle, mut host_thread)) =
            make_connected_threads();
        let calc_a = priority_ordering(client_handle.local_addr(), host_handle.local_addr());
        let calc_b = priority_ordering(
            host_handle.connection_addresses().unwrap()[0],
            host_handle.local_addr(),
        );
        let calc_c = priority_ordering(
            client_handle.local_addr(),
            client_handle.connection_addresses().unwrap()[0],
        );

        assert_eq!(calc_a, calc_b);
        assert_eq!(calc_b, calc_c);

        let host_has_priority = calc_a == std::cmp::Ordering::Less;

        host_thread.tick();
        client_thread.tick();

        let ping_evt = RemoteEvent::Ping {
            payload: Duration::from_millis(3000).into(),
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap(),
        };
        if host_has_priority {
            client_handle.send_event(ping_evt.clone()).unwrap();
            client_handle.send_event(RemoteEvent::Play).unwrap();
            client_handle
                .send_event(RemoteEvent::MediaOpen("Test".to_owned()))
                .unwrap();
            host_handle.send_event(RemoteEvent::Play).unwrap();
            host_handle
                .send_event(RemoteEvent::Jump(Duration::from_millis(2000)))
                .unwrap();
            host_handle
                .send_event(RemoteEvent::RequestTransfer("Test2".to_owned()))
                .unwrap();
            host_thread.tick();
            client_thread.tick();
            host_thread.tick();
            client_thread.tick();

            assert_eq!(client_handle.check_events().unwrap(), vec![]);
            assert_eq!(
                host_handle.check_events().unwrap(),
                vec![RemoteEvent::MediaOpen("Test".to_owned())]
            );
        } else {
            host_handle.send_event(ping_evt.clone()).unwrap();
            host_handle.send_event(RemoteEvent::Play).unwrap();
            host_handle
                .send_event(RemoteEvent::MediaOpen("Test".to_owned()))
                .unwrap();
            client_handle.send_event(RemoteEvent::Play).unwrap();
            client_handle
                .send_event(RemoteEvent::Jump(Duration::from_millis(2000)))
                .unwrap();
            client_handle
                .send_event(RemoteEvent::RequestTransfer("Test2".to_owned()))
                .unwrap();
            host_thread.tick();
            client_thread.tick();
            host_thread.tick();
            client_thread.tick();

            assert_eq!(
                client_handle.check_events().unwrap(),
                vec![RemoteEvent::MediaOpen("Test".to_owned())]
            );
            assert_eq!(host_handle.check_events().unwrap(), vec![]);
        }
    }
}
