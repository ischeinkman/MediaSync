#![allow(unused)]

use crate::MyResult;
use igd;
use rand;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::ProtocolMessage;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
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
        unimplemented!()
    }
}

pub struct CommunicationThread {
    thread: thread::JoinHandle<()>,
    pub sender: mpsc::Sender<ProtocolMessage>,
    pub recv: mpsc::Receiver<ProtocolMessage>,
}
impl CommunicationThread {
    pub fn start(mut stream: TcpStream) -> MyResult<Self> {
        let (sender, recv) = mpsc::channel::<ProtocolMessage>();
        let (thread_sender, thread_recv) = mpsc::channel::<ProtocolMessage>();
        stream.set_nonblocking(true)?;
        let handle = thread::spawn(move || {
            let _unused = thread_runner(stream, sender, thread_recv);
        });
        Ok(Self {
            sender: thread_sender,
            recv,
            thread: handle,
        })
    }
}

impl Drop for CommunicationThread {
    fn drop(&mut self) {
        self.sender.send(ProtocolMessage::Shutdown);
    }
}

fn thread_runner(
    mut stream: TcpStream,
    mut local_sender: mpsc::Sender<ProtocolMessage>,
    mut thread_recv: mpsc::Receiver<ProtocolMessage>,
) -> MyResult<()> {
    loop {
        // First check for any impending messages
        let mut track_change_buffer = String::new();
        let mut track_change_chars_left = 0;

        let mut cur_buffer = [0; 32];
        let mut cur_bytes_read = 0;
        let cur_slice = &mut cur_buffer[cur_bytes_read..32 - cur_bytes_read];
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
        cur_bytes_read += batch_read;
        if cur_bytes_read > 32 {
            return Err(format!(
                "Error: somehow overflowed our remove message buffer to size {}",
                cur_bytes_read
            )
            .into());
        } else if cur_bytes_read == 32 {
            let message = RawBlock::from_data(cur_buffer).parse()?;

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
                    track_change_chars_left = path_length;
                    let num_payload_chars = 13.min(track_change_chars_left);
                    let payload_raw = &payload[0..num_payload_chars as usize];
                    let payload_parsed = String::from_utf16(&payload_raw)?;
                    track_change_buffer.push_str(&payload_parsed);
                    track_change_chars_left -= num_payload_chars;
                    if track_change_chars_left == 0 {
                        let mut path = String::new();
                        std::mem::swap(&mut path, &mut track_change_buffer);
                        local_sender.send(ProtocolMessage::MediaChange(path));
                    }
                }
                MessageBlock::TrackChangePacket {
                    packet_idx,
                    payload,
                } => {
                    //TODO: verify packet index to buffer length
                    let num_payload_chars = 13.min(track_change_chars_left);
                    let payload_raw = &payload[0..num_payload_chars as usize];
                    let payload_parsed = String::from_utf16(&payload_raw)?;
                    track_change_buffer.push_str(&payload_parsed);
                    track_change_chars_left -= num_payload_chars;
                    if track_change_chars_left == 0 {
                        let mut path = String::new();
                        std::mem::swap(&mut path, &mut track_change_buffer);
                        local_sender.send(ProtocolMessage::MediaChange(path));
                    }
                }
                _ => {}
            }
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
            let bytes_written = stream.write(&block.data)?;
        }
        thread::yield_now();
    }

    Ok(())
}

pub struct Communicator {
    listener: TcpListener,
    public_ip: SocketAddrV4,
    threads: Vec<CommunicationThread>,
}

impl Communicator {
    pub fn open(min_port: u16, max_port: u16) -> MyResult<Self> {
        let listener = open_port(min_port, max_port)?;
        let local_ip = match listener.local_addr()? {
            SocketAddr::V4(s) => s, 
            SocketAddr::V6(s) => {
                return Err(format!("Error: currently do not work with IPv6, but found IP {:?}", s).into());
            }

        };
        let public_ip = map_igd(local_ip)?;
        Ok(Communicator {
            listener, 
            public_ip, 
            threads : Vec::new(),
        })
    }

    pub fn check_incoming(&mut self) -> MyResult<()> {
        let (mut stream, _) = match self.listener.accept() {
            Ok((s, a)) => (s, a), 
            Err(e) => {
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    return Ok(())
                }
                else {
                    return Err(e.into());
                }
            }
        };
        let new_thread = CommunicationThread::start(stream)?;
        self.threads.push(new_thread);
        Ok(())
    }

    pub fn send_message(&mut self, msg : ProtocolMessage) -> MyResult<()> {
        for thread in self.threads.iter_mut() {
            thread.sender.send(msg.clone())?;
        }
        Ok(())
    }

    pub fn check_message(&mut self) -> MyResult<Vec<ProtocolMessage>> {
        let mut retvl = Vec::new();
        for thread in self.threads.iter_mut() {
            loop {
                match thread.recv.try_recv() {
                    Ok(evt) => {
                        retvl.push(evt);
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        break;
                    }
                    Err(e) => {
                        return Err(e.into());
                    }
                }
            }
        }

        Ok(retvl)
    }
}

impl Drop for Communicator {
    fn drop(&mut self) {
        igd::search_gateway(Default::default()).unwrap().remove_port(igd::PortMappingProtocol::TCP, self.public_ip.port());
    }
}
pub fn open_port(min_port: u16, max_port: u16) -> MyResult<TcpListener> {
    let ip = local_network_ip()?;
    let port = random_port(min_port, max_port);
    let addr = SocketAddrV4::new(ip, port);
    let listener = TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

pub fn local_network_ip() -> MyResult<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 40000))?;
    socket.connect((Ipv4Addr::new(8, 8, 8, 8), 4000))?;
    let got_addr = socket.local_addr()?;
    if let IpAddr::V4(ip) = got_addr.ip() {
        Ok(ip)
    } else {
        Err(format!("Error: got invalid IP: {:?}", got_addr.ip()).into())
    }
}

pub fn random_port(from: u16, to: u16) -> u16 {
    let valid_range = to - from;
    let info: u16 = rand::random();
    let offset = info % valid_range;
    from + offset
}

pub fn map_igd(addr: SocketAddrV4) -> MyResult<SocketAddrV4> {
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
