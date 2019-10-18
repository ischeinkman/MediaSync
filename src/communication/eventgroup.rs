use crate::events::RemoteEvent;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct PlayerEventGroup {
    media_open_event: Vec<RemoteEvent>,
    is_paused: Option<bool>,
    did_jump: bool,
    position: Option<Duration>,
    position_reference: Option<Duration>,
}

impl Default for PlayerEventGroup {
    fn default() -> Self {
        PlayerEventGroup::new()
    }
}

impl std::iter::FromIterator<RemoteEvent> for PlayerEventGroup {
    fn from_iter<I: IntoIterator<Item = RemoteEvent>>(iter: I) -> Self {
        let mut retvl = PlayerEventGroup::new();
        for evt in iter {
            retvl = retvl.with_event(evt);
        }
        retvl
    }
}

impl PlayerEventGroup {
    pub fn new() -> Self {
        PlayerEventGroup {
            media_open_event: Vec::new(),
            is_paused: None,
            did_jump: false,
            position: None,
            position_reference: None,
        }
    }

    pub fn with_event(mut self, event: RemoteEvent) -> Self {
        if let Some(RemoteEvent::MediaOpen(_)) = self.media_open_event.last() {
            return self;
        }
        match event {
            RemoteEvent::Pause => self.is_paused = Some(true),
            RemoteEvent::Play => self.is_paused = Some(false),
            RemoteEvent::Jump(dt) => {
                self.position = Some(dt);
                self.position_reference = None;
                self.did_jump = true;
            }
            RemoteEvent::MediaOpen(_) => {
                self.media_open_event.push(event);
                self.is_paused = None;
                self.did_jump = false;
                self.position = None;
                return self;
            }
            RemoteEvent::MediaOpenOkay(_)
            | RemoteEvent::RequestTransfer(_)
            | RemoteEvent::RespondTransfer { .. } => {
                self.media_open_event.insert(0, event);
            }
            RemoteEvent::Ping { payload, timestamp } => {
                self.position.get_or_insert(payload.time());
                self.position_reference.get_or_insert(timestamp);
            }
            RemoteEvent::Shutdown => {
                return PlayerEventGroup::new();
            }
        }
        self
    }

    pub fn into_events(self) -> impl Iterator<Item = RemoteEvent> {
        let status_event = self
            .is_paused
            .map(|paused| {
                if paused {
                    RemoteEvent::Pause
                } else {
                    RemoteEvent::Play
                }
            })
            .into_iter();
        let position_event = if self.did_jump {
            let pos = self.position.unwrap(); //(Duration::from_micros(0));
            Some(RemoteEvent::Jump(pos)).into_iter()
        } else {
            self.position
                .and_then(|pos| self.position_reference.map(|rf| ((pos).into(), rf)))
                .map(|(payload, timestamp)| RemoteEvent::Ping { payload, timestamp })
                .into_iter()
        };
        let media_event = self.media_open_event.into_iter();
        media_event.chain(position_event).chain(status_event)
    }

    pub fn cascade(
        mut self,
        self_addr: SocketAddr,
        other: &PlayerEventGroup,
        other_addr: SocketAddr,
    ) -> PlayerEventGroup {
        if let Some(RemoteEvent::MediaOpen(_)) = self.media_open_event.last() {
            let mut other_events = other.media_open_event.clone();
            other_events.extend_from_slice(&self.media_open_event);
            self.media_open_event = other_events;
            self.is_paused = None;
            self.did_jump = false;
            self.position = None;
            self
        } else if let Some(RemoteEvent::MediaOpen(_)) = other.media_open_event.last() {
            self.media_open_event
                .extend_from_slice(&other.media_open_event);
            self.is_paused = None;
            self.did_jump = false;
            self.position = None;
            self
        } else {
            let mut rectified_self = self.rectify(self_addr, other, other_addr);
            rectified_self
                .media_open_event
                .extend_from_slice(&other.media_open_event);
            let position_reference = if rectified_self.did_jump || other.did_jump {
                None
            } else {
                rectified_self
                    .position_reference
                    .or(other.position_reference)
            };
            PlayerEventGroup {
                media_open_event: rectified_self.media_open_event,
                is_paused: rectified_self.is_paused.or(other.is_paused),
                did_jump: rectified_self.did_jump || other.did_jump,
                position: rectified_self.position.or(other.position),
                position_reference,
            }
        }
    }

    pub fn rectify(
        self,
        self_addr: SocketAddr,
        other: &PlayerEventGroup,
        other_addr: SocketAddr,
    ) -> PlayerEventGroup {
        let self_has_priority =
            priority_ordering(self_addr, other_addr) == std::cmp::Ordering::Greater;
        match (self.media_open_event.last(), other.media_open_event.last()) {
            (Some(RemoteEvent::MediaOpen(ref self_url)), Some(RemoteEvent::MediaOpen(_))) => {
                if self_has_priority {
                    return PlayerEventGroup::new()
                        .with_event(RemoteEvent::MediaOpen(self_url.to_owned()));
                } else {
                    return PlayerEventGroup::new();
                }
            }
            (Some(RemoteEvent::MediaOpen(ref self_url)), _) => {
                return PlayerEventGroup::new()
                    .with_event(RemoteEvent::MediaOpen(self_url.to_owned()));
            }
            (_, Some(RemoteEvent::MediaOpen(_))) => {
                return PlayerEventGroup::new();
            }
            _ => {}
        }

        let mut retvl = PlayerEventGroup::new();
        retvl.media_open_event = self.media_open_event;
        retvl.did_jump = self.did_jump && (!other.did_jump || self_has_priority);
        let use_self_position = if self.did_jump == other.did_jump {
            self_has_priority
        } else {
            self.did_jump
        };

        retvl.position = if use_self_position || other.position.is_none() {
            self.position
        } else {
            None
        };

        retvl.is_paused = if self_has_priority || other.is_paused.is_none() {
            self.is_paused
        } else {
            None
        };
        if !retvl.did_jump && retvl.position.is_some() {
            retvl.position_reference = self.position_reference;
        }
        retvl
    }
}

pub(crate) fn priority_ordering(
    self_addr: SocketAddr,
    other_addr: SocketAddr,
) -> std::cmp::Ordering {
    if self_addr == other_addr {
        return std::cmp::Ordering::Equal;
    }

    let self_public = match self_addr.ip() {
        IpAddr::V4(ip) => !ip.is_private(),
        IpAddr::V6(_) => true,
    };
    let other_public = match other_addr.ip() {
        IpAddr::V4(ip) => !ip.is_private(),
        IpAddr::V6(_) => true,
    };
    let self_bytes: u128 = match self_addr.ip() {
        IpAddr::V4(ip) => ip.to_ipv6_compatible().into(),
        IpAddr::V6(ip) => ip.into(),
    };
    let other_bytes: u128 = match other_addr.ip() {
        IpAddr::V4(ip) => ip.to_ipv6_compatible().into(),
        IpAddr::V6(ip) => ip.into(),
    };

    let public_cmp = if self_public && !other_public {
        std::cmp::Ordering::Greater
    } else if !self_public && other_public {
        std::cmp::Ordering::Less
    } else {
        std::cmp::Ordering::Equal
    };
    public_cmp
        .then(self_bytes.cmp(&other_bytes))
        .then(self_addr.port().cmp(&other_addr.port()))
}

#[cfg(test)]
mod test {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    #[test]
    fn test_ip_priority() {
        let ip_a = SocketAddr::from(([192, 168, 1, 32], 49111));
        let events_a = PlayerEventGroup::new()
            .with_event(RemoteEvent::Ping {
                payload: Duration::from_millis(10_000).into(),
                timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap(),
            })
            .with_event(RemoteEvent::Pause);
        let ip_b = SocketAddr::from(([128, 14, 1, 32], 4));
        let events_b = PlayerEventGroup::new()
            .with_event(RemoteEvent::Ping {
                payload: Duration::from_millis(1_000).into(),
                timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap(),
            })
            .with_event(RemoteEvent::Play);

        let rectified_b = events_b.clone().rectify(ip_b, &events_a, ip_a);
        let rectified_a = events_a.clone().rectify(ip_a, &events_b, ip_b);
        assert_eq!(
            Vec::<RemoteEvent>::new(),
            rectified_a.into_events().collect::<Vec<_>>()
        );
        assert_eq!(events_b, rectified_b);
    }

    #[test]
    fn test_jump_priority() {
        let ip_a = SocketAddr::from(([192, 168, 1, 32], 49111));
        let events_a = PlayerEventGroup::new()
            .with_event(RemoteEvent::Jump(Duration::from_millis(10_000)))
            .with_event(RemoteEvent::Pause);
        let ip_b = SocketAddr::from(([128, 14, 1, 32], 4));
        let events_b = PlayerEventGroup::new()
            .with_event(RemoteEvent::Ping {
                payload: Duration::from_millis(1_000).into(),
                timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap(),
            })
            .with_event(RemoteEvent::Play);

        let rectified_b = events_b.clone().rectify(ip_b, &events_a, ip_a);
        let rectified_a = events_a.clone().rectify(ip_a, &events_b, ip_b);
        assert_eq!(
            vec![RemoteEvent::Jump(Duration::from_millis(10_000))],
            rectified_a.into_events().collect::<Vec<_>>()
        );
        assert_eq!(
            vec![RemoteEvent::Play],
            rectified_b.into_events().collect::<Vec<_>>()
        );

        let cascaded_a = events_a.clone().cascade(ip_a, &events_b, ip_b);
        let cascaded_b = events_b.clone().cascade(ip_b, &events_a, ip_a);
        assert_eq!(cascaded_a, cascaded_b);
        assert_eq!(
            vec![
                RemoteEvent::Jump(Duration::from_millis(10_000)),
                RemoteEvent::Play
            ],
            cascaded_a.into_events().collect::<Vec<_>>()
        );
    }
}
