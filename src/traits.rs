use crate::players::events::{PlayerEvent, TimePing};
use crate::MyResult;
use std::time::Duration;

pub trait MediaPlayer: Send {
    fn name(&self) -> &str;
    fn send_event(&mut self, event: PlayerEvent) -> MyResult<()>;
    fn check_events(&mut self, time_since_previous: Duration) -> MyResult<Vec<PlayerEvent>>;
    fn ping(&self) -> MyResult<TimePing>;
    fn on_ping(&mut self, ping: TimePing, reference: Duration) -> MyResult<()>;
}

pub trait MediaPlayerList {
    type Player: MediaPlayer;
    fn new() -> Self;
    fn list_name(&self) -> &str;
    fn available(&mut self) -> MyResult<Vec<String>>;
    fn open(&mut self, name: &str) -> MyResult<Self::Player>;
}
