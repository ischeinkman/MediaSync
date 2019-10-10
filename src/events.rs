use crate::MyResult;
use std::time::Duration;

#[derive(Clone, Eq, PartialEq, Debug, Hash)]
pub enum PlayerEvent {
    Pause,
    Play,
    Jump(Duration),
    MediaOpen(String),
    Shutdown,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct TimePing {
    time: Duration,
}

impl TimePing {
    pub fn from_micros(micros: u64) -> TimePing {
        Duration::from_micros(micros).into()
    }
    pub fn from_duration(time: Duration) -> TimePing {
        TimePing { time }
    }
    pub fn as_micros(&self) -> u64 {
        self.time.as_micros() as u64
    }
    pub fn time(&self) -> Duration {
        self.time
    }
}

impl From<Duration> for TimePing {
    fn from(time: Duration) -> TimePing {
        TimePing { time }
    }
}

#[derive(Clone, Eq, PartialEq, Debug, Hash)]
pub enum RemoteEvent {
    Ping(TimePing),
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

impl RemoteEvent {
    pub fn as_blocks(&self) -> Vec<RawBlock> {
        match self {
            RemoteEvent::Ping(TimePing { time: tm }) => {
                let timestamp = tm.as_micros() as u64;
                let retvl = RawBlock::new().with_kind(0).with_count_u64(timestamp);
                vec![retvl]
            }
            RemoteEvent::Pause => vec![RawBlock::new().with_kind(1)],
            RemoteEvent::Play => vec![RawBlock::new().with_kind(2)],
            RemoteEvent::Jump(tm) => {
                let timestamp = tm.as_micros() as u64;
                let retvl = RawBlock::new().with_kind(3).with_count_u64(timestamp);
                vec![retvl]
            }
            RemoteEvent::MediaOpen(url) => {
                let string_bytes = url.as_bytes();
                let url_len = string_bytes.len();
                let bytes_iter = string_bytes.chunks(22).enumerate();
                let blocks = bytes_iter.map(|(idx, bytes)| {
                    let kind = if idx == 0 { 4 } else { 4 | 1 << 7 };
                    let length_or_idx = if idx == 0 { url_len as u64 } else { idx as u64 };
                    RawBlock::new()
                        .with_kind(kind)
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
                    let kind = if idx == 0 { 5 } else { 5 | 1 << 7 };
                    let length_or_idx = if idx == 0 { url_len as u64 } else { idx as u64 };
                    RawBlock::new()
                        .with_kind(kind)
                        .with_count_u64(length_or_idx)
                        .with_payload(bytes)
                });
                blocks.collect()
            }
            RemoteEvent::RespondTransfer {
                size,
                transfer_code: Some(code),
            } => {
                let mut retvl = RawBlock::new().with_kind(6).with_count_u64(*size);
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
                    let kind = if idx == 0 { 7 } else { 7 | 1 << 7 };
                    let length_or_idx = if idx == 0 { url_len as u64 } else { idx as u64 };
                    RawBlock::new()
                        .with_kind(kind)
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

impl From<TimePing> for RemoteEvent {
    fn from(ping: TimePing) -> RemoteEvent {
        RemoteEvent::Ping(ping)
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct RawBlock {
    data: [u8; 32],
}
impl RawBlock {
    pub const PAYLOAD_SIZE: usize = 32 - 10;
    pub fn new() -> Self {
        RawBlock { data: [0; 32] }
    }
    pub fn with_kind(mut self, kind_byte: u8) -> Self {
        self.data[0] = kind_byte;
        self
    }
    pub fn with_count_u64(mut self, count: u64) -> Self {
        let count_bytes = count.to_le_bytes();
        let relevant_portion = &mut self.data[2..10];
        relevant_portion.copy_from_slice(&count_bytes);
        self
    }

    pub fn with_payload(mut self, payload: &[u8]) -> Self {
        let mylen = Self::PAYLOAD_SIZE;
        let self_payload = &mut self.data[10..10 + mylen.min(payload.len())];
        self_payload.copy_from_slice(payload);
        self
    }
    pub fn from_data(data: [u8; 32]) -> Self {
        Self { data }
    }

    pub fn kind(&self) -> u8 {
        self.data[0]
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
}

#[derive(Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct BlockParser {
    buffer: Vec<RawBlock>,
}

impl BlockParser {
    pub fn new() -> BlockParser {
        BlockParser { buffer: Vec::new() }
    }

    pub fn parse_next(&mut self, next: RawBlock) -> MyResult<Option<RemoteEvent>> {
        if !self.buffer.is_empty() {
            let buffer_kind = self.buffer.first().map(|b| b.kind()).unwrap_or(0);
            let next_kind = next.kind();
            if next_kind != buffer_kind | 1 << 7 {
                return Err(format!(
                    "Error parsing block: got kind {:x} in the middle of chain for block type {:x}",
                    next_kind, buffer_kind
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

            let string_length = self.buffer.first().map(|b| b.count_u64()).unwrap_or(0);
            let total_expected = 1 + string_length / (RawBlock::PAYLOAD_SIZE as u64);
            if self.buffer.len() as u64 == total_expected {
                let mut moved_buffer = Vec::new();
                std::mem::swap(&mut self.buffer, &mut moved_buffer);
                let (kind, length) = moved_buffer
                    .first()
                    .map(|b| (b.kind(), b.count_u64()))
                    .unwrap_or((0, 0));
                let mut bytes = Vec::new();
                for block in moved_buffer.into_iter() {
                    bytes.extend_from_slice(block.payload());
                }
                bytes.resize(length as usize, 0);
                let msg = String::from_utf8(bytes)?;
                match kind {
                    4 => Ok(Some(RemoteEvent::MediaOpen(msg))),
                    5 => Ok(Some(RemoteEvent::RequestTransfer(msg))),
                    7 => Ok(Some(RemoteEvent::MediaOpenOkay(msg))),
                    e => Err(
                        format!("Error: got invalid kind {:x} for string message {}", e, msg)
                            .into(),
                    ),
                }
            } else {
                Ok(None)
            }
        } else {
            match next.kind() {
                0 => Ok(Some(RemoteEvent::Ping(TimePing::from_micros(
                    next.count_u64(),
                )))),
                1 => Ok(Some(RemoteEvent::Pause)),
                2 => Ok(Some(RemoteEvent::Play)),
                3 => Ok(Some(RemoteEvent::Jump(Duration::from_micros(
                    next.count_u64(),
                )))),
                4 => {
                    self.buffer.push(next);
                    Ok(None)
                }
                5 => {
                    self.buffer.push(next);
                    Ok(None)
                }
                6 => {
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
                        Ok(Some(RemoteEvent::RespondTransfer {
                            size,
                            transfer_code: Some(code_buffer),
                        }))
                    } else {
                        Ok(Some(RemoteEvent::RespondTransfer {
                            size,
                            transfer_code: None,
                        }))
                    }
                }
                7 => {
                    self.buffer.push(next);
                    Ok(None)
                }
                0xff => Ok(Some(RemoteEvent::Shutdown)),
                other => Err(format!("Error: got invalid initial event kind {:x}", other).into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_ping_parsing() {
        let mut parser = BlockParser::new();
        let ping_time = 1000 * 1000 * 60 * 132;
        let ping = RemoteEvent::Ping(TimePing::from_micros(ping_time));
        let mut ping_raw = ping.as_blocks();
        assert_eq!(1, ping_raw.len());
        assert_eq!(0, ping_raw[0].kind());
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
