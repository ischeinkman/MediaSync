use crate::messages::{PlayerPosition, PlayerState};
use crate::traits::{SyncPlayer, SyncPlayerList};
use crate::DynResult;
use futures::future::{FutureExt, LocalBoxFuture};
use std::time::Duration;

pub struct MprisPlayer<'a> {
    player: mpris::Player<'a>,
}

impl<'a> MprisPlayer<'a> {
    pub fn new(player: mpris::Player<'a>) -> Self {
        Self { player }
    }
}

impl<'a> SyncPlayer for MprisPlayer<'a> {
    fn get_state(&self) -> LocalBoxFuture<'_, DynResult<PlayerState>> {
        let fut = async move {
            let raw_state = self.player.get_playback_status().map_err(wrap_dbus_error)?;
            let state = match raw_state {
                mpris::PlaybackStatus::Playing => PlayerState::Playing,
                _ => PlayerState::Paused,
            };
            Ok(state)
        };
        fut.boxed_local()
    }
    fn set_state(&mut self, message: PlayerState) -> LocalBoxFuture<'_, DynResult<()>> {
        let fut = async move {
            match message {
                PlayerState::Paused => {
                    self.player.pause().map_err(wrap_dbus_error)?;
                }
                PlayerState::Playing => {
                    self.player.play().map_err(wrap_dbus_error)?;
                }
            }
            Ok(())
        };
        fut.boxed_local()
    }
    fn get_pos(&self) -> LocalBoxFuture<'_, DynResult<PlayerPosition>> {
        let retvl = self
            .player
            .get_position()
            .map(|dur| (dur.as_millis() & u64::max_value() as u128) as u64)
            .map(PlayerPosition::from_millis)
            .map_err(wrap_dbus_error);
        futures::future::ready(retvl).boxed_local()
    }
    fn set_pos(&mut self, state: PlayerPosition) -> LocalBoxFuture<'_, DynResult<()>> {
        let fut = async move {
            let track_id = self
                .player
                .get_metadata()
                .map_err(wrap_dbus_error)?
                .track_id()
                .ok_or_else(|| "Error: nothing is currently playing!".to_owned())?;
            self.player
                .set_position(track_id, &Duration::from_millis(state.as_millis()))
                .map_err(wrap_dbus_error)?;
            Ok(())
        };
        fut.boxed_local()
    }
}

pub struct MprisPlayerList {
    list: mpris::PlayerFinder,
}

impl SyncPlayerList for MprisPlayerList {
    fn new() -> DynResult<Self> {
        Ok(Self {
            list: mpris::PlayerFinder::new().map_err(wrap_dbus_error)?,
        })
    }
    fn get_players(&mut self) -> DynResult<Vec<(String, Box<dyn SyncPlayer>)>> {
        let res = self.list.find_all().map_err(wrap_finding_error)?;
        let mut retvl = Vec::with_capacity(res.len());
        for raw in res {
            let name = format!("{} ({})", raw.identity(), raw.unique_name());
            let player = MprisPlayer::new(raw);
            let boxed_player = Box::new(player);

            let polymorphed = Box::<dyn SyncPlayer>::from(boxed_player);
            retvl.push((name, polymorphed));
        }
        Ok(retvl)
    }
}

fn wrap_dbus_error(err: mpris::DBusError) -> crate::MyError {
    format!("mpris::DebusError: {:?}", err).into()
}

fn wrap_finding_error(err: mpris::FindingError) -> crate::MyError {
    format!("mpris::FindingError: {:?}", err).into()
}
