use crate::events::PlayerEvent;
use crate::events::TimePing;
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
}

pub struct PlayerStatus {
    is_paused: bool,
    track_id: Option<mpris::TrackID>,
    track_url: Option<String>,
    position: Duration,
    prev_events: Vec<PlayerEvent>,
}

impl PlayerStatus {
    pub fn abs_time_diff(&self, other: &PlayerStatus) -> Duration {
        if self.position > other.position {
            self.position - other.position
        } else {
            other.position - self.position
        }
    }
}

impl<'a> MprisPlayerHandle<'a> {
    pub fn from_player(mut player: mpris::Player<'a>) -> MyResult<Self> {
        player.set_dbus_timeout_ms(100);
        let retvl = Self {
            name: player.identity().to_owned(),
            player,
            previous_status: None,
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
            .map_err(|e| {
                Box::<dyn std::error::Error>::from(format!("mpris::MetadataError: {:?}", e))
            })?
            .url()
            .map(|s| s.to_owned());
        Ok(PlayerStatus {
            is_paused,
            track_id,
            position,
            track_url,
            prev_events: Vec::new(),
        })
    }
}

impl<'a> MediaPlayer for MprisPlayerHandle<'a> {
    fn name(&self) -> &str {
        &self.name
    }
    fn send_event(&mut self, msg: PlayerEvent) -> MyResult<()> {
        match msg {
            PlayerEvent::Jump(time) => {
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
                if let Some(prev) = self.previous_status.as_mut() {
                    prev.position = time;
                }

                println!("Finished jump?");
            }
            PlayerEvent::Pause => {
                crate::debug_print(format!("Player got paused."));
                self.player.pause().map_err(DebugError::into_myerror)?;
                if let Some(prev) = self.previous_status.as_mut() {
                    prev.is_paused = true;
                }
            }
            PlayerEvent::Play => {
                crate::debug_print(format!("Player got played."));
                self.player.play().map_err(DebugError::into_myerror)?;
                if let Some(prev) = self.previous_status.as_mut() {
                    prev.is_paused = false;
                }
            }
            PlayerEvent::MediaOpen(url) => {
                crate::debug_print(format!("Player got open: {}.", url));
                self.player
                    .add_track_at_start(&url, true)
                    .map_err(DebugError::into_myerror)?;
                let new_id = self.current_trackid()?;
                if let Some(prev) = self.previous_status.as_mut() {
                    prev.track_url = Some(url);
                    prev.track_id = new_id;
                }
            }
            _ => unimplemented!(),
        }
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
        crate::debug_print(format!("{}: Start check_events", self.name,));
        if !self.player.is_running() {
            crate::debug_print(format!("{}: Found shutdown in check_events", self.name,));
            return Ok(vec![PlayerEvent::Shutdown]);
        }
        let mut retvl = Vec::new();
        let mut current_status = self.current_status()?;
        crate::debug_print(format!("{}: Got status in check_events", self.name,));
        let prev_status_lock = &mut self.previous_status;
        crate::debug_print(format!(
            "{}: Got prev_status_lock in check_events",
            self.name,
        ));
        let has_prev_media_change = prev_status_lock
            .as_ref()
            .map(|p| {
                p.prev_events.iter().any(|evt| match evt {
                    PlayerEvent::MediaOpen(_) => true,
                    _ => false,
                })
            })
            .unwrap_or(false);
        crate::debug_print(format!(
            "{}: Got has_prev_media_change in check_events",
            self.name,
        ));
        if !has_prev_media_change {
            match prev_status_lock.as_ref() {
                Some(prev_status) if current_status.track_url == prev_status.track_url => {}
                _ => {
                    let url = current_status
                        .track_url
                        .as_ref()
                        .cloned()
                        .unwrap_or_else(|| "".to_owned());
                    retvl.push(PlayerEvent::MediaOpen(url.clone()));
                    let current_id = current_status.track_id.clone();
                    let prev_id = prev_status_lock
                        .as_ref()
                        .and_then(|t| t.track_id.as_ref().cloned());
                    crate::debug_print(format!("Player sending url {}", url));
                    crate::debug_print(format!("    ID change: {:?} => {:?}", prev_id, current_id));
                }
            }
        }
        crate::debug_print(format!(
            "{}: Built media change evt in check_events",
            self.name,
        ));
        let dt = prev_status_lock
            .as_ref()
            .map(|prev_status| current_status.abs_time_diff(&prev_status))
            .unwrap_or(current_status.position);
        crate::debug_print(format!("{}: Got dt in check_events", self.name,));
        let dt_error = if dt > time_since_previous {
            dt - time_since_previous
        } else {
            time_since_previous - dt
        };
        crate::debug_print(format!("{}: Got dt_error in check_events", self.name,));
        if dt_error > JUMP_ERROR_MARGIN {
            retvl.push(PlayerEvent::Jump(current_status.position));
            crate::debug_print(format!(
                "{}: Player sending Jump {}",
                self.name,
                current_status.position.as_millis()
            ));
        }
        crate::debug_print(format!("{}: Built jump evt in check_events", self.name,));
        let play_status_change = prev_status_lock
            .as_ref()
            .map(|prev_status| prev_status.is_paused != current_status.is_paused)
            .unwrap_or(true);
        crate::debug_print(format!(
            "{}: Got play_status_change in check_events",
            self.name,
        ));
        if play_status_change {
            if current_status.is_paused {
                retvl.push(PlayerEvent::Pause);
                crate::debug_print(format!("Player sending Pause"));
            } else {
                retvl.push(PlayerEvent::Play);
                crate::debug_print(format!("Player sending Play"));
            }
        }
        crate::debug_print(format!(
            "{}: Built play status evt in check_events",
            self.name,
        ));
        current_status.prev_events = retvl.clone();
        *prev_status_lock = Some(current_status);
        crate::debug_print(format!("{}: End check_events", self.name,));
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
