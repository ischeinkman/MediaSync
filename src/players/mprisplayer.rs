use crate::players::events::{PlayerEvent, TimePing};
use crate::traits::{MediaPlayer, MediaPlayerList};
use crate::{DebugError, MyResult};
use mpris;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const POSITION_ERROR_MARGIN: Duration = Duration::from_millis(500);
const JUMP_ERROR_MARGIN: Duration = Duration::from_millis(1000);
const TIME_OFFSET: Duration =
    Duration::from_millis(((POSITION_ERROR_MARGIN.as_millis() * 50) / 100) as u64);

pub struct MprisPlayerHandle<'a> {
    name: String,
    player: mpris::Player<'a>,
    previous_status: Option<PlayerStatus>,
    unprocessed_events: Vec<PlayerEvent>,
}

pub struct PlayerStatus {
    is_paused: bool,
    track_id: Option<mpris::TrackID>,
    track_meta: Option<mpris::Metadata>,
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
        let prev_id = previous.and_then(|p| p.track_id.as_ref());
        if self.track_id.as_ref() != prev_id {
            let cur_url = self.track_meta.as_ref().and_then(|m| m.url());
            let cur_status = self
                .track_meta
                .as_ref()
                .and_then(|m| match m.get("status") {
                    Some(mpris::MetadataValue::String(s)) => Some(s.as_ref()),
                    _ => None,
                });
            let prev_url = previous
                .as_ref()
                .and_then(|p| p.track_meta.as_ref())
                .and_then(|m| m.url());
            let prev_status = previous
                .as_ref()
                .and_then(|p| p.track_meta.as_ref())
                .and_then(|m| match m.get("status") {
                    Some(mpris::MetadataValue::String(s)) => Some(s.as_ref()),
                    _ => None,
                });
            if cur_url != prev_url
                && cur_url != prev_status
                && (cur_status != prev_url || prev_url.is_none())
            {
                let new_url = cur_url.or(cur_status).unwrap_or("");
                retvl.push(PlayerEvent::MediaOpen(new_url.to_owned()));
            }
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
        let playstatus = self
            .player
            .get_playback_status()
            .map_err(DebugError::into_myerror)?;
        let is_paused = playstatus != mpris::PlaybackStatus::Playing;
        let position = self
            .player
            .get_position()
            .map_err(DebugError::into_myerror)?;
        let meta = self
            .player
            .get_metadata()
            .map_err(|e| format!("mpris::MetadataError: {:?}", e))?;
        let track_id = meta.track_id();
        Ok(PlayerStatus {
            is_paused,
            track_id,
            position,
            track_meta: Some(meta),
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
                let current_track_id = match self.current_trackid()? {
                    Some(t) => t,
                    None => return Ok(()),
                };
                let adapted_time = *time + TIME_OFFSET;
                self.player
                    .set_position(current_track_id, &adapted_time)
                    .map_err(DebugError::into_myerror)?;
                self.player.play().map_err(DebugError::into_myerror)?;
            }
            PlayerEvent::Pause => {
                self.player.pause().map_err(DebugError::into_myerror)?;
            }
            PlayerEvent::Play => {
                self.player.play().map_err(DebugError::into_myerror)?;
            }
            PlayerEvent::MediaOpen(ref url) => {
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
        Ok(TimePing::from(current_time))
    }

    fn on_ping(&mut self, ping: TimePing, reference_timestamp: Duration) -> MyResult<()> {
        let status = self.current_status()?;
        let expected_time = if status.is_paused {
            ping.time()
        } else {
            let time_since_ping = SystemTime::now()
                .duration_since(UNIX_EPOCH + reference_timestamp)
                .unwrap();
            ping.time() + time_since_ping
        };
        let current_time = status.position;
        let difference = if current_time > expected_time {
            current_time - expected_time
        } else {
            expected_time - current_time
        };
        if difference > POSITION_ERROR_MARGIN {
            let current_track = if let Some(t) = self.current_trackid()? {
                t
            } else {
                return Ok(());
            };
            let adapted_time = ping.time() + TIME_OFFSET;
            println!(
                "Ping correction: {}{} over margin ({} , {}) => {}",
                if current_time > expected_time {
                    "+"
                } else {
                    "-"
                },
                (difference - POSITION_ERROR_MARGIN).as_millis(),
                current_time.as_millis(),
                ping.time().as_millis(),
                adapted_time.as_millis()
            );
            self.player
                .set_position(current_track, &adapted_time)
                .map_err(DebugError::into_myerror)?;
            if let Some(prev) = self.previous_status.as_mut() {
                prev.position = adapted_time;
            }
        }
        Ok(())
    }

    #[inline(always)]
    fn check_events(&mut self, time_since_previous: Duration) -> MyResult<Vec<PlayerEvent>> {
        let current_status = self.current_status()?;
        let raw_evts =
            current_status.events_since(time_since_previous, self.previous_status.as_ref());
        let mut skip_pause = false;
        let mut skip_play = false;
        let mut skip_jump = false;
        let mut skip_open = false;
        for evt in self.unprocessed_events.drain(..) {
            if skip_jump && skip_open && skip_pause && skip_play {
                break;
            }
            match evt {
                PlayerEvent::Pause => skip_pause = true,
                PlayerEvent::Play => skip_play = true,
                PlayerEvent::Jump(_) => skip_jump = true,
                PlayerEvent::MediaOpen(_) => skip_open = true,
                PlayerEvent::Shutdown => {}
            }
        }
        let retvl: Vec<PlayerEvent> = raw_evts
            .into_iter()
            .filter(|evt| match evt {
                PlayerEvent::Pause => !skip_pause,
                PlayerEvent::Play => !skip_play,
                PlayerEvent::Jump(_) => !skip_jump,
                PlayerEvent::MediaOpen(_) => !skip_open,
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
