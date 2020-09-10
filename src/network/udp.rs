use crate::network::friendcodes::FriendCode;
use crate::network::publicaddr::PublicAddr;
use crate::network::utils::random_localaddr;
use crate::messages::Message;
use crate::DynResult;

use futures::stream::FuturesUnordered;
use futures::StreamExt as FutureStreamExt;
use futures::TryStreamExt as FutureTryStreamExt;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::udp::{RecvHalf, SendHalf};
use tokio::net::UdpSocket;
use tokio::stream::Stream;
use tokio::sync::mpsc;
use tokio::sync::{Mutex, RwLock};

fn make_event_stream(
    reader: Arc<Mutex<Vec<RecvHalf>>>,
) -> futures::stream::BoxStream<'static, DynResult<(SocketAddr, Message)>> {
    futures::stream::unfold(reader, move |rdr| async move {
        let mut rdrlock = rdr.lock().await;
        let mut futs: FuturesUnordered<_> = rdrlock
            .iter_mut()
            .map(|sock| async move {
                let mut buff = [0; 32];
                let res = sock.recv_from(&mut buff).await;
                res.map_err(|e| e.into()).and_then(|(_, addr)| {
                    let msg = Message::parse_block(buff)?;
                    Ok((addr, msg))
                })
            })
            .collect();
        let nxt = futs.select_next_some().await;
        drop(futs);
        drop(rdrlock);
        Some((nxt, rdr))
    })
    .boxed()
}

struct EventSink {
    addresses: Arc<RwLock<Vec<SocketAddr>>>,
    senders: Vec<SendHalf>,
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
                let mut ra = self
                    .senders
                    .iter_mut()
                    .map(|snd| async move { snd.send_to(&msg.into_block(), addr).await })
                    .collect::<FuturesUnordered<_>>();
                let mut cur_res = Err(std::io::Error::from(std::io::ErrorKind::NotConnected));
                while cur_res.is_err() {
                    if let Some(nxt) = ra.next().await {
                        cur_res = nxt;
                    }
                }
                let res = cur_res;
                retvl.push((*addr, res.map(|_| ())));
            }
            futures::stream::iter(retvl)
        };
        futures::stream::once(retfut).flatten()
    }
}

#[allow(dead_code)]
pub struct NetworkManager {
    local_addr: SocketAddr,
    public_addr: RwLock<Option<PublicAddr>>,
    connection_addrs: Arc<RwLock<Vec<SocketAddr>>>,
    event_sinks: Mutex<EventSink>,
    new_connections_sink: Arc<SpmcSend<SocketAddr>>,
    recv: Arc<Mutex<Vec<RecvHalf>>>,
}

#[allow(dead_code)]
impl NetworkManager {
    pub async fn new_random_port(min_port: u16, max_port: u16) -> DynResult<Self> {
        let addr = random_localaddr(min_port, max_port).await.unwrap();
        let listener = UdpSocket::bind(addr).await.unwrap();
        Self::new(listener)
    }

    fn new(listener: UdpSocket) -> DynResult<Self> {
        let local_addr = listener.local_addr().unwrap();
        let (rawrecv, rawsnd) = listener.split();
        let recv = Arc::new(Mutex::new(vec![rawrecv]));
        let connection_addrs = Arc::new(RwLock::new(Vec::new()));
        let event_sinks = Mutex::new(EventSink {
            addresses: Arc::clone(&connection_addrs),
            senders: vec![rawsnd],
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

    pub fn new_connections(&self) -> impl Stream<Item = DynResult<SocketAddr>> {
        let stream = self.new_connections_sink.new_recv();
        futures::stream::unfold(stream, |mut strm| async {
            let nxt = strm.next().await?;
            Some((Ok(nxt), strm))
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn public_addr(&self) -> Option<SocketAddr> {
        self.public_addr.read().await.as_ref().map(|p| p.addr())
    }

    pub async fn request_public(&self) -> DynResult<()> {
        let mut public_addr_lock = self.public_addr.write().await;
        if public_addr_lock.is_some() {
            return Ok(());
        }
        let local_ip = self.local_addr.ip();
        let mut new_port = UdpSocket::bind((local_ip, 0)).await?;
        let pubaddr = PublicAddr::request_public(&mut new_port).await?;

        let (new_recv, new_send) = new_port.split();
        let mut sink_lock = self.event_sinks.lock().await;
        let mut stream_lock = self.recv.lock().await;
        sink_lock.senders.push(new_send);
        stream_lock.push(new_recv);
        *public_addr_lock = Some(pubaddr);
        Ok(())
    }
    pub async fn connect_to(&self, code: FriendCode) -> DynResult<()> {
        let addr = code.as_addr();
        self.add_connection(addr).await
    }
    async fn add_connection(&self, addr: SocketAddr) -> DynResult<()> {
        self.connection_addrs.write().await.push(addr);
        self.new_connections_sink.send(addr).await;
        Ok(())
    }

    pub fn remote_event_stream(&self) -> impl Stream<Item = DynResult<Message>> {
        let constream_ref = Arc::clone(&self.new_connections_sink);
        let conref = Arc::clone(&self.connection_addrs);
        make_event_stream(Arc::clone(&self.recv)).and_then(move |(addr, msg)| {
            let constream_ref = Arc::clone(&constream_ref);
            let conref = Arc::clone(&conref);
            async move {
                let existing_lock = conref.read().await;
                let already_exists = existing_lock.contains(&addr);
                drop(existing_lock);
                if !already_exists {
                    conref.write().await.push(addr);
                    constream_ref.send(addr).await;
                }
                Ok(msg)
            }
        })
    }

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
        SpmcRecv {
            queue,
            recv,
            updator: self.updator.clone(),
        }
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
    pub async fn send(&self, val: T) {
        let mut cb_lock = self.new_callbacks_queue.lock().await;
        let mut new_callbacks = Vec::new();
        while let Ok((queue, signal)) = cb_lock.try_recv() {
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
