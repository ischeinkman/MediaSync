use crate::MyResult;
use crate::{DebugError, ProtocolMessage};
use igd;
use rand;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

pub fn decode_ipv4(digits: [char; 6]) -> u32 {
    let mut retvl = 0;
    for (idx, &c) in digits.iter().enumerate() {
        let power = 2 - (idx as u32);
        let scaled = 62u32.pow(power);
        let coeff = if c >= '0' && c <= '9' {
            c as u32 - '0' as u32
        } else if c >= 'a' && c <= 'z' {
            c as u32 - 'a' as u32 + 10
        } else if c >= 'A' && c <= 'Z' {
            c as u32 - 'A' as u32 + 36
        } else {
            0
        };
        retvl += coeff * scaled;
    }

    retvl
}

pub fn decode_port(digits: [char; 3]) -> u16 {
    let mut retvl = 0u16;
    for (idx, &c) in digits.iter().enumerate() {
        let power = 2 - (idx as u32);
        let scaled = 62u16.pow(power);
        let coeff = if c >= '0' && c <= '9' {
            c as u16 - '0' as u16
        } else if c >= 'a' && c <= 'z' {
            c as u16 - 'a' as u16 + 10
        } else if c >= 'A' && c <= 'Z' {
            c as u16 - 'A' as u16 + 36
        } else {
            0
        };
        retvl += coeff * scaled;
    }
    retvl
}

pub fn encode_port(port: u16) -> [char; 3] {
    let mut retvl = ['\0'; 3];
    let mut left = u32::from(port);
    let mut cur_idx = 2;
    while left > 0 {
        let digit = left % 62;
        let digit_char = if digit < 10 {
            std::char::from_digit(digit, 10).unwrap()
        } else if digit < 36 {
            let offset = digit - 10 + u32::from('a');
            std::char::from_u32(offset).unwrap()
        } else {
            let offset = digit - 36 + u32::from('A');
            std::char::from_u32(offset).unwrap()
        };
        retvl[cur_idx] = digit_char;
        if left < 62 {
            break;
        }
        cur_idx -= 1;
        left /= 62;
    }

    retvl
}

pub fn encode_ipv4(ip: u32) -> [char; 6] {
    let mut retvl = ['\0'; 6];
    let mut left = ip;
    let mut cur_idx = 5;
    while left > 0 {
        let digit = left % 62;
        let digit_char = if digit < 10 {
            std::char::from_digit(digit, 10).unwrap()
        } else if digit < 36 {
            let offset = digit - 10 + u32::from('a');
            std::char::from_u32(offset).unwrap()
        } else {
            let offset = digit - 36 + u32::from('A');
            std::char::from_u32(offset).unwrap()
        };
        retvl[cur_idx] = digit_char;
        if left < 62 {
            break;
        }
        left /= 62;
        cur_idx -= 1;
    }

    retvl
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Default)]
pub struct RawBlock {
    data: [u8; 32],
}

impl RawBlock {
    pub fn from_data(data: [u8; 32]) -> Self {
        Self { data }
    }
}

impl RawBlock {
    pub fn parse(self) -> MyResult<MessageBlock> {
        let kind = self.data[0];
        if kind == 0 {
            let command_byte = self.data[1];
            let command = TimedCommandKind::from_kind_byte(command_byte).ok_or_else(|| {
                format!("Error: got invalid TimedCommand kind {:x}", command_byte)
            })?;
            let nano_bytes = [
                self.data[2],
                self.data[3],
                self.data[4],
                self.data[5],
                self.data[6],
                self.data[7],
                self.data[8],
                self.data[9],
            ];
            let nanoseconds = u64::from_le_bytes(nano_bytes);
            Ok(MessageBlock::TimedCommand {
                kind: command,
                nanoseconds,
            })
        } else if kind == 1 {
            let length_bytes = [self.data[1], self.data[2], self.data[3], self.data[4]];
            let path_length = u32::from_le_bytes(length_bytes);
            let mut payload = [0u16; 13];
            for (idx, cur_byte) in payload.iter_mut().enumerate() {
                let cur_bytes = [self.data[2 * idx + 6], self.data[2 * idx + 7]];
                *cur_byte = u16::from_le_bytes(cur_bytes);
            }
            Ok(MessageBlock::TrackChangeHeader {
                path_length,
                payload,
            })
        } else if kind == 2 {
            let idx_bytes = [self.data[1], self.data[2], self.data[3], self.data[4]];
            let packet_idx = u32::from_le_bytes(idx_bytes);
            let mut payload = [0u16; 13];
            for (idx, cur_byte) in payload.iter_mut().enumerate() {
                let cur_bytes = [self.data[2 * idx + 6], self.data[2 * idx + 7]];
                *cur_byte = u16::from_le_bytes(cur_bytes);
            }
            Ok(MessageBlock::TrackChangePacket {
                packet_idx,
                payload,
            })
        } else {
            Err(format!("Error: got invalid message kind {:x}.", kind).into())
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum TimedCommandKind {
    Ping,
    Jump,
    Pause,
    Play,
}

impl TimedCommandKind {
    pub fn from_kind_byte(byte: u8) -> Option<TimedCommandKind> {
        match byte {
            0 => Some(TimedCommandKind::Ping),
            1 => Some(TimedCommandKind::Jump),
            2 => Some(TimedCommandKind::Pause),
            3 => Some(TimedCommandKind::Play),
            _ => None,
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub enum MessageBlock {
    TimedCommand {
        kind: TimedCommandKind,
        nanoseconds: u64,
    },
    TrackChangeHeader {
        path_length: u32, // In UTF16 characters
        payload: [u16; 13],
    },
    TrackChangePacket {
        packet_idx: u32, // Always > 0
        payload: [u16; 13],
    },
}

impl MessageBlock {
    pub fn into_raw(self) -> RawBlock {
        //TODO: this
        RawBlock::default()
    }
}

pub struct CommunicationThread {
    thread: Arc<thread::JoinHandle<()>>,
    pub sender: mpsc::Sender<ProtocolMessage>,
    pub recv: Arc<Mutex<mpsc::Receiver<ProtocolMessage>>>,
}
impl CommunicationThread {
    pub fn start(
        listener: Arc<RwLock<TcpListener>>,
        server_addrs: Arc<RwLock<Vec<SocketAddr>>>,
        client_streams: Arc<RwLock<Vec<TcpStream>>>,
        server_streams: Arc<RwLock<Vec<TcpStream>>>,
    ) -> MyResult<Self> {
        let (sender, recv) = mpsc::channel::<ProtocolMessage>();
        let (thread_sender, thread_recv) = mpsc::channel::<ProtocolMessage>();
        let handle = thread::spawn(move || {
            let _unused = thread_runner(
                listener,
                server_addrs,
                client_streams,
                server_streams,
                sender,
                thread_recv,
            );
        });
        Ok(Self {
            sender: thread_sender,
            recv: Arc::new(Mutex::new(recv)),
            thread: Arc::new(handle),
        })
    }

    pub fn handle(&self) -> CommunicationThread {
        CommunicationThread {
            thread: Arc::clone(&self.thread),
            sender: self.sender.clone(),
            recv: Arc::clone(&self.recv),
        }
    }
}

impl Drop for CommunicationThread {
    fn drop(&mut self) {
        //Ignore the error, since we are just trying to shut down the thread anyway
        self.sender.send(ProtocolMessage::Shutdown).unwrap_or_default();
    }
}
fn process_stream(
    stream: &mut TcpStream,
    local_sender: &mpsc::Sender<ProtocolMessage>,
    track_change_buffers: &mut HashMap<SocketAddr, (String, u32)>,
    command_buffers: &mut HashMap<SocketAddr, ([u8; 32], usize)>,
) -> MyResult<()> {
    let buffer_key = stream.peer_addr()?;

    let (ref mut cur_buffer, ref mut cur_bytes_read) =
        command_buffers.entry(buffer_key).or_insert(([0; 32], 0));
    let (ref mut track_change_buffer, ref mut track_change_chars_left) = track_change_buffers
        .entry(buffer_key)
        .or_insert((String::new(), 0));

    let cur_slice = &mut cur_buffer[*cur_bytes_read..32 - *cur_bytes_read];
    let batch_read = match stream.read(cur_slice) {
        Ok(n) => n,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                0
            } else {
                return Err(e.into());
            }
        }
    };
    *cur_bytes_read += batch_read;
    if *cur_bytes_read > 32 {
        return Err(format!(
            "Error: somehow overflowed our remove message buffer to size {}",
            cur_bytes_read
        )
        .into());
    } else if *cur_bytes_read == 32 {
        let message = RawBlock::from_data(*cur_buffer).parse()?;

        match message {
            MessageBlock::TimedCommand { kind, nanoseconds } => {
                let tm = Duration::from_nanos(nanoseconds);
                let command_to_send = match kind {
                    TimedCommandKind::Play => ProtocolMessage::Play(tm),
                    TimedCommandKind::Pause => ProtocolMessage::Pause(tm),
                    TimedCommandKind::Jump => ProtocolMessage::Jump(tm),
                    TimedCommandKind::Ping => ProtocolMessage::TimePing(tm),
                };
                local_sender.send(command_to_send)?;
            }
            MessageBlock::TrackChangeHeader {
                path_length,
                payload,
            } => {
                track_change_buffer.clear();
                *track_change_chars_left = path_length;
                let num_payload_chars = 13.min(*track_change_chars_left);
                let payload_raw = &payload[0..num_payload_chars as usize];
                let payload_parsed = String::from_utf16(&payload_raw)?;
                track_change_buffer.push_str(&payload_parsed);
                *track_change_chars_left -= num_payload_chars;
                if *track_change_chars_left == 0 {
                    let mut path = String::new();
                    std::mem::swap(&mut path, track_change_buffer);
                    local_sender.send(ProtocolMessage::MediaChange(path))?;
                }
            }
            MessageBlock::TrackChangePacket {
                packet_idx: _packet_idx,
                payload,
            } => {
                //TODO: verify packet index to buffer length
                let num_payload_chars = 13.min(*track_change_chars_left);
                let payload_raw = &payload[0..num_payload_chars as usize];
                let payload_parsed = String::from_utf16(&payload_raw)?;
                track_change_buffer.push_str(&payload_parsed);
                *track_change_chars_left -= num_payload_chars;
                if *track_change_chars_left == 0 {
                    let mut path = String::new();
                    std::mem::swap(&mut path, track_change_buffer);
                    local_sender.send(ProtocolMessage::MediaChange(path))?;
                }
            }
        }
    }
    Ok(())
}
fn check_incoming(
    listener: &RwLock<TcpListener>,
    server_streams: &RwLock<Vec<TcpStream>>,
    server_addrs: &RwLock<Vec<SocketAddr>>,
) -> MyResult<()> {
    let listener = listener.write().map_err(DebugError::into_myerror)?;

    let (stream, _) = match listener.accept() {
        Ok((s, a)) => (s, a),
        Err(e) => {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(());
            } else {
                return Err(e.into());
            }
        }
    };
    stream.set_nonblocking(true)?;
    let mut server_lock = server_streams.write().map_err(DebugError::into_myerror)?;
    let mut addr_lock = server_addrs.write().map_err(DebugError::into_myerror)?;
    addr_lock.push(stream.peer_addr()?);
    server_lock.push(stream);
    Ok(())
}
fn thread_runner(
    listener: Arc<RwLock<TcpListener>>,
    server_addrs: Arc<RwLock<Vec<SocketAddr>>>,
    client_streams: Arc<RwLock<Vec<TcpStream>>>,
    server_streams: Arc<RwLock<Vec<TcpStream>>>,
    local_sender: mpsc::Sender<ProtocolMessage>,
    thread_recv: mpsc::Receiver<ProtocolMessage>,
) -> MyResult<()> {
    let mut track_change_buffers: HashMap<SocketAddr, (String, u32)> = HashMap::new();
    let mut command_buffers: HashMap<SocketAddr, ([u8; 32], usize)> = HashMap::new();
    loop {
        check_incoming(&listener, &server_streams, &server_addrs)?;
        let mut client_lock = client_streams.write().map_err(DebugError::into_myerror)?;
        let mut server_lock = server_streams.write().map_err(DebugError::into_myerror)?;
        // First check for any impending messages from remote hosts
        for stream in client_lock.iter_mut().chain(server_lock.iter_mut()) {
            process_stream(
                stream,
                &local_sender,
                &mut track_change_buffers,
                &mut command_buffers,
            )?;
        }

        // Next respond to local messages if we have any
        let data_to_send = match thread_recv.try_recv() {
            Ok(ProtocolMessage::TimePing(tm)) => {
                let message = MessageBlock::TimedCommand {
                    kind: TimedCommandKind::Ping,
                    nanoseconds: tm.as_nanos() as u64,
                };
                message.into_raw().into()
            }
            Ok(ProtocolMessage::Play(tm)) => {
                let message = MessageBlock::TimedCommand {
                    kind: TimedCommandKind::Play,
                    nanoseconds: tm.as_nanos() as u64,
                };
                message.into_raw().into()
            }
            Ok(ProtocolMessage::Pause(tm)) => {
                let message = MessageBlock::TimedCommand {
                    kind: TimedCommandKind::Pause,
                    nanoseconds: tm.as_nanos() as u64,
                };
                message.into_raw().into()
            }
            Ok(ProtocolMessage::Jump(tm)) => {
                let message = MessageBlock::TimedCommand {
                    kind: TimedCommandKind::Jump,
                    nanoseconds: tm.as_nanos() as u64,
                };
                message.into_raw().into()
            }
            Ok(ProtocolMessage::Shutdown) => {
                break;
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(e) => {
                return Err(e.into());
            }
            _ => None,
        };
        if let Some(block) = data_to_send {
            for stream in client_lock.iter_mut().chain(server_lock.iter_mut()) {
                stream.write_all(&block.data)?;
            }
        }
        thread::yield_now();
    }

    Ok(())
}

pub struct Communicator {
    pub listener: Arc<RwLock<TcpListener>>,
    pub public_ip: SocketAddrV4,
    pub client_addrs: Arc<RwLock<Vec<SocketAddr>>>,
    pub server_addrs: Arc<RwLock<Vec<SocketAddr>>>,
    pub client_streams: Arc<RwLock<Vec<TcpStream>>>,
    pub server_streams: Arc<RwLock<Vec<TcpStream>>>,
    pub communication_thread: Option<CommunicationThread>,
}

impl Communicator {
    pub fn open(min_port: u16, max_port: u16) -> MyResult<Self> {
        let listener = open_port(min_port, max_port)?;
        let local_ip = match listener.local_addr()? {
            SocketAddr::V4(s) => s,
            SocketAddr::V6(s) => {
                return Err(format!(
                    "Error: currently do not work with IPv6, but found IP {:?}",
                    s
                )
                .into());
            }
        };
        let public_ip = map_igd(local_ip)?;
        let mut retvl = Communicator {
            listener: Arc::new(RwLock::new(listener)),
            public_ip,
            client_addrs: Arc::new(RwLock::new(Vec::new())),
            server_addrs: Arc::new(RwLock::new(Vec::new())),
            client_streams: Arc::new(RwLock::new(Vec::new())),
            server_streams: Arc::new(RwLock::new(Vec::new())),
            communication_thread: None,
        };
        retvl.start_background()?;
        Ok(retvl)
    }

    pub fn handle(&self) -> Communicator {
        Communicator {
            listener: Arc::clone(&self.listener),
            public_ip: self.public_ip,
            client_addrs: Arc::clone(&self.client_addrs),
            server_addrs: Arc::clone(&self.server_addrs),
            client_streams: Arc::clone(&self.client_streams),
            server_streams: Arc::clone(&self.server_streams),
            communication_thread: self.communication_thread.as_ref().map(|t| t.handle()),
        }
    }

    pub fn send_message(&self, msg: ProtocolMessage) -> MyResult<()> {
        if let Some(thread) = self.communication_thread.as_ref() {
            thread.sender.send(msg.clone())?;
            Ok(())
        } else {
            Err(
                "Error: tried sending message to non-existant background thread!"
                    .to_owned()
                    .into(),
            )
        }
    }

    pub fn check_message(&self) -> MyResult<Vec<ProtocolMessage>> {
        if let Some(thread) = self.communication_thread.as_ref() {
            let lock = thread.recv.lock().map_err(DebugError::into_myerror)?;
            Ok(lock.try_iter().collect())
        } else {
            Ok(Vec::new())
        }
    }

    pub fn open_remote(&self, remote: SocketAddr) -> MyResult<()> {
        let mut addr_lock = self
            .client_addrs
            .write()
            .map_err(DebugError::into_myerror)?;
        let mut stream_lock = self
            .client_streams
            .write()
            .map_err(DebugError::into_myerror)?;
        let stream = TcpStream::connect(remote)?;
        stream.set_nonblocking(true)?;
        stream_lock.push(stream);
        addr_lock.push(remote);
        Ok(())
    }

    fn start_background(&mut self) -> MyResult<()> {
        self.communication_thread = Some(CommunicationThread::start(
            Arc::clone(&self.listener),
            Arc::clone(&self.server_addrs),
            Arc::clone(&self.client_streams),
            Arc::clone(&self.server_streams),
        )?);
        Ok(())
    }
}

impl<T> DebugError for std::sync::PoisonError<T> {
    fn message(self) -> String {
        format!("std::sync::PoisonError : {:?}", self)
    }
}

impl Drop for Communicator {
    fn drop(&mut self) {
        if !self.public_ip.ip().is_private() {
            igd::search_gateway(Default::default())
                .map_err(Box::<dyn std::error::Error>::from)
                .and_then(|g| {
                    g.remove_port(igd::PortMappingProtocol::TCP, self.public_ip.port())
                        .map_err(|e| e.into())
                }).unwrap();
        }
    }
}
fn open_port(min_port: u16, max_port: u16) -> MyResult<TcpListener> {
    let ip = local_network_ip()?;
    let port = random_port(min_port, max_port);
    let addr = SocketAddrV4::new(ip, port);
    let listener = TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
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

fn map_igd(addr: SocketAddrV4) -> MyResult<SocketAddrV4> {
    const DEFAULT_LEASE_DURATION: u32 = 60 * 60 * 2;
    let opts = igd::SearchOptions::default();

    let description = format!("MediaSync communication port for local IP {:?}", addr.ip());
    let gateway = igd::search_gateway(opts)?;
    let retvl = gateway.get_any_address(
        igd::PortMappingProtocol::TCP,
        addr,
        DEFAULT_LEASE_DURATION,
        &description,
    )?;
    Ok(retvl)
}
