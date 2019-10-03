

mod communication;
mod mprisplayer;

use std::time::Duration;

type MyResult<T> = Result<T, Box<dyn std::error::Error>>;

fn main() {
    let mut player = mprisplayer::MprisPlayer::find_open().unwrap().unwrap();
    let mut comms = communication::Communicator::open(40000, 41000).unwrap();

    let interval_goal = Duration::from_millis(20);
    let mut prev_iteration = std::time::Instant::now();
    loop {
        comms.check_incoming().unwrap();
        let local_events = player.check_events(std::time::Instant::now() - prev_iteration, true).unwrap();
        for local_evt in local_events.into_iter() {
            comms.send_message(local_evt).unwrap();
        }

        let comms_events = comms.check_message().unwrap();
        for remote_evt in comms_events.into_iter() {
            player.on_message(remote_evt).unwrap();
        }

        prev_iteration = std::time::Instant::now();
        std::thread::sleep(interval_goal);
    }

}

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum ProtocolMessage {
    TimePing(Duration),
    Pause(Duration),
    Play(Duration),
    Jump(Duration),
    MediaChange(String),
    Shutdown, 
}

pub trait DebugError : Sized {
    fn message(self) -> String;
    fn into_myerror(self) -> Box<dyn std::error::Error> {
        self.message().into()
    }
}

pub trait MediaPlayer {
    fn on_message(&mut self, message : ProtocolMessage) -> MyResult<()> ;
}