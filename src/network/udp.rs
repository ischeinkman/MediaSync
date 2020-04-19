use crate::network::utils::udp::{PublicAddr, random_listener};
use crate::protocols::Message;
use crate::DynResult;

use futures::StreamExt as FutureStreamExt;
use futures::TryStreamExt as FutureTryStreamExt;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::udp::{RecvHalf, SendHalf};
use tokio::net::UdpSocket;
use tokio::stream::Stream;
use tokio::sync::mpsc;
use tokio::sync::{Mutex, RwLock};

pub fn make_event_stream(
    reader: Arc<Mutex<RecvHalf>>,
) -> futures::stream::BoxStream<'static, DynResult<(SocketAddr, Message)>> {
    futures::stream::unfold(reader, move |rdr| async move {
        let mut buff = [0; 32];
        let mut rdrlock = rdr.lock().await;
        let res = rdrlock.recv_from(&mut buff).await;
        drop(rdrlock);
        let nxt = res.map_err(|e| e.into()).and_then(|(_, addr)| {
            let msg = Message::parse_block(buff)?;
            Ok((addr, msg))
        });
        Some((nxt, rdr))
    })
    .boxed()
}

pub struct EventSink {
    addresses: Arc<RwLock<Vec<SocketAddr>>>,
    sender: SendHalf,
}

impl EventSink {
    pub fn write_message<'a>(
        &'a mut self,
        msg: Message,
    ) -> impl Stream<Item = (SocketAddr, Result<(), std::io::Error>)> + 'a {
        let retfut = async move {
            let addresses = self.addresses.read().await;
            let mut retvl = Vec::new();
            for addr in addresses.iter() {
                let res = self.sender.send_to(&msg.into_block(), addr).await;
                retvl.push((addr.clone(), res.map(|_| ())));
            }
            futures::stream::iter(retvl)
        };
        futures::stream::once(retfut).flatten()
    }
}

pub struct NetworkManager {
    local_addr: SocketAddr,
    public_addr: RwLock<Option<PublicAddr>>,
    connection_addrs: Arc<RwLock<Vec<SocketAddr>>>,
    event_sinks: Mutex<EventSink>,
    new_connections_sink: Arc<SpmcSend<SocketAddr>>,
    recv: Arc<Mutex<RecvHalf>>,
}

impl NetworkManager {

    #[allow(dead_code)]
    pub async fn new_random_port(min_port : u16, max_port : u16) -> DynResult<Self> {
        let listener = random_listener(min_port, max_port).await?;
        Self::new(listener)
    }

    pub fn new(listener: UdpSocket) -> DynResult<Self> {
        let local_addr = listener.local_addr().unwrap();
        let (rawrecv, rawsnd) = listener.split();
        let recv = Arc::new(Mutex::new(rawrecv));
        let connection_addrs = Arc::new(RwLock::new(Vec::new()));
        let event_sinks = Mutex::new(EventSink {
            addresses: Arc::clone(&connection_addrs),
            sender: rawsnd,
        });
        let new_connections_sink = Arc::new(SpmcSend::new());
        let retvl = Self {
            local_addr,
            public_addr: RwLock::new(None),
            connection_addrs,
            event_sinks,
            new_connections_sink,
            recv,
        };
        Ok(retvl)
    }

    #[allow(dead_code)]
    pub fn new_connections(&self) -> impl Stream<Item = DynResult<SocketAddr>> {
        let stream = self.new_connections_sink.new_recv();
        futures::stream::unfold(stream, |mut strm| async {
            let nxt = strm.next().await?;
            Some((Ok(nxt), strm))
        })
    }

    #[allow(dead_code)]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    #[allow(dead_code)]
    pub async fn public_addr(&self) -> Option<SocketAddr> {
        self.public_addr.read().await.as_ref().map(|p| p.addr())
    }

    #[allow(dead_code)]
    pub async fn request_public(&self) -> DynResult<()> {
        let mut public_addr_lock = self.public_addr.write().await;
        if public_addr_lock.is_some() {
            return Ok(());
        }
        let new_public = PublicAddr::request_public(self.local_addr).await?;
        *public_addr_lock = Some(new_public);
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn add_connection(&self, addr: SocketAddr) -> DynResult<()> {
        self.connection_addrs.write().await.push(addr.clone());
        self.new_connections_sink.send(addr).await;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn remote_event_stream(&self) -> impl Stream<Item = DynResult<Message>> {
        let constream_ref = Arc::clone(&self.new_connections_sink);
        let conref = Arc::clone(&self.connection_addrs);
        make_event_stream(Arc::clone(&self.recv)).and_then(move |(addr, msg)| {
            let constream_ref = Arc::clone(&constream_ref);
            let conref = Arc::clone(&conref);
            async move {
                conref.write().await.push(addr.clone());
                constream_ref.send(addr).await;
                Ok(msg)
            }
        })
    }

    #[allow(dead_code)]
    pub async fn broadcast_event(&self, msg: Message) -> DynResult<()> {
        let mut lock = self.event_sinks.lock().await;
        let res_stream = lock.write_message(msg);
        res_stream
            .filter_map(|(addr, res)| async move {
                match res {
                    Ok(()) => None,
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {
                        log::warn!("Connection was reset.");
                        Some(Ok(addr))
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                        log::warn!("Connection pipe was broken.");
                        Some(Ok(addr))
                    }
                    Err(e) => Some(Err(e.into())),
                }
            })
            .try_for_each(|baddr| async move {
                let mut lock = self.connection_addrs.write().await;
                for idx in 0..lock.len() {
                    let matches = {
                        let itm = lock.get(idx);
                        itm == Some(&baddr)
                    };
                    if matches {
                        lock.remove(idx);
                        break;
                    }
                }
                DynResult::Ok(())
            })
            .await
    }
}

pub struct SpmcRecv<T: Clone> {
    queue: Arc<Mutex<Vec<T>>>,
    recv: tokio::sync::watch::Receiver<()>,
    updator: mpsc::UnboundedSender<(Arc<Mutex<Vec<T>>>, tokio::sync::watch::Sender<()>)>,
}

impl<T: Clone> SpmcRecv<T> {
    pub async fn next(&mut self) -> Option<T> {
        loop {
            let mut qlock = self.queue.lock().await;
            if !qlock.is_empty() {
                return Some(qlock.remove(0));
            }
            drop(qlock);
            self.recv.recv().await?;
        }
    }
}

impl<T: Clone> Clone for SpmcRecv<T> {
    fn clone(&self) -> Self {
        let queue = Arc::new(Mutex::new(Vec::new()));
        let (snd, recv) = tokio::sync::watch::channel(());
        let _cbres = self.updator.send((Arc::clone(&queue), snd));
        let retvl = SpmcRecv {
            queue,
            recv,
            updator: self.updator.clone(),
        };
        retvl
    }
}

pub struct SpmcSend<T: Clone> {
    new_callbacks_queue:
        Mutex<mpsc::UnboundedReceiver<(Arc<Mutex<Vec<T>>>, tokio::sync::watch::Sender<()>)>>,
    updator: mpsc::UnboundedSender<(Arc<Mutex<Vec<T>>>, tokio::sync::watch::Sender<()>)>,
}

impl<T: Clone> SpmcSend<T> {
    pub fn new() -> Self {
        let (updator, raw_new_callbacks_queue) = mpsc::unbounded_channel();
        let new_callbacks_queue = Mutex::new(raw_new_callbacks_queue);
        Self {
            new_callbacks_queue,
            updator,
        }
    }
    #[allow(dead_code)]
    pub async fn send(&self, val: T) {
        let mut cb_lock = self.new_callbacks_queue.lock().await;
        let mut new_callbacks = Vec::new();
        loop {
            let (queue, signal) = match cb_lock.try_recv() {
                Ok(cb) => cb,
                Err(_e) => {
                    break;
                }
            };
            let mut qlock = queue.lock().await;
            qlock.push(val.clone());
            drop(qlock);
            let nxt = match signal.broadcast(()) {
                Ok(()) => Some((queue, signal)),
                Err(_) => None,
            };
            if let Some(nxt) = nxt {
                new_callbacks.push(nxt);
            }
        }
        for cb in new_callbacks.into_iter() {
            let res = self.updator.send(cb);
            debug_assert!(res.is_ok());
        }
    }
    pub fn new_recv(&self) -> SpmcRecv<T> {
        let queue = Arc::new(Mutex::new(Vec::new()));
        let (snd, recv) = tokio::sync::watch::channel(());
        let cbres = self.updator.send((Arc::clone(&queue), snd));
        assert!(!cbres.is_err());
        SpmcRecv {
            queue,
            recv,
            updator: self.updator.clone(),
        }
    }
}
