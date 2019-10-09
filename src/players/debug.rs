
use crate::traits::{MediaPlayer, MediaPlayerList};
use crate::MyResult;
use crate::events::{PlayerEvent, TimePing};

use std::fs::{self, File};
use std::io::{self, Write};
use std::time::{Duration, Instant};

pub enum DebugEventSink {
    StdOut, 
    File(File),
}

impl Write for DebugEventSink {
    fn write(&mut self, buf : &[u8]) -> io::Result<usize> {
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

pub struct DebugPlayer {
    previous_ping : TimePing, 
    previous_ping_time : Instant,
    sink : DebugEventSink,
}

impl DebugPlayer {
    pub fn new(sink : DebugEventSink) -> Self {
        Self {
            previous_ping : TimePing::from_micros(0),
            previous_ping_time : Instant::now(), 
            sink,
        }
    }
}

impl MediaPlayer for DebugPlayer {
    fn name(&self) -> &str {
        match &self.sink {
            DebugEventSink::StdOut => "Standard Output",
            DebugEventSink::File(_f) => {
                "Log File"
            }
        }
    }
    fn send_event(&mut self, event: PlayerEvent) -> MyResult<()> {
        writeln!(&mut self.sink, "Event: {:?}", event)?;
        Ok(())
    }
    fn check_events(&mut self, _time_since_previous : Duration) -> MyResult<Vec<PlayerEvent>> {
        //writeln!(&mut self.sink, "check_events called.")?;
        Ok(Vec::new())
    }
    fn ping(&self) -> MyResult<TimePing> {
        let dt = Instant::now() - self.previous_ping_time;
        let ping_time = self.previous_ping.time() + dt;
        Ok(TimePing::from_duration(ping_time))
    }
    fn on_ping(&mut self, ping : TimePing) -> MyResult<()> {
        self.previous_ping = ping; 
        self.previous_ping_time = Instant::now();
        //writeln!(&mut self.sink, "On ping called.")?;
        Ok(())
    }
}

pub struct DebugPlayerList {}

impl MediaPlayerList for DebugPlayerList {
    type Player = DebugPlayer;

    fn new() -> Self {
        DebugPlayerList{}
    }

    fn list_name(&self) -> &str {
        "Debug"
    }
    fn available(&mut self) -> MyResult<Vec<String>> {
        Ok(vec![
            "Standard Output".to_owned(), 
            "Log File".to_owned(),
        ])
    }
    fn open(&mut self, name : &str) -> MyResult<Self::Player> {
        match name {
            "Standard Output" => Ok(DebugPlayer::new(DebugEventSink::StdOut)),
            "Log File" => {
                let path = "/home/ilan/mediasync_debug_player_log.txt";
                let file = fs::OpenOptions::new().append(true).create(true).open(path).unwrap();
                Ok(DebugPlayer::new(DebugEventSink::File(file)))
            }
            other => {
                Err(format!("Invalid debug player name : {}", other).into())
            }
        }
    }
}