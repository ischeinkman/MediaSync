use super::DynResult;
use std::time::SystemTime;
pub mod sync;
use std::ops::Add;
pub use sync::{PlayerPosition, PlayerState, SyncMessage};

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct TimeStamp {
    millis: u64,
}

impl TimeStamp {
    pub fn now() -> TimeStamp {
        loop {
            let cur = SystemTime::now();
            let orig = SystemTime::UNIX_EPOCH;
            let diff = match cur.duration_since(orig) {
                Ok(d) => d,
                Err(_) => {
                    continue;
                }
            };
            debug_assert!(diff.as_millis() <= u64::max_value() as u128);
            break TimeStamp {
                millis: (diff.as_millis() & u64::max_value() as u128) as u64,
            };
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> TimeStamp {
        let mut data_bytes = [0; 8];
        let data_bytes_len = data_bytes.len();
        let data_slice = &mut data_bytes[..data_bytes_len.min(bytes.len())];
        let source_slice = &bytes[..bytes.len().min(data_bytes_len)];
        data_slice.copy_from_slice(source_slice);
        let millis = u64::from_le_bytes(data_bytes);
        TimeStamp { millis }
    }

    pub fn as_millis(self) -> u64 {
        self.millis
    }
}

impl Add<TimeDelta> for TimeStamp {
    type Output = TimeStamp;
    fn add(self, dt: TimeDelta) -> TimeStamp {
        TimeStamp {
            millis: self.millis + dt.millis,
        }
    }
}
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct TimeDelta {
    millis: u64,
}

impl TimeDelta {
    pub fn from_millis(millis: u64) -> Self {
        TimeDelta { millis }
    }
    pub fn as_millis(self) -> u64 {
        self.millis
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct UserId {
    data: u128,
}

impl UserId {
    pub fn from_bytes(bytes: &[u8]) -> UserId {
        let mut data_bytes = [0; 16];
        let data_bytes_len = data_bytes.len();
        let data_slice = &mut data_bytes[..data_bytes_len.min(bytes.len())];
        let source_slice = &bytes[..bytes.len().min(data_bytes_len)];
        data_slice.copy_from_slice(source_slice);
        let data = u128::from_le_bytes(data_bytes);
        UserId { data }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum MessageProto {
    Sync,
    Media,
}
impl MessageProto {
    pub fn tag_byte(self) -> u8 {
        match self {
            MessageProto::Sync => 1,
            MessageProto::Media => 2,
        }
    }

    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(MessageProto::Sync),
            2 => Some(MessageProto::Media),
            _ => None,
        }
    }
}

fn get_proto(buffer: &[u8; 32]) -> Result<MessageProto, u8> {
    let bt = buffer[24];
    MessageProto::from_tag(bt).ok_or(bt)
}
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum Message {
    Sync(SyncMessage),
}

impl Message {
    pub fn parse_block(block: [u8; 32]) -> DynResult<Message> {
        match get_proto(&block) {
            Ok(MessageProto::Sync) => Ok(Message::Sync(SyncMessage::from_raw(block))),
            Ok(MessageProto::Media) => Err("Error: Media protocol is not yet implemented"
                .to_owned()
                .into()),
            Err(tag) => Err(format!("Error: found invalid protocol tag {}", tag).into()),
        }
    }

    pub fn into_block(self) -> [u8; 32] {
        match self {
            Message::Sync(msg) => msg.into_raw(),
        }
    }
}

impl From<SyncMessage> for Message {
    fn from(body: SyncMessage) -> Self {
        Message::Sync(body)
    }
}
