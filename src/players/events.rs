use std::time::Duration;
#[derive(Clone, Eq, PartialEq, Debug, Hash)]
pub enum PlayerEvent {
    Pause,
    Play,
    Jump(Duration),
    MediaOpen(String),
    Shutdown,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct TimePing {
    time: Duration,
}

impl TimePing {
    pub fn from_micros(micros: u64) -> TimePing {
        Duration::from_micros(micros).into()
    }
    pub fn from_duration(time: Duration) -> TimePing {
        TimePing { time }
    }
    pub fn as_micros(&self) -> u64 {
        self.time.as_micros() as u64
    }
    pub fn time(&self) -> Duration {
        self.time
    }
}

impl From<Duration> for TimePing {
    fn from(time: Duration) -> TimePing {
        TimePing { time }
    }
}
