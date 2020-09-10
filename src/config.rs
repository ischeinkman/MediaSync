use crate::messages::{TimeDelta, UserId};

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