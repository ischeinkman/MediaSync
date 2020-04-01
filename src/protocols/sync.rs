use super::{MessageProto, TimeDelta, TimeStamp, UserId};
use crate::utils::{array_copy, AbsSub};
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct SyncMessage {
    // All numbers are stored in LITTLE ENDIAN.
    // Structure:
    //
    // Header:
    // -  0 - 16 : source id,
    // - 16 - 24 : created timestamp,
    // - 24 - 25 : message protocol,
    //
    // Body:
    // - 25 - 27: event flags,
    // ---- 25 & 1 : jumped,
    // ---- 25 & 2 : state changed,
    // ---- 25 & 4 : is playing,
    // ---- rest : reserved,
    // - 27 - 32 : player position,
    raw_data: [u8; 32],
}

impl Default for SyncMessage {
    fn default() -> Self {
        let mut data = [0; 32];
        data[24] = MessageProto::Sync.tag_byte();
        Self { raw_data: data }
    }
}

impl SyncMessage {
    pub fn zero() -> Self {
        Self::default()
    }

    pub fn from_raw(raw_data: [u8; 32]) -> Self {
        Self { raw_data }
    }

    pub fn into_raw(self) -> [u8; 32] {
        self.raw_data
    }
    pub fn source_id(&self) -> UserId {
        let source_bytes = array_copy(&self.raw_data[0..]);
        let uid_raw = u128::from_le_bytes(source_bytes);
        UserId { data: uid_raw }
    }

    pub fn set_source_id(&mut self, id: UserId) {
        let raw_bytes = id.data.to_le_bytes();
        (&mut self.raw_data[0..raw_bytes.len()]).copy_from_slice(&raw_bytes);
    }

    pub fn created(&self) -> TimeStamp {
        TimeStamp::from_bytes(&self.raw_data[16..])
    }

    pub fn set_created(&mut self, created: TimeStamp) {
        let raw_bytes = created.millis.to_le_bytes();
        (&mut self.raw_data[16..16 + raw_bytes.len()]).copy_from_slice(&raw_bytes);
    }

    pub fn position(&self) -> PlayerPosition {
        let mut pos_bytes = [0; 8];
        let valid_pos_bytes = &mut pos_bytes[0..5];
        valid_pos_bytes.copy_from_slice(&self.raw_data[27..]);
        let pos_millis = u64::from_le_bytes(pos_bytes);
        PlayerPosition { millis: pos_millis }
    }

    pub fn set_position(&mut self, pos: PlayerPosition) {
        let pos_eight_bytes = pos.millis.to_le_bytes();
        let pos_bytes = &pos_eight_bytes[0..5];
        (&mut self.raw_data[27..]).copy_from_slice(pos_bytes);
    }

    fn event_flags(&self) -> u16 {
        let bytes = [self.raw_data[25], self.raw_data[26]];
        u16::from_le_bytes(bytes)
    }

    pub fn jumped(&self) -> bool {
        self.event_flags() & 1 != 0
    }

    pub fn set_jumped(&mut self, jumped: bool) {
        let jumped_bit = if jumped { 1 } else { 0 };
        self.raw_data[25] = (self.raw_data[25] & !1) | jumped_bit;
    }

    pub fn state(&self) -> PlayerState {
        if self.event_flags() & 4 != 0 {
            PlayerState::Playing
        } else {
            PlayerState::Paused
        }
    }

    pub fn set_state(&mut self, state: PlayerState) {
        let state_bit = if state == PlayerState::Playing { 4 } else { 0 };
        self.raw_data[25] = (self.raw_data[25] & !4) | state_bit;
    }

    pub fn changed_state(&self) -> bool {
        self.event_flags() & 2 != 0
    }

    pub fn set_changed_state(&mut self, changed_state: bool) {
        let changed_state_bit = if changed_state { 2 } else { 0 };
        self.raw_data[25] = (self.raw_data[25] & !2) | changed_state_bit;
    }

    pub fn projected_to_time(&self, new_time: TimeStamp) -> Self {
        let mut copied = *self;
        copied.set_created(new_time);
        if self.state() == PlayerState::Playing {
            let old_pos = self.position();
            let time_diff = new_time.millis.saturating_sub(self.created().millis);
            let new_pos = PlayerPosition {
                millis: old_pos.millis.saturating_add(time_diff),
            };
            copied.set_position(new_pos);
        }
        copied
    }
}
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct PlayerPosition {
    millis: u64,
}

impl PlayerPosition {
    pub fn from_millis(millis: u64) -> Self {
        Self { millis }
    }

    pub fn as_millis(self) -> u64 {
        self.millis
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut pos_bytes = [0; 8];
        let pos_slice = &mut pos_bytes[0..5.min(bytes.len())];
        pos_slice.copy_from_slice(&bytes[..pos_slice.len()]);
        let millis = u64::from_le_bytes(pos_bytes);
        PlayerPosition::from_millis(millis)
    }
}

impl AbsSub for PlayerPosition {
    type Output = TimeDelta;
    fn abs_sub(self, other: Self) -> Self::Output {
        let millis = self.millis.abs_sub(other.millis);
        TimeDelta { millis }
    }
}

impl std::ops::Add<TimeDelta> for PlayerPosition {
    type Output = Self;
    fn add(self, other: TimeDelta) -> Self {
        PlayerPosition::from_millis(self.millis + other.as_millis())
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum PlayerState {
    Playing,
    Paused,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_message() {
        let mut msg = SyncMessage::zero();

        let rand_id_bytes: [u8; 16] = rand::random();
        let rand_id = UserId::from_bytes(&rand_id_bytes);
        msg.set_source_id(rand_id);

        let rand_created = TimeStamp::now();
        msg.set_created(rand_created);

        assert_eq!(msg.source_id(), rand_id);
        assert_eq!(msg.created(), rand_created);
    }

    #[test]
    fn test_player_pos() {
        let pa = PlayerPosition::from_millis(200);
        let pb = PlayerPosition::from_millis(250);
        let diff_a = pa.abs_sub(pb);
        let diff_b = pb.abs_sub(pa);
        let diff_c = TimeDelta::from_millis(50);
        assert_eq!(diff_a, diff_b);
        assert_eq!(diff_a, diff_c);
    }
}
