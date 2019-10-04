use crate::{DebugError, MyResult};
use mpris;
use std::sync::{Arc, Mutex};
use std::time::{Duration};

use crate::{MediaPlayer, ProtocolMessage};

pub struct MprisPlayer<'a> {
    name: String,
    player: Arc<Mutex<mpris::Player<'a>>>,
    previous_status: Arc<Mutex<Option<PlayerStatus>>>,
}

pub struct PlayerStatus {
    is_paused: bool,
    track: Option<mpris::TrackID>,
    track_url: Option<String>,
    position: Duration,
}

impl<'a> MprisPlayer<'a> {
    pub fn find_open() -> MyResult<Option<Self>> {
        let finder = mpris::PlayerFinder::new().map_err(DebugError::into_myerror)?;
        let found = if let Some(f) = finder.find_all().map_err(DebugError::into_myerror)?.pop() {
            f
        } else {
            return Ok(None);
        };
        let retvl = MprisPlayer {
            name: found.identity().to_owned(),
            player: Arc::new(Mutex::new(found)),
            previous_status: Arc::new(Mutex::new(None)),
        };
        Ok(Some(retvl))
    }
    fn current_trackid(&self) -> MyResult<Option<mpris::TrackID>> {
        let player = self.player.lock().map_err(DebugError::into_myerror)?;
        player
            .get_metadata()
            .map_err(|e| format!("mpris::MetadataError: {:?}", e).into())
            .map(|m| m.track_id())
    }
    fn current_status(&self) -> MyResult<PlayerStatus> {
        let player = self.player.lock().map_err(DebugError::into_myerror)?;
        let is_paused = player
            .get_playback_status()
            .map_err(DebugError::into_myerror)?
            == mpris::PlaybackStatus::Paused;
        let track = self.current_trackid()?;
        let position = player.get_position().map_err(DebugError::into_myerror)?;
        let track_url: Option<String> = player
            .get_metadata()
            .map_err(|e| {
                Box::<dyn std::error::Error>::from(format!("mpris::MetadataError: {:?}", e))
            })?
            .url()
            .map(|s| s.to_owned());
        Ok(PlayerStatus {
            is_paused,
            track,
            position,
            track_url,
        })
    }

    pub fn check_events(
        &mut self,
        interval: Duration,
        should_ping: bool,
    ) -> MyResult<Vec<ProtocolMessage>> {
        let mut retvl = Vec::new();
        let current_status = self.current_status()?;
        if should_ping {
            retvl.push(ProtocolMessage::TimePing(current_status.position));
        }

        let mut status_lock = self.previous_status.lock().map_err(DebugError::into_myerror)?;
        if let Some(prev) = status_lock.as_ref() {
            let time_delta = current_status.position.checked_sub(prev.position);
            let should_jump = time_delta
                .filter(|&t| t <= (interval * 101) / 100)
                .is_none();
            if should_jump {
                retvl.push(ProtocolMessage::Jump(current_status.position));
            }

            if current_status.is_paused && !prev.is_paused {
                retvl.push(ProtocolMessage::Pause(current_status.position));
            } else if !current_status.is_paused && prev.is_paused {
                retvl.push(ProtocolMessage::Play(current_status.position));
            }

            if current_status.track != prev.track {
                retvl.push(ProtocolMessage::MediaChange(
                    current_status
                        .track_url
                        .clone()
                        .unwrap_or_else(|| "".to_owned()),
                ));
            }
        }

        *status_lock = Some(current_status);
        Ok(retvl)
    }

    pub fn handle(&self) -> MprisPlayer<'a> {
        MprisPlayer {
            name : self.name.clone(), 
            player : Arc::clone(&self.player),
            previous_status : Arc::clone(&self.previous_status),
        }
    }
}
impl<'a> MediaPlayer for MprisPlayer<'a> {
    fn name(&self) -> &str {
        &self.name
    }
    fn on_message(&mut self, msg: ProtocolMessage) -> MyResult<()> {
        let player = self.player.lock().map_err(DebugError::into_myerror)?;
        match msg {
            ProtocolMessage::Jump(time) => {
                player
                    .set_position(
                        self.current_trackid()?
                            .ok_or_else(|| "Tried jumping without TrackID!".to_owned())?,
                        &time,
                    )
                    .map_err(DebugError::into_myerror)?;
            }
            ProtocolMessage::Pause(time) => {
                player
                    .set_position(
                        self.current_trackid()?
                            .ok_or_else(|| "Tried Pausing without TrackID!".to_owned())?,
                        &time,
                    )
                    .map_err(DebugError::into_myerror)?;
                player.pause().map_err(DebugError::into_myerror)?;
            }
            ProtocolMessage::Play(time) => {
                if mpris::PlaybackStatus::Playing
                    == player
                        .get_playback_status()
                        .map_err(DebugError::into_myerror)?
                {
                    return Err("Error: tried playing player when it already started!"
                        .to_owned()
                        .into());
                }
                player
                    .set_position(
                        self.current_trackid()?
                            .ok_or_else(|| "Tried Unpausing without TrackID!".to_owned())?,
                        &time,
                    )
                    .map_err(DebugError::into_myerror)?;
                player.play().map_err(DebugError::into_myerror)?;
            }
            ProtocolMessage::TimePing(time) => {
                const ERROR_MARGIN: Duration = Duration::from_millis(20);
                let current_time = player.get_position().map_err(DebugError::into_myerror)?;
                let difference = current_time
                    .checked_sub(time)
                    .or_else(|| time.checked_sub(current_time))
                    .unwrap_or(Duration::from_secs(0));
                if difference > ERROR_MARGIN {
                    player
                        .set_position(
                            self.current_trackid()?
                                .ok_or_else(|| "Tried ponging without TrackID!".to_owned())?,
                            &time,
                        )
                        .map_err(DebugError::into_myerror)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl DebugError for mpris::DBusError {
    fn message(self) -> String {
        format!("mpris::DBusError : {:?}", self)
    }
}

impl DebugError for mpris::FindingError {
    fn message(self) -> String {
        format!("mpris::FindingError : {:?}", self)
    }
}

unsafe impl<'a> Send for MprisPlayer<'a> {}
