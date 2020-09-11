use futures::{Stream, StreamExt};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

const CHANNEL_CAPACITY: usize = 128;

pub struct ConnectionsList {
    event_sink: broadcast::Sender<ConnectionEvent>,
    _event_recv: broadcast::Receiver<ConnectionEvent>,
    connection_list: Arc<RwLock<HashSet<SocketAddr>>>,
}

impl Clone for ConnectionsList {
    fn clone(&self) -> Self {
        let event_sink = self.event_sink.clone();
        let _event_recv = self.event_sink.subscribe();
        let connection_list = Arc::clone(&self.connection_list);
        Self {
            event_sink,
            _event_recv,
            connection_list,
        }
    }
}

impl ConnectionsList {
    pub fn new() -> Self {
        let (event_sink, _event_recv) = broadcast::channel(CHANNEL_CAPACITY);
        let connection_list = Arc::new(RwLock::new(HashSet::new()));
        Self {
            event_sink,
            _event_recv,
            connection_list,
        }
    }
    pub async fn contains(&self, addr: SocketAddr) -> bool {
        self.connection_list.read().await.contains(&addr)
    }
    pub async fn push_connection(&self, addr: SocketAddr) {
        if self.contains(addr).await {
            return;
        }

        let mut list_lock = self.connection_list.write().await;
        if !list_lock.insert(addr) {
            return;
        }
        let evt = ConnectionEvent::Add { addr };
        if self.event_sink.send(evt).is_err() {
            // Not possible since we hold a reciever as a member variable.
        }
    }
    pub async fn remove_connection(&self, addr: SocketAddr) {
        if !self.contains(addr).await {
            return;
        }

        let mut list_lock = self.connection_list.write().await;
        if !list_lock.remove(&addr) {
            return;
        }
        let evt = ConnectionEvent::Remove { addr };
        if self.event_sink.send(evt).is_err() {
            // Not possible since we hold a reciever as a member variable.
        }
    }
    pub async fn all_connections(&self) -> Vec<SocketAddr> {
        let list_lock = self.connection_list.read().await;
        list_lock.iter().copied().collect()
    }
    pub fn event_stream(&self) -> impl Stream<Item = ConnectionEvent> {
        self.event_sink
            .subscribe()
            .filter_map(|itm| async { itm.ok() })
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ConnectionEvent {
    Add { addr: SocketAddr },
    Remove { addr: SocketAddr },
}
