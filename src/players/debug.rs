use crate::events::{PlayerEvent, TimePing};
use crate::traits::{MediaPlayer, MediaPlayerList};
use crate::MyResult;

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

pub enum DebugEventSink {
    StdOut,
    File(File),
}

impl Write for DebugEventSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            DebugEventSink::File(f) => f.write(buf),
            DebugEventSink::StdOut => io::stdout().write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            DebugEventSink::File(f) => f.flush(),
            DebugEventSink::StdOut => io::stdout().flush(),
        }
    }
}

pub struct SinkDebugPlayer {
    previous_ping: TimePing,
    previous_ping_time: Instant,
    sink: DebugEventSink,
}

impl SinkDebugPlayer {
    pub fn new(sink: DebugEventSink) -> Self {
        Self {
            previous_ping: TimePing::from_micros(0),
            previous_ping_time: Instant::now(),
            sink,
        }
    }
}

pub enum DebugPlayer {
    Sink(SinkDebugPlayer),
    Socket(SocketDebugPlayer),
}

impl MediaPlayer for DebugPlayer {
    fn name(&self) -> &str {
        match self {
            DebugPlayer::Sink(p) => match &p.sink {
                DebugEventSink::StdOut => "Standard Output",
                DebugEventSink::File(_f) => "Log File",
            },
            DebugPlayer::Socket(_s) => "Socket-Based Text Debug",
        }
    }
    fn send_event(&mut self, event: PlayerEvent) -> MyResult<()> {
        match self {
            DebugPlayer::Sink(p) => {
                writeln!(&mut p.sink, "Event: {:?}", event)?;
            }
            DebugPlayer::Socket(s) => {
                for stream in s.connection_info.iter_mut() {
                    writeln!(stream, "Got sent an event: {:?}", event)?;
                }
            }
        }
        Ok(())
    }
    fn check_events(&mut self, _time_since_previous: Duration) -> MyResult<Vec<PlayerEvent>> {
        if let DebugPlayer::Socket(s) = self {
            loop {
                match s.listener.accept() {
                    Ok((stream, _addr)) => {
                        stream.set_nonblocking(true)?;
                        s.connection_info.push(stream);
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                        break;
                    }
                    Err(e) => {
                        return Err(e.into());
                    }
                }
            }
            let mut retvl = Vec::new();
            for stream in s.connection_info.iter_mut() {
                let mut res = Vec::new();
                let mut buff = [0; 32];
                loop {
                    match stream.read(&mut buff) {
                        Ok(0) => {
                            break;
                        }
                        Ok(b) => {
                            res.extend_from_slice(&buff[..b]);
                        }
                        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                            break;
                        }
                        Err(e) => {
                            return Err(e.into());
                        }
                    }
                }
                let msg = String::from_utf8_lossy(&res);
                for ln in msg.lines() {
                    let ln = ln.trim();
                    if ln.starts_with('p') {
                        retvl.push(PlayerEvent::Pause);
                    } else if ln.starts_with('P') {
                        retvl.push(PlayerEvent::Play);
                    } else if ln.starts_with('j') {
                        let time_portion = ln
                            .splitn(2, ' ')
                            .nth(1)
                            .ok_or_else(|| format!("Error parsing command {}", ln))?;
                        let seconds: f64 = time_portion.parse()?;
                        let time = Duration::from_secs_f64(seconds);
                        retvl.push(PlayerEvent::Jump(time));
                    } else if ln.starts_with('o') {
                        let url = ln
                            .splitn(2, ' ')
                            .nth(1)
                            .ok_or_else(|| format!("Error parsing command {}", ln))?;
                        retvl.push(PlayerEvent::MediaOpen(url.to_owned()));
                    }
                }
            }
            Ok(retvl)
        } else {
            Ok(Vec::new())
        }
    }
    fn ping(&self) -> MyResult<TimePing> {
        match self {
            DebugPlayer::Sink(s) => {
                let dt = Instant::now() - s.previous_ping_time;
                let ping_time = s.previous_ping.time() + dt;
                Ok(TimePing::from_duration(ping_time))
            }
            DebugPlayer::Socket(s) => {
                let dt = if s.is_paused {
                    Duration::from_nanos(0)
                } else {
                    Instant::now() - s.last_position_modification
                };
                let new_pos = s.last_position + dt;
                Ok(TimePing::from_duration(new_pos))
            }
        }
    }
    fn on_ping(&mut self, ping: TimePing) -> MyResult<()> {
        match self {
            DebugPlayer::Sink(s) => {
                s.previous_ping = ping;
                s.previous_ping_time = Instant::now();
            }
            DebugPlayer::Socket(s) => {
                s.last_position = ping.time();
                s.last_position_modification = Instant::now();
            }
        }
        Ok(())
    }
}

pub struct SocketDebugPlayer {
    last_position_modification: Instant,
    last_position: Duration,
    is_paused: bool,
    listener: TcpListener,
    connection_info: Vec<TcpStream>,
}
pub struct DebugPlayerList {}

impl MediaPlayerList for DebugPlayerList {
    type Player = DebugPlayer;

    fn new() -> Self {
        DebugPlayerList {}
    }

    fn list_name(&self) -> &str {
        "Debug"
    }
    fn available(&mut self) -> MyResult<Vec<String>> {
        Ok(vec![
            "Standard Output".to_owned(),
            "Log File".to_owned(),
            "Socket based".to_owned(),
        ])
    }
    fn open(&mut self, name: &str) -> MyResult<Self::Player> {
        match name {
            "Standard Output" => Ok(DebugPlayer::Sink(SinkDebugPlayer::new(
                DebugEventSink::StdOut,
            ))),
            "Log File" => {
                let path = "/home/ilan/mediasync_debug_player_log.txt";
                let file = fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(path)?;
                Ok(DebugPlayer::Sink(SinkDebugPlayer::new(
                    DebugEventSink::File(file),
                )))
            }
            "Socket based" => {
                let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 49234)))?;
                let player = SocketDebugPlayer {
                    last_position_modification: Instant::now(),
                    last_position: Duration::from_nanos(0),
                    is_paused: false,
                    listener,
                    connection_info: Vec::new(),
                };
                Ok(DebugPlayer::Socket(player))
            }
            other => Err(format!("Invalid debug player name : {}", other).into()),
        }
    }
}
