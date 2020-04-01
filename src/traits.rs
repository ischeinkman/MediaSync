pub mod sync {
    use crate::protocols::sync::{PlayerPosition, PlayerState, SyncMessage};
    use crate::protocols::{TimeDelta, TimeStamp, UserId};
    use crate::utils::AbsSub;
    use crate::DynResult;
    #[derive(Clone, Eq, PartialEq, Debug)]
    pub struct SyncConfig {
        pub id: UserId,
        pub pos_err_threshold: TimeDelta,
        pub jump_threshold: TimeDelta,
        pub jump_if_backwards: bool,
    }

    impl SyncConfig {
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl Default for SyncConfig {
        fn default() -> Self {
            let id_bytes: [u8; 16] = rand::random();
            let id = UserId::from_bytes(&id_bytes);
            let pos_err_threshold = TimeDelta::from_millis(300);
            let jump_threshold = TimeDelta::from_millis(3000);
            let jump_if_backwards = false;
            Self {
                id,
                pos_err_threshold,
                jump_threshold,
                jump_if_backwards,
            }
        }
    }
    pub struct SyncPlayerWrapper<T: SyncPlayer> {
        player: T,
        config: SyncConfig,
        previous_status: SyncMessage,
        previous_event_times: std::collections::HashMap<UserId, TimeStamp>,
    }

    impl<T: SyncPlayer> SyncPlayerWrapper<T> {
        pub fn new(player: T, config: SyncConfig) -> DynResult<Self> {
            let cur_state = player.get_state()?;
            let cur_pos = player.get_pos()?;
            let cur_time = TimeStamp::now();
            let mut previous_status = SyncMessage::zero();
            previous_status.set_source_id(config.id);
            previous_status.set_created(cur_time);
            previous_status.set_state(cur_state);
            previous_status.set_position(cur_pos);
            previous_status.set_jumped(false);
            previous_status.set_changed_state(false);
            let previous_event_times = std::collections::HashMap::new();
            Ok(Self {
                player,
                config,
                previous_status,
                previous_event_times,
            })
        }
    }

    impl<T: SyncPlayer> SyncOps for SyncPlayerWrapper<T> {
        fn push_sync_status(&mut self, message: SyncMessage) -> DynResult<ShouldRebroadcast> {
            if message.source_id() == self.config.id {
                return Ok(false);
            }
            let has_newer = self
                .previous_event_times
                .get(&message.source_id())
                .map(|&ts| ts >= message.created())
                .unwrap_or(false);
            if has_newer {
                return Ok(false);
            }

            let mut next_state = self.previous_status;

            let current_state = self.player.get_state()?;
            if current_state != message.state() {
                if message.changed_state() {
                    self.player.set_state(message.state())?;
                    next_state.set_state(message.state());
                } else {
                    eprintln!("  WARNING: Got a state desync with another player. We are {:?}, but they are {:?}.", current_state, message.state());
                    eprintln!(
                        "  Defaulting to {:?} to best rectify the situation.",
                        PlayerState::Paused
                    );
                    if current_state != PlayerState::Paused {
                        self.player.set_state(PlayerState::Paused)?;
                        next_state.set_state(PlayerState::Paused);
                    }
                }
            }

            let current_time = TimeStamp::now();
            let current_pos = self.player.get_pos()?;

            let expected_pos = message.projected_to_time(current_time).position();
            if current_pos.abs_sub(expected_pos) > self.config.pos_err_threshold {
                if message.jumped() {
                    let npos = expected_pos
                        + TimeDelta::from_millis(self.config.pos_err_threshold.as_millis() / 4);
                    self.player.set_pos(npos)?;
                    next_state.set_position(npos);
                    next_state.set_created(TimeStamp::now());
                } else {
                    eprintln!("  WARNING: Got a position desync with another player. We are at {}, but they are at {}.", current_pos.as_millis(), expected_pos.as_millis());
                    eprintln!("  Rectifying to earlier.");
                    if expected_pos < current_pos {
                        let npos = expected_pos
                            + TimeDelta::from_millis(self.config.pos_err_threshold.as_millis() / 4);
                        self.player.set_pos(npos)?;
                        next_state.set_position(npos);
                        next_state.set_created(TimeStamp::now());
                    }
                }
            }
            self.previous_event_times
                .insert(message.source_id(), message.created());
            self.previous_status = next_state;
            Ok(true)
        }
        fn get_sync_status(&mut self) -> DynResult<SyncMessage> {
            let prev_status = self.previous_status;
            let mut retvl = prev_status;
            let now = TimeStamp::now();
            retvl.set_created(now);
            retvl.set_source_id(self.config.id);

            let cur_play_state = self.player.get_state()?;
            let cur_pos = self.player.get_pos()?;
            retvl.set_position(cur_pos);
            retvl.set_state(cur_play_state);

            let play_status = cur_play_state;
            let changed_status = play_status != prev_status.state();
            retvl.set_changed_state(changed_status);
            let expected_pos = prev_status.projected_to_time(now).position();
            let err = cur_pos.abs_sub(expected_pos);
            let jumped = (err > self.config.jump_threshold)
                || (self.config.jump_if_backwards && cur_pos < expected_pos);
            retvl.set_jumped(jumped);
            self.previous_status = retvl;
            Ok(retvl)
        }
    }
    pub type ShouldRebroadcast = bool;
    pub trait SyncOps {
        fn get_sync_status(&mut self) -> DynResult<SyncMessage>;
        fn push_sync_status(&mut self, message: SyncMessage) -> DynResult<ShouldRebroadcast>;
    }

    pub trait SyncPlayer {
        fn get_state(&self) -> DynResult<PlayerState>;
        fn set_state(&mut self, state: PlayerState) -> DynResult<()>;
        fn get_pos(&self) -> DynResult<PlayerPosition>;
        fn set_pos(&mut self, state: PlayerPosition) -> DynResult<()>;
    }

    impl<'a> SyncPlayer for Box<dyn SyncPlayer + 'a> {
        fn get_state(&self) -> DynResult<PlayerState> {
            self.as_ref().get_state()
        }
        fn set_state(&mut self, state: PlayerState) -> DynResult<()> {
            self.as_mut().set_state(state)
        }
        fn get_pos(&self) -> DynResult<PlayerPosition> {
            self.as_ref().get_pos()
        }
        fn set_pos(&mut self, state: PlayerPosition) -> DynResult<()> {
            self.as_mut().set_pos(state)
        }
    }

    pub trait SyncPlayerList: Sized {
        fn new() -> DynResult<Self>;
        fn get_players(&mut self) -> DynResult<Vec<(String, Box<dyn SyncPlayer>)>>;
    }
}
