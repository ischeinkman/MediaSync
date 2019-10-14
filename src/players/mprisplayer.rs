use crate::players::events::{PlayerEvent, TimePing};
use crate::traits::{MediaPlayer, MediaPlayerList};
use crate::{DebugError, MyResult};
use mpris;
use std::collections::HashMap;
use std::time::Duration;

const POSITION_ERROR_MARGIN: Duration = Duration::from_millis(300);
const JUMP_ERROR_MARGIN: Duration = Duration::from_millis(1000);

pub struct MprisPlayerHandle<'a> {
    name: String,
    player: mpris::Player<'a>,
    previous_status: Option<PlayerStatus>,
    unprocessed_events: Vec<PlayerEvent>,
}

pub struct PlayerStatus {
    is_paused: bool,
    track_id: Option<mpris::TrackID>,
    track_url: Option<String>,
    position: Duration,
}

use std::cmp::PartialOrd;
use std::ops::Sub;
trait AbsDiff<Rhs = Self>: Sub<Rhs> + PartialOrd<Rhs> + Sized
where
    Rhs: Sub<Self, Output = Self::Output>,
{
    fn abs_diff(self, other: Rhs) -> Self::Output {
        if self > other {
            self - other
        } else {
            other - self
        }
    }
}

impl<T: Sub<Rhs> + PartialOrd<Rhs> + Sized, Rhs: Sub<T, Output = T::Output>> AbsDiff<Rhs> for T {}

impl PlayerStatus {
    pub fn events_since(&self, dt: Duration, previous: Option<&PlayerStatus>) -> Vec<PlayerEvent> {
        let mut retvl = Vec::new();
        let prev_url = previous.and_then(|p| p.track_url.as_ref());
        if self.track_url.as_ref() != prev_url {
            retvl.push(PlayerEvent::MediaOpen(
                self.track_url.as_ref().cloned().unwrap_or_else(String::new),
            ));
        }
        let needs_jump = previous.map_or(true, |prev| {
            if prev.position > self.position {
                true
            } else if self.is_paused {
                self.position.abs_diff(prev.position) > JUMP_ERROR_MARGIN
            } else {
                self.position.abs_diff(prev.position + dt) > JUMP_ERROR_MARGIN
            }
        });
        if needs_jump {
            retvl.push(PlayerEvent::Jump(self.position));
        }
        if previous.map_or(true, |p| p.is_paused != self.is_paused) {
            if self.is_paused {
                retvl.push(PlayerEvent::Pause);
            } else {
                retvl.push(PlayerEvent::Play);
            }
        }
        retvl
    }
}

impl<'a> MprisPlayerHandle<'a> {
    pub fn from_player(mut player: mpris::Player<'a>) -> MyResult<Self> {
        player.set_dbus_timeout_ms(100);
        let retvl = Self {
            name: player.identity().to_owned(),
            player,
            previous_status: None,
            unprocessed_events: Vec::new(),
        };
        Ok(retvl)
    }
    fn current_trackid(&self) -> MyResult<Option<mpris::TrackID>> {
        self.player
            .get_metadata()
            .map_err(|e| format!("mpris::MetadataError: {:?}", e).into())
            .map(|m| m.track_id())
    }
    fn current_status(&self) -> MyResult<PlayerStatus> {
        let is_paused = self
            .player
            .get_playback_status()
            .map_err(DebugError::into_myerror)?
            == mpris::PlaybackStatus::Paused;
        let track_id = self.current_trackid()?;
        let position = self
            .player
            .get_position()
            .map_err(DebugError::into_myerror)?;
        let track_url: Option<String> = self
            .player
            .get_metadata()
            .map_err(|e| format!("mpris::MetadataError: {:?}", e))?
            .url()
            .map(|s| s.to_owned());
        Ok(PlayerStatus {
            is_paused,
            track_id,
            position,
            track_url,
        })
    }
}

impl<'a> MediaPlayer for MprisPlayerHandle<'a> {
    fn name(&self) -> &str {
        &self.name
    }
    fn send_event(&mut self, msg: PlayerEvent) -> MyResult<()> {
        match msg {
            PlayerEvent::Jump(ref time) => {
                println!("Trying to jump to {}", time.as_millis());
                let current_track_id = match self.current_trackid()? {
                    Some(t) => t,
                    None => return Ok(()),
                };
                crate::debug_print(format!("Hello cutie"));
                crate::debug_print(format!("You're amazing"));
                self.player
                    .set_position(current_track_id, &time)
                    .map_err(DebugError::into_myerror)?;
                crate::debug_print(format!("And I love you"));
                self.player.play().map_err(DebugError::into_myerror)?;

                println!("Finished jump?");
            }
            PlayerEvent::Pause => {
                crate::debug_print(format!("Player got paused."));
                self.player.pause().map_err(DebugError::into_myerror)?;
            }
            PlayerEvent::Play => {
                crate::debug_print(format!("Player got played."));
                self.player.play().map_err(DebugError::into_myerror)?;
            }
            PlayerEvent::MediaOpen(ref url) => {
                crate::debug_print(format!("Player got open: {}.", url));
                self.player
                    .add_track_at_start(&url, true)
                    .map_err(DebugError::into_myerror)?;
            }
            _ => unimplemented!(),
        }
        self.unprocessed_events.push(msg);
        Ok(())
    }

    fn ping(&self) -> MyResult<TimePing> {
        let current_time = self
            .player
            .get_position()
            .map_err(DebugError::into_myerror)?;
        crate::debug_print(format!("Sending ping {}", current_time.as_millis()));
        Ok(TimePing::from(current_time))
    }

    fn on_ping(&mut self, ping: TimePing) -> MyResult<()> {
        let status = self.current_status()?;
        let current_time = status.position;
        let difference = if current_time > ping.time() {
            current_time - ping.time()
        } else {
            ping.time() - current_time
        };
        if difference > POSITION_ERROR_MARGIN {
            println!(
                "Ping correction: {}{}",
                if current_time > ping.time() { "-" } else { "+" },
                difference.as_millis()
            );
            let current_track = if let Some(t) = self.current_trackid()? {
                t
            } else {
                println!(
                    "WARN: got no track to set position to for ping with time {}!",
                    ping.as_micros()
                );
                return Ok(());
            };
            let adapted_time = ping.time() + (POSITION_ERROR_MARGIN * 50) / 100;
            self.player
                .set_position(current_track, &adapted_time)
                .map_err(DebugError::into_myerror)?;
            if let Some(prev) = self.previous_status.as_mut() {
                prev.position = adapted_time;
            }
        }
        crate::debug_print(format!("Got ping {}", ping.as_micros() / 1000));
        Ok(())
    }

    fn check_events(&mut self, time_since_previous: Duration) -> MyResult<Vec<PlayerEvent>> {
        if !self.player.is_running() {
            return Ok(vec![PlayerEvent::Shutdown]);
        }
        let current_status = self.current_status()?;
        let raw_evts =
            current_status.events_since(time_since_previous, self.previous_status.as_ref());
        let retvl = raw_evts
            .into_iter()
            .filter(|evt| match evt {
                PlayerEvent::Pause => !self
                    .unprocessed_events
                    .iter()
                    .any(|evt| evt == &PlayerEvent::Pause),
                PlayerEvent::Play => !self
                    .unprocessed_events
                    .iter()
                    .any(|evt| evt == &PlayerEvent::Play),
                PlayerEvent::Jump(_) => !self.unprocessed_events.iter().any(|evt| {
                    if let PlayerEvent::Jump(_) = *evt {
                        true
                    } else {
                        false
                    }
                }),
                PlayerEvent::MediaOpen(_) => !self.unprocessed_events.iter().any(|evt| {
                    if let PlayerEvent::MediaOpen(_) = *evt {
                        true
                    } else {
                        false
                    }
                }),
                _ => true,
            })
            .collect();
        self.previous_status = Some(current_status);
        self.unprocessed_events.clear();
        Ok(retvl)
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

unsafe impl<'a> Send for MprisPlayerHandle<'a> {}

pub struct MprisPlayerList {
    finder: Option<mpris::PlayerFinder>,
}

impl MediaPlayerList for MprisPlayerList {
    type Player = MprisPlayerHandle<'static>;
    fn new() -> Self {
        Self { finder: None }
    }
    fn list_name(&self) -> &str {
        "DBUS Media Players"
    }
    fn available(&mut self) -> MyResult<Vec<String>> {
        let idents = self
            .finder()?
            .find_all()
            .map_err(DebugError::into_myerror)?
            .into_iter()
            .map(|player| (player.identity().to_owned(), player));

        let mut by_name: HashMap<String, Vec<mpris::Player<'_>>> = HashMap::new();
        for (k, v) in idents {
            let ent = by_name.entry(k).or_insert_with(Vec::new);
            ent.push(v);
        }

        let retvl = by_name
            .into_iter()
            .flat_map(|(name, players)| {
                if players.len() == 1 {
                    vec![name].into_iter()
                } else {
                    players
                        .into_iter()
                        .map(|p| format!("{} (IDENT: {})", name, p.unique_name()))
                        .collect::<Vec<_>>()
                        .into_iter()
                }
            })
            .collect();
        Ok(retvl)
    }
    fn open(&mut self, name: &str) -> MyResult<MprisPlayerHandle<'static>> {
        let mut components_iter = name.rsplitn(2, "(IDENT:");
        let (true_name, unique_name) = {
            let first = components_iter.next().unwrap();
            let second = components_iter.next();
            match second {
                Some(true_name) => (true_name.trim(), Some(first.trim_end_matches(')').trim())),
                None => (first.trim(), None),
            }
        };
        let players = self
            .finder()?
            .find_all()
            .map_err(DebugError::into_myerror)?;
        players
            .into_iter()
            .find(|player| {
                player.identity() == true_name
                    && unique_name
                        .map(|u| u == player.unique_name())
                        .unwrap_or(true)
            })
            .ok_or_else(|| {
                format!(
                    "Error: could not find open MPRIS player with name {} ( => {} / {:?})",
                    name, true_name, unique_name
                )
                .into()
            })
            .and_then(MprisPlayerHandle::from_player)
    }
}

impl MprisPlayerList {
    fn finder(&mut self) -> MyResult<&mpris::PlayerFinder> {
        if self.finder.is_none() {
            let new_finder = mpris::PlayerFinder::new().map_err(DebugError::into_myerror)?;
            self.finder = Some(new_finder);
        }
        Ok(self.finder.as_ref().unwrap())
    }
}
