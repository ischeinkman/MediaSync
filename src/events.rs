use crate::players::events::{PlayerEvent, TimePing};
use crate::MyResult;
use std::time::Duration;

#[derive(Clone, Eq, PartialEq, Debug, Hash)]
pub enum RemoteEvent {
    Ping {
        timestamp: Duration,
        payload: TimePing,
    },
    Pause,
    Play,
    Jump(Duration),
    MediaOpen(String),
    RequestTransfer(String),
    RespondTransfer {
        size: u64,
        transfer_code: Option<[char; 9]>,
    },
    MediaOpenOkay(String),
    Shutdown,
}

#[derive(Eq, PartialEq, Hash, Clone, Copy, Debug, Ord, PartialOrd)]
pub enum RemoteEventKind {
    Ping = 0,
    Pause = 1,
    Play = 2,
    Jump = 3,
    MediaOpen = 4,
    RequestTransfer = 5,
    RespondTransfer = 6,
    MediaOpenOkay = 7,
    Shutdown = 0xff,
}

impl RemoteEventKind {
    pub fn parse_kind_byte(byte: u8) -> Option<RemoteEventKind> {
        match byte & 0x7f {
            0 => RemoteEventKind::Ping.into(),
            1 => RemoteEventKind::Pause.into(),
            2 => RemoteEventKind::Play.into(),
            3 => RemoteEventKind::Jump.into(),
            4 => RemoteEventKind::MediaOpen.into(),
            5 => RemoteEventKind::RequestTransfer.into(),
            6 => RemoteEventKind::RespondTransfer.into(),
            7 => RemoteEventKind::MediaOpenOkay.into(),
            0x7f => RemoteEventKind::Shutdown.into(),
            _ => None,
        }
    }

    pub fn is_multiblock(self) -> bool {
        match self {
            RemoteEventKind::MediaOpen => true,
            RemoteEventKind::RequestTransfer => true,
            RemoteEventKind::MediaOpenOkay => true,
            _ => false,
        }
    }
}
impl From<RemoteEventKind> for u8 {
    fn from(kind: RemoteEventKind) -> u8 {
        kind as u8
    }
}

impl RemoteEvent {
    pub fn kind(&self) -> RemoteEventKind {
        match self {
            RemoteEvent::Ping { .. } => RemoteEventKind::Ping,
            RemoteEvent::Pause => RemoteEventKind::Pause,
            RemoteEvent::Play => RemoteEventKind::Play,
            RemoteEvent::Jump(_) => RemoteEventKind::Jump,
            RemoteEvent::MediaOpen(_) => RemoteEventKind::MediaOpen,
            RemoteEvent::RequestTransfer(_) => RemoteEventKind::RequestTransfer,
            RemoteEvent::RespondTransfer { .. } => RemoteEventKind::RespondTransfer,
            RemoteEvent::MediaOpenOkay(_) => RemoteEventKind::MediaOpenOkay,
            RemoteEvent::Shutdown => RemoteEventKind::Shutdown,
        }
    }
    pub fn parse_block(next: RawBlock) -> Option<Self> {
        let kind = next.kind()?;
        if kind.is_multiblock() {
            return None;
        }
        match kind {
            RemoteEventKind::Ping => {
                let ping_micros = next.count_u64();
                let ping = TimePing::from_micros(ping_micros);
                let mut ts_bytes = [0; 8];
                (&mut ts_bytes).copy_from_slice(&next.payload()[..8]);
                let timestamp = Duration::from_micros(u64::from_le_bytes(ts_bytes));
                (Some(RemoteEvent::Ping {
                    payload: ping,
                    timestamp,
                }))
            }
            RemoteEventKind::Pause => (Some(RemoteEvent::Pause)),
            RemoteEventKind::Play => (Some(RemoteEvent::Play)),
            RemoteEventKind::Jump => {
                (Some(RemoteEvent::Jump(Duration::from_micros(next.count_u64()))))
            }
            RemoteEventKind::RespondTransfer => {
                let size = next.count_u64();
                let code_slice = &next.payload()[0..9];
                let mut code_buffer = ['\0'; 9];
                let valid =
                    code_buffer
                        .iter_mut()
                        .zip(code_slice.iter())
                        .all(|(buffer, &payload)| {
                            let payload_char = char::from(payload);
                            if payload_char.is_alphanumeric() {
                                *buffer = payload_char;
                                true
                            } else {
                                false
                            }
                        });
                if valid {
                    Some(RemoteEvent::RespondTransfer {
                        size,
                        transfer_code: Some(code_buffer),
                    })
                } else {
                    Some(RemoteEvent::RespondTransfer {
                        size,
                        transfer_code: None,
                    })
                }
            }
            RemoteEventKind::Shutdown => (Some(RemoteEvent::Shutdown)),
            _ => unreachable!(),
        }
    }
    pub fn as_blocks(&self) -> Vec<RawBlock> {
        match self {
            RemoteEvent::Ping { timestamp, payload } => {
                let png_micros = payload.as_micros();
                let timestamp_micros = timestamp.as_micros() as u64;
                let ts_bytes = timestamp_micros.to_le_bytes();
                let retvl = RawBlock::new()
                    .with_kind(self.kind())
                    .with_count_u64(png_micros)
                    .with_payload(&ts_bytes);

                vec![retvl]
            }
            RemoteEvent::Pause => vec![RawBlock::new().with_kind(self.kind())],
            RemoteEvent::Play => vec![RawBlock::new().with_kind(self.kind())],
            RemoteEvent::Jump(tm) => {
                let timestamp = tm.as_micros() as u64;
                let retvl = RawBlock::new()
                    .with_kind(self.kind())
                    .with_count_u64(timestamp);
                vec![retvl]
            }
            RemoteEvent::MediaOpen(url) => {
                let string_bytes = url.as_bytes();
                let url_len = string_bytes.len();
                let bytes_iter = string_bytes.chunks(22).enumerate();
                let blocks = bytes_iter.map(|(idx, bytes)| {
                    let length_or_idx = if idx == 0 { url_len as u64 } else { idx as u64 };
                    RawBlock::new()
                        .with_kind(self.kind())
                        .with_append_flag(idx != 0)
                        .with_count_u64(length_or_idx)
                        .with_payload(bytes)
                });
                blocks.collect()
            }
            RemoteEvent::RequestTransfer(url) => {
                let string_bytes = url.as_bytes();
                let url_len = string_bytes.len();
                let bytes_iter = string_bytes.chunks(22).enumerate();
                let blocks = bytes_iter.map(|(idx, bytes)| {
                    let length_or_idx = if idx == 0 { url_len as u64 } else { idx as u64 };
                    RawBlock::new()
                        .with_kind(self.kind())
                        .with_append_flag(idx != 0)
                        .with_count_u64(length_or_idx)
                        .with_payload(bytes)
                });
                blocks.collect()
            }
            RemoteEvent::RespondTransfer {
                size,
                transfer_code: Some(code),
            } => {
                let mut retvl = RawBlock::new().with_kind(self.kind()).with_count_u64(*size);
                for (idx, c) in code.iter().enumerate() {
                    retvl.data[10 + idx] = *c as u8;
                }
                vec![retvl]
            }
            RemoteEvent::MediaOpenOkay(url) => {
                let string_bytes = url.as_bytes();
                let url_len = string_bytes.len();
                let bytes_iter = string_bytes.chunks(22).enumerate();
                let blocks = bytes_iter.map(|(idx, bytes)| {
                    let length_or_idx = if idx == 0 { url_len as u64 } else { idx as u64 };
                    RawBlock::new()
                        .with_kind(self.kind())
                        .with_append_flag(idx != 0)
                        .with_count_u64(length_or_idx)
                        .with_payload(bytes)
                });
                blocks.collect()
            }
            RemoteEvent::Shutdown => vec![RawBlock::from_data([0xff; 32])],
            _ => unimplemented!(),
        }
    }
}

impl From<PlayerEvent> for RemoteEvent {
    fn from(evt: PlayerEvent) -> RemoteEvent {
        match evt {
            PlayerEvent::Jump(tm) => RemoteEvent::Jump(tm),
            PlayerEvent::Pause => RemoteEvent::Pause,
            PlayerEvent::Play => RemoteEvent::Play,
            PlayerEvent::Shutdown => RemoteEvent::Shutdown,
            PlayerEvent::MediaOpen(url) => RemoteEvent::MediaOpen(url),
        }
    }
}
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct RawBlock {
    data: [u8; RawBlock::BLOCK_SIZE],
}
impl RawBlock {
    const APPEND_FLAG: u8 = 1 << 7;
    const PAYLOAD_START: usize = 10;
    pub const PAYLOAD_SIZE: usize = Self::BLOCK_SIZE - Self::PAYLOAD_START;
    pub const BLOCK_SIZE: usize = 32;
    pub fn new() -> Self {
        RawBlock {
            data: [0; RawBlock::BLOCK_SIZE],
        }
    }
    pub fn with_kind(mut self, kind: RemoteEventKind) -> Self {
        self.data[0] = u8::from(kind);
        self
    }
    pub fn with_append_flag(mut self, is_append: bool) -> Self {
        let cur_kind = self.data[0];
        let next_kind = if is_append {
            cur_kind | Self::APPEND_FLAG
        } else {
            cur_kind & !(Self::APPEND_FLAG)
        };
        self.data[0] = next_kind;
        self
    }
    pub fn with_count_u64(mut self, count: u64) -> Self {
        let count_bytes = count.to_le_bytes();
        let relevant_portion = &mut self.data[2..Self::PAYLOAD_START];
        relevant_portion.copy_from_slice(&count_bytes);
        self
    }

    pub fn with_payload(mut self, payload: &[u8]) -> Self {
        let mylen = Self::PAYLOAD_SIZE;
        let self_payload =
            &mut self.data[Self::PAYLOAD_START..Self::PAYLOAD_START + mylen.min(payload.len())];
        self_payload.copy_from_slice(payload);
        self
    }
    pub fn from_data(data: [u8; RawBlock::BLOCK_SIZE]) -> Self {
        Self { data }
    }

    pub fn kind(&self) -> Option<RemoteEventKind> {
        RemoteEventKind::parse_kind_byte(self.kind_byte() & !RawBlock::APPEND_FLAG)
    }

    pub fn kind_byte(&self) -> u8 {
        self.data[0]
    }

    pub fn is_append(&self) -> bool {
        self.kind_byte() & Self::APPEND_FLAG != 0
    }

    pub fn count_u64(&self) -> u64 {
        let data_portion = [
            self.data[2],
            self.data[3],
            self.data[4],
            self.data[5],
            self.data[6],
            self.data[7],
            self.data[8],
            self.data[9],
        ];
        u64::from_le_bytes(data_portion)
    }

    pub fn payload(&self) -> &[u8] {
        &self.data[10..]
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn into_payload(self) -> [u8; Self::PAYLOAD_SIZE] {
        let mut retvl = [0; Self::PAYLOAD_SIZE];
        (&mut retvl).copy_from_slice(self.payload());
        retvl
    }
}

pub struct ArrayIter<Item: Copy, Array: AsRef<[Item]>> {
    arr: Array,
    idx: usize,
    _phantom: std::marker::PhantomData<Item>,
}
impl<Item: Copy, Array: AsRef<[Item]>> ArrayIter<Item, Array> {
    pub fn new(arr: Array) -> Self {
        ArrayIter {
            arr,
            idx: 0,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<Item: Copy, Array: AsRef<[Item]>> Iterator for ArrayIter<Item, Array> {
    type Item = Item;
    fn next(&mut self) -> Option<Item> {
        let arr = self.arr.as_ref();
        let retvl = arr.get(self.idx).copied();
        if retvl.is_some() {
            self.idx += 1;
        }
        retvl
    }
}

#[derive(Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct BlockParser {
    buffer: Vec<RawBlock>,
}

impl BlockParser {
    pub fn new() -> BlockParser {
        BlockParser { buffer: Vec::new() }
    }

    fn parse_buffer(&mut self) -> MyResult<Option<RemoteEvent>> {
        let header = match self.buffer.first().copied() {
            Some(h) => h,
            None => unreachable!(),
        };
        let buffer_kind = header.kind().unwrap_or(RemoteEventKind::Shutdown);
        let string_length = header.count_u64();
        let current_bytes = (self.buffer.len() * RawBlock::PAYLOAD_SIZE) as u64;
        if current_bytes < string_length {
            return Ok(None);
        }
        let bytes: Vec<_> = self
            .buffer
            .drain(..)
            .flat_map(|block| ArrayIter::new(block.into_payload()))
            .take(string_length as usize)
            .collect();
        let msg = String::from_utf8(bytes)?;
        match buffer_kind {
            RemoteEventKind::MediaOpen => Ok(Some(RemoteEvent::MediaOpen(msg))),
            RemoteEventKind::RequestTransfer => Ok(Some(RemoteEvent::RequestTransfer(msg))),
            RemoteEventKind::MediaOpenOkay => Ok(Some(RemoteEvent::MediaOpenOkay(msg))),
            _ => Err(format!(
                "Error: got invalid kind {:x} for string message {}",
                header.kind_byte(),
                msg
            )
            .into()),
        }
    }

    pub fn parse_next(&mut self, next: RawBlock) -> MyResult<Option<RemoteEvent>> {
        if let Some(header) = self.buffer.first().copied() {
            let buffer_kind = header.kind().unwrap_or(RemoteEventKind::Shutdown);
            if next.kind() != Some(buffer_kind) || !next.is_append() {
                return Err(format!(
                    "Error parsing block: got kind {:x} in the middle of chain for block type {:x}",
                    next.kind_byte(),
                    u8::from(buffer_kind)
                )
                .into());
            }
            let packet_idx = next.count_u64();
            if packet_idx != self.buffer.len() as u64 {
                return Err(format!(
                    "Error parsing block: got packet idx {} even though only have {} in the buffer",
                    packet_idx,
                    self.buffer.len()
                )
                .into());
            }
            self.buffer.push(next);
            self.parse_buffer()
        } else {
            match next.kind() {
                Some(kind) if kind.is_multiblock() => {
                    self.buffer.push(next);
                    self.parse_buffer()
                }
                Some(kind) if !kind.is_multiblock() => {
                    RemoteEvent::parse_block(next).map(Some).ok_or_else(|| {
                        format!("Error: bad block parse: {:x?}", next.as_bytes()).into()
                    })
                }
                _ => Err(format!(
                    "Error: got invalid initial event kind {:x}",
                    next.kind_byte()
                )
                .into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    #[test]
    fn test_ping_parsing() {
        let mut parser = BlockParser::new();
        let ping_time = 1000 * 1000 * 60 * 132;
        let now_duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros();

        let ping = RemoteEvent::Ping {
            payload: TimePing::from_micros(ping_time),
            timestamp: Duration::from_micros(now_duration as u64),
        };
        let mut ping_raw = ping.as_blocks();
        assert_eq!(1, ping_raw.len());
        assert_eq!(Some(RemoteEventKind::Ping), ping_raw[0].kind());
        assert_eq!(ping_time, ping_raw[0].count_u64());

        let parsed = parser.parse_next(ping_raw.pop().unwrap()).unwrap().unwrap();
        assert_eq!(ping, parsed);
    }

    #[test]
    fn test_mediaopen_parsing() {
        let msg = "file:///home/test/some/long/path_to_a_file_to_play.mkv";
        let expected_bytes = msg.as_bytes().len();
        let expected_packets = 1 + expected_bytes / RawBlock::PAYLOAD_SIZE;

        let event = RemoteEvent::MediaOpen(msg.to_owned());
        let packets = event.as_blocks();
        assert_eq!(expected_packets, packets.len());

        let mut parser = BlockParser::new();
        let parsed = packets
            .into_iter()
            .filter_map(|block| parser.parse_next(block).unwrap())
            .next()
            .unwrap();
        assert_eq!(event, parsed);
    }
}
