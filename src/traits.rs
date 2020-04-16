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
            let pos_err_threshold = TimeDelta::from_millis(400);
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
        last_state_push_time : TimeStamp, 
        last_jump_time : TimeStamp,
    }

    impl<T: SyncPlayer> SyncPlayerWrapper<T> {
        pub fn new(player: T, config: SyncConfig) -> DynResult<Self> {
            let cur_state = player.get_state().unwrap();
            let cur_pos = player.get_pos().unwrap();
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
                last_jump_time: cur_time,
                last_state_push_time : cur_time,
            })
        }
    }

    impl<T: SyncPlayer> SyncOps for SyncPlayerWrapper<T> {
        fn push_sync_status(&mut self, message: SyncMessage) -> DynResult<ShouldRebroadcast> {
            if message.source_id() == self.config.id {
                //Don't repeat messages forever 
                return Ok(false);
            }
            let has_newer = self
                .previous_event_times
                .get(&message.source_id())
                .map(|&ts| ts >= message.created())
                .unwrap_or(false);
            if has_newer {
                //We have newer.
                return Ok(false);
            }
            let current_time = TimeStamp::now();
            let dt = current_time.as_millis() - message.created().as_millis();
            if dt >= (3 * crate::PUSH_FREQUENCY_MILLIS)/2 {
                //log::debug!("Not syncing to remote due to age of {}. Flags: ({:?}, {:?})", dt,  message.changed_state(), message.jumped());
                //It's been too long; wait for the next one.
                //return Ok(false);
            }

            let mut next_state = self.previous_status;

            let current_state = self.player.get_state().unwrap();
            if self.previous_status.state() != message.state() {
                log::info!("Rectifying self with remote data {} millis old. ", dt);
                if message.changed_state() && message.created() > self.last_state_push_time {
                    log::info!(
                        "Got a state change! Now changing state to {:?}.",
                        message.state()
                    );
                    self.player.set_state(message.state())?;
                    next_state.set_state(message.state());
                    next_state.set_created(TimeStamp::now());
                    self.last_state_push_time = message.created();
                } else if current_state != self.previous_status.state() {
                    log::warn!("  WARNING: Player changed state mid push.");
                } else {
                    log::warn!("  WARNING: Got a state desync with another player. We are {:?}, but they are {:?}.", current_state, message.state());
                    log::warn!(
                        "  Defaulting to {:?} to best rectify the situation.",
                        PlayerState::Paused
                    );
                    if current_state != PlayerState::Paused {
                        self.player.set_state(PlayerState::Paused)?;
                        next_state.set_state(PlayerState::Paused);
                        next_state.set_created(TimeStamp::now());
                    }
                    self.last_state_push_time = message.created();
                }
            }

            let current_time = TimeStamp::now();
            let current_pos = self
                .previous_status
                .projected_to_time(current_time)
                .position();

            let expected_pos = message.projected_to_time(current_time).position();
            if current_pos.abs_sub(expected_pos) > self.config.pos_err_threshold && message.created() > self.last_jump_time {
                if message.jumped() {
                    log::info!(
                        "Got a remote jump! Now jumping to {}.",
                        expected_pos.as_millis()
                    );
                    let npos = expected_pos;
                    self.player.set_pos(npos)?;
                    next_state.set_position(npos);
                    next_state.set_created(TimeStamp::now());
                    self.last_jump_time = message.created();
                } else {
                    let player_pos = self.player.get_pos()?;
                    let different_mag = player_pos.abs_sub(current_pos);
                    if different_mag > self.config.jump_threshold
                        || (player_pos < current_pos && self.config.jump_if_backwards)
                    {
                        log::warn!("  WARNING: Player jumped mid push.");
                    } else if player_pos.abs_sub(expected_pos) <= self.config.pos_err_threshold {
                        log::warn!("  WARNING: Got a position desync with another player. We are at {} ({}), but they are at {}.", current_pos.as_millis(), player_pos.as_millis(), expected_pos.as_millis());
                        log::warn!("  Not rectifying since it seems our actual position is okay?");
                        next_state.set_position(player_pos);
                        next_state.set_created(TimeStamp::now());
                    } else {
                        log::warn!("  WARNING: Got a position desync with another player. We are at {} ({}), but they are at {}.", current_pos.as_millis(), player_pos.as_millis(), expected_pos.as_millis());
                        log::warn!(
                            "ERR: {} ({}) VS {}",
                            current_pos.abs_sub(expected_pos).as_millis(),
                            player_pos.abs_sub(expected_pos).as_millis(),
                            self.config.pos_err_threshold.as_millis()
                        );
                        log::warn!("  Now attempting to rectify.");
                        if expected_pos < current_pos {
                            let npos = expected_pos
                                + TimeDelta::from_millis(
                                    self.config.pos_err_threshold.as_millis() / 3,
                                );
                            self.player.set_pos(npos)?;
                            next_state.set_position(npos);
                            next_state.set_created(TimeStamp::now());
                        }
                        else {
                            //Did we miss a jump somewhere?
                            let npos = expected_pos;
                            self.player.set_pos(npos)?;
                            next_state.set_position(npos);
                            next_state.set_created(TimeStamp::now());

                        }
                        self.last_jump_time = message.created();
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
            let mut retvl = SyncMessage::zero();
            let now = TimeStamp::now();
            retvl.set_created(now);
            retvl.set_source_id(self.config.id);

            let cur_play_state = self.player.get_state()?;
            let cur_pos = self.player.get_pos()?;
            retvl.set_position(cur_pos);
            retvl.set_state(cur_play_state);

            let changed_status = cur_play_state != prev_status.state();
            retvl.set_changed_state(changed_status);
            let expected_pos = prev_status.projected_to_time(now).position();
            let err = cur_pos.abs_sub(expected_pos);
            let jumped = (err > self.config.jump_threshold)
                || (self.config.jump_if_backwards && cur_pos < expected_pos);
            retvl.set_jumped(jumped);
            if retvl.changed_state() || retvl.jumped() {
                log::info!(
                    "Local player raised event flags: ({:?}, {:?}) with values ({:?}, {:?})",
                    retvl.changed_state(),
                    retvl.jumped(),
                    retvl.state(),
                    retvl.position()
                );
            }
            self.previous_status = retvl;
            if changed_status {
                self.last_state_push_time = now;
            }
            if jumped {
                self.last_jump_time = now;
            }
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
