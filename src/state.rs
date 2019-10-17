use crate::communication::{
    ConnectionsThreadHandle, ConnnectionThread, FileTransferClient, FileTransferHost, FriendCode,
};
use crate::events::RemoteEvent;
use crate::players;
use crate::players::events::PlayerEvent;
use crate::traits::MediaPlayer;
use crate::{DebugError, MyResult};
use std::net::SocketAddr;
use std::sync::{self, Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Eq, PartialEq, Default)]
pub struct MediaOpenState {
    kind: MediaOpenStateKind,
    url: Option<String>,
}
impl MediaOpenState {
    pub fn with_url(self, url: String) -> Self {
        Self {
            url: Some(url),
            ..self
        }
    }

    pub fn okay() -> Self {
        MediaOpenState {
            kind: MediaOpenStateKind::Okay,
            ..Default::default()
        }
    }

    pub fn requesting() -> Self {
        MediaOpenState {
            kind: MediaOpenStateKind::Requesting,
            ..Default::default()
        }
    }
    pub fn transfering() -> Self {
        MediaOpenState {
            kind: MediaOpenStateKind::Transfering,
            ..Default::default()
        }
    }
    pub fn broadcasting() -> Self {
        MediaOpenState {
            kind: MediaOpenStateKind::Broadcasting,
            ..Default::default()
        }
    }

    pub fn is_broadcasting(&self, url: &str) -> bool {
        self.url.as_ref().map_or(false, |cur| cur == url)
            && self.kind == MediaOpenStateKind::Broadcasting
    }

    pub fn is_transfering(&self, url: &str) -> bool {
        self.url.as_ref().map_or(false, |cur| cur == url)
            && self.kind == MediaOpenStateKind::Transfering
    }
    pub fn is_requesting(&self, url: &str) -> bool {
        self.url.as_ref().map_or(false, |cur| cur == url)
            && self.kind == MediaOpenStateKind::Requesting
    }
}

#[derive(Eq, PartialEq, Clone, Copy)]
pub enum MediaOpenStateKind {
    Broadcasting,
    Requesting,
    Transfering,
    Okay,
}

impl Default for MediaOpenStateKind {
    fn default() -> Self {
        MediaOpenStateKind::Okay
    }
}

#[derive(Default)]
pub struct AppState {
    comms: Option<ConnectionsThreadHandle>,
    player: Option<Arc<Mutex<Box<dyn MediaPlayer>>>>,
    transfer_hosts: Vec<FileTransferHost>,
    transfers: Vec<FileTransferClient>,
    local_event_thread: Option<thread::JoinHandle<()>>,
    local_event_recv: Option<sync::mpsc::Receiver<RemoteEvent>>,
    state: Arc<RwLock<MediaOpenState>>,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start_comms(&mut self) -> MyResult<()> {
        if self.comms.is_none() {
            let comms = ConnnectionThread::open(40_000, 41_000)?.start()?;
            self.comms = Some(comms);
        }
        Ok(())
    }

    pub fn local_friendcode_str(&self) -> String {
        self.comms
            .as_ref()
            .map(|t| FriendCode::from_addr(t.local_addr()).as_friend_code())
            .unwrap_or_else(|| "ERROR: no comms thread found.".to_owned())
    }

    pub fn public_friendcode_str(&self) -> String {
        match self.comms.as_ref().map(|c| c.public_addr()) {
            Some(Some(s)) => FriendCode::from_addr(s).as_friend_code(),
            Some(None) => "NONE".to_owned(),
            None => "ERROR: no comms thread found.".to_owned(),
        }
    }

    pub fn open_public(&mut self) -> MyResult<()> {
        if let Some(comms) = self.comms.as_mut() {
            comms.open_public()?;
            Ok(())
        } else {
            Err("Error: called open_public without valid comms thread!"
                .to_owned()
                .into())
        }
    }

    pub fn search_players(&self) -> MyResult<Vec<(String, String)>> {
        Ok(players::all_player_lists()?
            .into_iter()
            .flat_map(|(list, ents)| ents.into_iter().map(move |v| (list.to_owned(), v)))
            .collect())
    }

    pub fn open_player(&mut self, list: &str, player: &str) -> MyResult<()> {
        let player = players::select_player(list, player)?;
        self.player = Some(Arc::new(Mutex::new(player)));
        self.start_local_event_thread()?;
        Ok(())
    }

    fn start_local_event_thread(&mut self) -> MyResult<()> {
        let player_ref = Arc::clone(
            self.player
                .as_ref()
                .ok_or_else(|| "Error: start_local_event_thread without active player!")?,
        );
        let mut comms_ref = self
            .comms
            .as_ref()
            .ok_or_else(|| "Error: start_local_event_thread without active comms thread!")?
            .clone();
        let mut prev_time = Instant::now();
        let state_ref = Arc::clone(&self.state);

        let (callback, event_sink) = sync::mpsc::channel();

        let handle = thread::spawn(move || loop {
            let cur_time = Instant::now();
            let ellapsed = cur_time - prev_time;
            let mut player_lock = player_ref.lock().unwrap();
            let player_events = player_lock.check_events(ellapsed).unwrap();
            let remote_events = comms_ref.check_events().unwrap();
            if comms_ref.connection_addresses().unwrap().is_empty() {
                let cur_state_bad = MediaOpenStateKind::Okay != state_ref.read().unwrap().kind;
                if cur_state_bad {
                    *state_ref.write().unwrap() = MediaOpenState::default();
                }
                continue;
            }
            let new_transfer = player_events.iter().find(|evt| {
                if let PlayerEvent::MediaOpen(_) = evt {
                    true
                } else {
                    false
                }
            });
            if let Some(PlayerEvent::MediaOpen(url)) = new_transfer {
                *state_ref.write().unwrap() = MediaOpenState::broadcasting().with_url(url.clone());
                comms_ref
                    .send_event(PlayerEvent::MediaOpen(url.clone()).into())
                    .unwrap();
            }
            let has_transfer = MediaOpenStateKind::Okay != state_ref.read().unwrap().kind;
            if !has_transfer {
                for evt in player_events {
                    comms_ref.send_event(evt.into()).unwrap();
                }
                comms_ref
                    .send_event(RemoteEvent::Ping(player_lock.ping().unwrap()))
                    .unwrap();
            } else if !player_events.iter().all(|evt| evt != &PlayerEvent::Play) {
                player_lock.send_event(PlayerEvent::Pause).unwrap();
            }
            for evt in remote_events {
                match evt {
                    RemoteEvent::Jump(tm) => {
                        if !has_transfer {
                            player_lock.send_event(PlayerEvent::Jump(tm)).unwrap();
                        }
                    }
                    RemoteEvent::Pause => {
                        if !has_transfer {
                            player_lock.send_event(PlayerEvent::Pause).unwrap();
                        }
                    }
                    RemoteEvent::Play => {
                        if !has_transfer {
                            player_lock.send_event(PlayerEvent::Play).unwrap();
                        }
                    }
                    RemoteEvent::Ping(png) => {
                        if !has_transfer {
                            player_lock.on_ping(png).unwrap();
                        }
                    }
                    RemoteEvent::MediaOpen(ref url) => {
                        let needs_transition = {
                            let state = state_ref.read().unwrap();
                            if let Some(ref old_url) = state.url {
                                if state.kind == MediaOpenStateKind::Okay {
                                    old_url != url
                                } else {
                                    false
                                }
                            } else {
                                false
                            }
                        };
                        if needs_transition {
                            player_lock.send_event(PlayerEvent::Pause).unwrap();
                            *state_ref.write().unwrap() =
                                MediaOpenState::requesting().with_url(url.to_owned());
                            callback.send(evt).unwrap();
                            break;
                        } else {
                            comms_ref
                                .send_event(RemoteEvent::MediaOpenOkay(url.to_owned()))
                                .unwrap();
                        }
                    }
                    RemoteEvent::RequestTransfer(ref url) => {
                        if state_ref.read().unwrap().is_broadcasting(url) {
                            callback.send(evt).unwrap();
                        }
                    }
                    RemoteEvent::RespondTransfer { .. } => {
                        if MediaOpenStateKind::Requesting == state_ref.read().unwrap().kind {
                            callback.send(evt).unwrap();
                        }
                    }
                    RemoteEvent::MediaOpenOkay(ref url) => {
                        let state = state_ref.read().unwrap();
                        let is_good = state.is_broadcasting(url)
                            || state.is_transfering(url)
                            || state.is_requesting(url);
                        if !is_good {
                            println!("WARN: got MediaOpenOkay({}) without being in the middle of a broadcast!", url);
                        } else {
                            *state_ref.write().unwrap() =
                                MediaOpenState::okay().with_url(url.clone());
                            player_lock.send_event(PlayerEvent::Play).unwrap();
                        }
                    }
                    RemoteEvent::Shutdown => {}
                }
            }
            prev_time = cur_time;
            std::thread::sleep(Duration::from_millis(20));
        });
        self.local_event_thread = Some(handle);
        self.local_event_recv = Some(event_sink);
        Ok(())
    }

    pub fn open_transfer_host(&mut self, url: String) -> MyResult<()> {
        if self.get_transfer_host(&url).is_none() {
            let new_thread = FileTransferHost::new(url)?;
            self.transfer_hosts.push(new_thread);
        }
        Ok(())
    }

    pub fn broadcast_transfer_host(&mut self, url: &str) -> MyResult<()> {
        if !self.state.read().unwrap().is_broadcasting(url) {
            return Err(format!(
                "ERROR: got transfer request for {} when not broadcasting!",
                url
            )
            .into());
        }
        let transfer_host = self.get_transfer_host(url).ok_or_else(|| {
            format!(
                "Tried broadcasting host for {} before the host started!",
                url
            )
        })?;
        let mut transfer_code = ['\0'; 9];
        for (outpt, inpt) in transfer_code
            .iter_mut()
            .zip(transfer_host.transfer_code().as_friend_code().chars())
        {
            *outpt = inpt;
        }
        let evt = RemoteEvent::RespondTransfer {
            size: transfer_host.file_size(),
            transfer_code: Some(transfer_code),
        };
        if let Some(c) = self.comms.as_mut() {
            c.send_event(evt)?;
            *self.state.write().unwrap() = MediaOpenState::transfering().with_url(url.to_owned());
        }
        Ok(())
    }

    pub fn get_transfer_host(&self, url: &str) -> Option<&FileTransferHost> {
        self.transfer_hosts.iter().find(|t| t.url() == url)
    }

    pub fn start_transfer(
        &mut self,
        url: String,
        file_size: u64,
        remote_addr: FriendCode,
    ) -> MyResult<()> {
        if !self.state.read().unwrap().is_requesting(&url) {
            return Err(format!("ERROR: Got start_transfer({}) without requesting!", url).into());
        }
        let has_transfer = self.transfers.iter().any(|t| t.url() == url);
        if !has_transfer {
            let new_transfer = FileTransferClient::new(url, file_size, remote_addr)?;
            self.transfers.push(new_transfer);
            Ok(())
        } else {
            Ok(())
        }
    }

    pub fn active_transfers(&self) -> impl Iterator<Item = &FileTransferClient> {
        self.transfers.iter().filter(|t| !t.is_finished())
    }

    pub fn media_open_okay(&mut self, url: String) -> MyResult<()> {
        if !self.state.read().unwrap().is_requesting(&url) {
            return Err(format!("Error: got unexpected MediaOpen for url {}", url).into());
        }
        let local_url = if url.starts_with("file://") {
            let local_path = self
                .transfers
                .iter()
                .find(|t| t.url() == url)
                .map(|c| c.local_file_path())
                .ok_or_else(|| format!("Error: cannot find local file for url {}", url))?;
            let local_path_str = local_path
                .to_str()
                .ok_or_else(|| format!("Error: could not parse path {:?}.", local_path))?;
            format!("file://{}", local_path_str)
        } else {
            url.clone()
        };
        if let Some(thrd) = &mut self.comms {
            thrd.send_event(RemoteEvent::MediaOpenOkay(url.clone()))?;
        } else {
            return Err("ERROR: comms thread panic before we could respond to the okay!".into());
        }
        if let Some(player_mutex) = self.player.as_ref() {
            let mut lock = player_mutex.lock().map_err(DebugError::into_myerror)?;
            lock.send_event(PlayerEvent::MediaOpen(local_url))?;
            *(self.state.write().unwrap()) = MediaOpenState::okay().with_url(url.clone());
            Ok(())
        } else {
            Err(format!("ERROR: no player found to open local file {}!", local_url).into())
        }
    }

    pub fn request_transfer(&mut self, url: String) -> MyResult<()> {
        if let Some(thrd) = &mut self.comms {
            thrd.send_event(RemoteEvent::RequestTransfer(url))
        } else {
            Err("ERROR: comms thread panic before we could request the transfer!".into())
        }
    }

    pub fn connections(&self) -> MyResult<Vec<SocketAddr>> {
        if let Some(c) = &self.comms {
            c.connection_addresses()
        } else {
            Ok(Vec::new())
        }
    }

    pub fn open_connection(&mut self, addr: SocketAddr) -> MyResult<()> {
        if let Some(c) = &mut self.comms {
            c.open_connection(addr)
        } else {
            Err(format!(
                "Error: tried connection to address {} without comms thread.",
                addr
            )
            .into())
        }
    }

    pub fn pop_event(&mut self) -> Option<RemoteEvent> {
        self.local_event_recv
            .as_ref()
            .and_then(|recv| match recv.try_recv() {
                Ok(evt) => Some(evt),
                Err(sync::mpsc::TryRecvError::Empty) => None,
                Err(sync::mpsc::TryRecvError::Disconnected) => {
                    MyResult::Err(sync::mpsc::TryRecvError::Disconnected.into()).unwrap()
                }
            })
    }

    pub fn comms_thread_up(&self) -> bool {
        self.comms
            .as_ref()
            .map(|c| !c.has_paniced())
            .unwrap_or(false)
    }
}
