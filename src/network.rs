pub mod friendcodes;
pub mod utils;

use crate::protocols::Message;
use crate::DynResult;
use utils::PublicAddr;

use futures::FutureExt as FutureFutureExt;
use futures::StreamExt as FutureStreamExt;
use futures::TryStreamExt as FutureTryStreamExt;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::stream::{Stream, StreamMap};
use tokio::sync::broadcast;
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;
type EventStream = futures::stream::BoxStream<'static, DynResult<Message>>;
type EventStreamStore = Arc<Mutex<StreamMap<SocketAddr, EventStream>>>;

pub struct EventSink {
    inner: WriteHalf<TcpStream>,
}

impl From<WriteHalf<TcpStream>> for EventSink {
    fn from(inner: WriteHalf<TcpStream>) -> Self {
        Self { inner }
    }
}

impl From<EventSink> for WriteHalf<TcpStream> {
    fn from(wrapped: EventSink) -> WriteHalf<TcpStream> {
        wrapped.inner
    }
}

impl EventSink {
    pub async fn write_message(&mut self, msg: Message) -> Result<(), std::io::Error> {
        let buff = msg.into_block();
        self.inner.write_all(&buff).await?;
        Ok(())
    }
}

type EventSinkStore = Arc<Mutex<Vec<EventSink>>>;

pub struct NetworkManager {
    local_addr: SocketAddr,
    public_addr: Option<PublicAddr>,
    event_sources: EventStreamStore,
    event_sinks: EventSinkStore,
    connection_task: BackgroundTask,
}

impl NetworkManager {
    pub fn new(listener: TcpListener) -> DynResult<Self> {
        let local_addr = listener.local_addr().unwrap();
        let public_addr = None;
        let (address_sink, _) = broadcast::channel(5);
        let event_sources = Arc::new(Mutex::new(StreamMap::new()));
        let event_sinks = Arc::new(Mutex::new(Vec::new()));
        let connection_task = BackgroundTask::new(
            listener,
            address_sink,
            Arc::clone(&event_sources),
            Arc::clone(&event_sinks),
        );
        Ok(Self {
            local_addr,
            public_addr,
            event_sources,
            event_sinks,
            connection_task,
        })
    }

    pub fn new_connections(&self) -> impl Stream<Item = DynResult<SocketAddr>> {
        self.connection_task
            .address_sink
            .subscribe()
            .map_err(|e| e.into())
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn public_addr(&self) -> Option<SocketAddr> {
        self.public_addr.as_ref().map(|p| p.addr())
    }

    pub async fn request_public(&mut self) -> DynResult<()> {
        if self.public_addr.is_some() {
            return Ok(());
        }
        let new_public = PublicAddr::request_public(self.local_addr).await?;
        self.public_addr = Some(new_public);
        Ok(())
    }

    pub async fn add_connection(&self, con: TcpStream) -> DynResult<()> {
        let addr = con.local_addr()?;
        let (read, write) = tokio::io::split(con);
        let mut sources_lock = self.event_sources.lock().await;
        let mut sinks_lock = self.event_sinks.lock().await;
        sources_lock.insert(addr, make_event_stream(read));
        sinks_lock.push(write.into());
        if let Err(_e) = self.connection_task.address_sink.send(addr) {}
        Ok(())
    }

    pub fn remote_event_stream(&self) -> impl Stream<Item = DynResult<Message>> {
        let sources = Arc::clone(&self.event_sources);
        futures::stream::unfold(sources, |srcref| async move {
            let nxt = loop {
                let mut sources_lock = srcref.lock().await;
                if sources_lock.is_empty() {
                    drop(sources_lock);
                    tokio::task::yield_now().await;
                    continue;
                } else {
                    break sources_lock.next().await;
                }
            };
            match nxt {
                Some((_, n)) => {
                    let retvl = (n, srcref);
                    tokio::task::yield_now().await;
                    Some(retvl)
                }
                None => Some((Err("No streams in sources!".to_owned().into()), srcref)),
            }
        })
    }

    pub async fn broadcast_event(&self, msg: Message) -> DynResult<()> {
        let mut sink_lock = self.event_sinks.lock().await;
        let sink_iter = futures::stream::iter(sink_lock.drain(..)).map(DynResult::Ok);
        let new_sink_res: Result<Vec<_>, _> = sink_iter
            .try_filter_map(|mut writer| async move {
                let res = writer.write_message(msg).await;
                match res {
                    Ok(()) => Ok(Some(writer)),
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {
                        eprintln!("  WARNING: Connection was reset.");
                        Ok(None)
                    }
                    e => {
                        e.unwrap();
                        unimplemented!()
                    }
                }
            })
            .try_collect()
            .await;
        let new_sinks = new_sink_res.unwrap();
        *sink_lock = new_sinks;
        Ok(())
    }
}

impl Drop for NetworkManager {
    fn drop(&mut self) {
        self.connection_task.abort();
    }
}

struct BackgroundTask {
    #[allow(unused)]
    handle: JoinHandle<()>,
    die_signal: Option<oneshot::Sender<()>>,
    address_sink: broadcast::Sender<SocketAddr>,
}
impl BackgroundTask {
    pub fn new(
        mut listener: TcpListener,
        address_sink: broadcast::Sender<SocketAddr>,
        event_sources: EventStreamStore,
        event_sinks: EventSinkStore,
    ) -> Self {
        let as2 = address_sink.clone();
        let (die_signal, kill_signal) = oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            let mut constream = listener.incoming();
            let mut die_future = kill_signal.fuse();
            loop {
                let mut nxtfut = constream.next().fuse();
                let con = futures::select! {
                    con = nxtfut => con,
                    dropped = die_future => {break;}
                };
                let con = match con {
                    Some(Ok(c)) => c,
                    Some(Err(_e)) => unimplemented!(),
                    None => {
                        break;
                    }
                };
                let addr = con.peer_addr().unwrap();
                let (read, write) = tokio::io::split(con);
                let mut sources_lock = event_sources.lock().await;
                let mut sinks_lock = event_sinks.lock().await;
                sources_lock.insert(addr, make_event_stream(read));
                sinks_lock.push(write.into());
                drop(sources_lock);
                drop(sinks_lock);
                as2.send(addr).unwrap();
                tokio::task::yield_now().await;
            }
        });
        Self {
            die_signal: Some(die_signal),
            address_sink,
            handle,
        }
    }
    pub fn abort(&mut self) {
        let sndres = self.die_signal.take().map(|s| s.send(()));
        match sndres {
            _ => {}
        }
    }
}

impl Drop for BackgroundTask {
    fn drop(&mut self) {
        self.abort();
    }
}

pub fn make_event_stream(reader: ReadHalf<TcpStream>) -> EventStream {
    futures::stream::unfold(reader, |mut rdr| async {
        let mut buff = [0; 32];
        let res = rdr.read_exact(&mut buff).await;
        let retvl = res
            .map_err(|e| e.into())
            .and_then(|_| Message::parse_block(buff))
            .unwrap();
        Some((Ok(retvl), rdr))
    })
    .boxed()
}
