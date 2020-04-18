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
        last_state_push_time: TimeStamp,
        last_jump_time: TimeStamp,
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
                last_state_push_time: cur_time,
            })
        }

        fn should_process(&self, message: SyncMessage) -> bool {
            if message.source_id() == self.config.id {
                return false;
            }
            self.previous_event_times
                .get(&message.source_id())
                .map(|&ts| ts < message.created())
                .unwrap_or(true)
        }

        fn process_state_change(&self, message: SyncMessage) -> DynResult<Option<PlayerState>> {
            // Only push if we actually need to
            if self.previous_status.state() == message.state() {
                return Ok(None);
            }
            // If we got a valid state push, do it.
            let should_push_state =
                message.changed_state() && message.created() > self.last_state_push_time;
            if should_push_state {
                log::info!(
                    "Got a state change! Now changing state to {:?}.",
                    message.state()
                );
                return Ok(Some(message.state()));
            }
            // We are currently desynched with the remote.

            // If the local player got pushed between messages, just wait
            // for the local event to get broadcast and ignore the desynch.
            let current_state = self.player.get_state().unwrap();
            if current_state != self.previous_status.state() {
                log::warn!("Player changed state mid push.");
                return Ok(None);
            }
            // Otherwise, default to Pause.
            let nstate = PlayerState::Paused;
            log::warn!(
                "Got a state desync with another player: {:?} , {:?}. Defaulting to {:?}.",
                current_state,
                message.state(),
                nstate
            );
            Ok(Some(nstate))
        }

        fn process_pos_change(
            &self,
            message: SyncMessage,
        ) -> DynResult<Option<(PlayerPosition, bool)>> {
            // 1: Get the current *expected* position of the player
            let current_time = TimeStamp::now();
            let current_pos = self
                .previous_status
                .projected_to_time(current_time)
                .position();

            // 2: Get the current (expected) position of the remote, and check if we need to jump at all
            let expected_pos = message.projected_to_time(current_time).position();
            let has_pos_diff = current_pos.abs_sub(expected_pos) > self.config.pos_err_threshold;
            let past_last_jump = message.created() > self.last_jump_time;
            if !(has_pos_diff && past_last_jump) {
                return Ok(None);
            }
            // 3: Did the remote jump? If so just push the remote's time.
            if message.jumped() {
                log::info!(
                    "Got a remote jump! Now jumping to {}.",
                    expected_pos.as_millis()
                );
                return Ok(Some((expected_pos, true)));
            }

            // 4: Get the local player's actual position, in case of deviation from the expected.
            let player_pos = self.player.get_pos().unwrap();
            let different_mag = player_pos.abs_sub(current_pos);
            let jumped_back = self.config.jump_if_backwards && player_pos < current_pos;

            // 5: If the local player jumped while processing the message, do nothing and let the jump get pushed on next query.
            if different_mag > self.config.jump_threshold || jumped_back {
                log::warn!("Player jumped mid push.");
                return Ok(None);
            }

            // 6: Otherwise, correct the desync.
            log::warn!(
                "Got a position desync with another player. We are at {} ({}), but they are at {}.",
                current_pos.as_millis(),
                player_pos.as_millis(),
                expected_pos.as_millis()
            );
            log::warn!(
                "Position error: {} ({}) VS {}",
                current_pos.abs_sub(expected_pos).as_millis(),
                player_pos.abs_sub(expected_pos).as_millis(),
                self.config.pos_err_threshold.as_millis()
            );
            log::warn!("Now attempting to rectify.");

            // 7: Verify that we actually need to correct a desynch, or if the reported time just drifted.
            if player_pos.abs_sub(expected_pos) <= self.config.pos_err_threshold {
                log::warn!("Not rectifying since it seems our actual position is okay?");
                return Ok(Some((player_pos, false)));
            }

            // 8: If there really is a desynch and the desynch is backwards, we still add a small offset o account for processing time.
            let offset_millis = if expected_pos < current_pos {
                self.config.pos_err_threshold.as_millis() / 3
            } else {
                0
            };
            let npos = expected_pos + TimeDelta::from_millis(offset_millis);
            Ok(Some((npos, true)))
        }
        pub fn push_sync_status(&mut self, message: SyncMessage) -> DynResult<ShouldRebroadcast> {
            // Verify that we need to process the message at all
            if !self.should_process(message) {
                return Ok(false);
            }

            let mut next_status = self.previous_status;

            // Update playpause state
            if let Some(nxt) = self.process_state_change(message)? {
                next_status.set_created(TimeStamp::now());
                next_status.set_state(nxt);
                self.player.set_state(nxt).unwrap();
                self.last_state_push_time = message.created();
            }

            // Update player position
            if let Some((npos, should_push)) = self.process_pos_change(message)? {
                next_status.set_created(TimeStamp::now());
                next_status.set_position(npos);
                if should_push {
                    self.player.set_pos(npos).unwrap();
                    self.last_jump_time = message.created();
                }
            }

            // Update the previous time map
            self.previous_event_times
                .insert(message.source_id(), message.created());
            self.previous_status = next_status;
            Ok(true)
        }
        fn inner_status(&self) -> DynResult<SyncMessage> {
            let prev_status = self.previous_status;
            let mut retvl = SyncMessage::zero();
            let now = TimeStamp::now();
            retvl.set_created(now);
            retvl.set_source_id(self.config.id);

            let cur_play_state = self.player.get_state()?;
            let changed_status = cur_play_state != prev_status.state();
            retvl.set_state(cur_play_state);
            retvl.set_changed_state(changed_status);

            let cur_pos = self.player.get_pos()?;
            let expected_pos = prev_status.projected_to_time(now).position();
            let err = cur_pos.abs_sub(expected_pos);
            let jump_back = self.config.jump_if_backwards && cur_pos < expected_pos;
            let jumped = (err > self.config.jump_threshold) || jump_back;
            retvl.set_jumped(jumped);
            retvl.set_position(cur_pos);
            Ok(retvl)
        }
        pub fn get_sync_status(&mut self) -> DynResult<SyncMessage> {
            let next_status = self.inner_status()?;
            self.previous_status = next_status;
            if next_status.changed_state() || next_status.jumped() {
                log::info!(
                    "Local player raised event flags: ({:?}, {:?}) with values ({:?}, {:?})",
                    next_status.changed_state(),
                    next_status.jumped(),
                    next_status.state(),
                    next_status.position()
                );
                if next_status.changed_state() {
                    self.last_state_push_time = next_status.created();
                }
                if next_status.jumped() {
                    self.last_jump_time = next_status.created();
                }
            }
            Ok(next_status)
        }
    }
    pub type ShouldRebroadcast = bool;

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
