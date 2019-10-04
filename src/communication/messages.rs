use crate::MyResult;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Default)]
pub struct RawBlock {
    pub data: [u8; 32],
}

impl RawBlock {
    pub fn from_data(data: [u8; 32]) -> Self {
        Self { data }
    }
    pub fn parse(self) -> MyResult<MessageBlock> {
        let kind = self.data[0];
        if kind == 0 {
            let command_byte = self.data[1];
            let command = TimedCommandKind::from_kind_byte(command_byte).ok_or_else(|| {
                format!("Error: got invalid TimedCommand kind {:x}", command_byte)
            })?;
            let nano_bytes = [
                self.data[2],
                self.data[3],
                self.data[4],
                self.data[5],
                self.data[6],
                self.data[7],
                self.data[8],
                self.data[9],
            ];
            let nanoseconds = u64::from_le_bytes(nano_bytes);
            Ok(MessageBlock::TimedCommand {
                kind: command,
                nanoseconds,
            })
        } else if kind == 1 {
            let length_bytes = [self.data[1], self.data[2], self.data[3], self.data[4]];
            let path_length = u32::from_le_bytes(length_bytes);
            let mut payload = [0u16; 13];
            for (idx, cur_byte) in payload.iter_mut().enumerate() {
                let cur_bytes = [self.data[2 * idx + 6], self.data[2 * idx + 7]];
                *cur_byte = u16::from_le_bytes(cur_bytes);
            }
            Ok(MessageBlock::TrackChangeHeader {
                path_length,
                payload,
            })
        } else if kind == 2 {
            let idx_bytes = [self.data[1], self.data[2], self.data[3], self.data[4]];
            let packet_idx = u32::from_le_bytes(idx_bytes);
            let mut payload = [0u16; 13];
            for (idx, cur_byte) in payload.iter_mut().enumerate() {
                let cur_bytes = [self.data[2 * idx + 6], self.data[2 * idx + 7]];
                *cur_byte = u16::from_le_bytes(cur_bytes);
            }
            Ok(MessageBlock::TrackChangePacket {
                packet_idx,
                payload,
            })
        } else {
            Err(format!("Error: got invalid message kind {:x}.", kind).into())
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum TimedCommandKind {
    Ping,
    Jump,
    Pause,
    Play,
}

impl TimedCommandKind {
    pub fn from_kind_byte(byte: u8) -> Option<TimedCommandKind> {
        match byte {
            0 => Some(TimedCommandKind::Ping),
            1 => Some(TimedCommandKind::Jump),
            2 => Some(TimedCommandKind::Pause),
            3 => Some(TimedCommandKind::Play),
            _ => None,
        }
    }
    pub fn to_kind_byte(self) -> u8 {
        match self {
            TimedCommandKind::Ping => 0,
            TimedCommandKind::Jump => 1,
            TimedCommandKind::Pause => 2,
            TimedCommandKind::Play => 3,
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub enum MessageBlock {
    TimedCommand {
        kind: TimedCommandKind,
        nanoseconds: u64,
    },
    TrackChangeHeader {
        path_length: u32, // In UTF16 characters
        payload: [u16; 13],
    },
    TrackChangePacket {
        packet_idx: u32, // Always > 0
        payload: [u16; 13],
    },
}

impl MessageBlock {
    pub fn into_raw(self) -> RawBlock {
        match self {
            MessageBlock::TimedCommand { kind, nanoseconds } => {
                let mut data = [0; 32];
                data[0] = 0;
                data[1] = kind.to_kind_byte();
                let time_bytes = &nanoseconds.to_le_bytes();
                (&mut data[2..10]).copy_from_slice(time_bytes);
                RawBlock { data }
            }

            _ => {
                //TODO: this
                RawBlock::default()
            }
        }
    }
}
