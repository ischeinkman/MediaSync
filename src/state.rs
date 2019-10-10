use crate::communication::{ConnectionsThreadHandle, ConnnectionThread, FileTransferClient, FileTransferHost, FriendCodeV4};
use crate::traits::{MediaPlayer};
use crate::events::{PlayerEvent, RemoteEvent};
use crate::players;
use crate::{MyResult, DebugError};
use std::sync::{self, Arc, Mutex, RwLock};
use std::thread;
use std::net::{SocketAddr};
use std::time::{Instant, Duration};


#[derive(Eq, PartialEq, Clone)]
pub enum MediaOpenState {
    Broadcasting(String),
    Requesting(String),
    Transfering(String),
    Okay(Option<String>),
}

impl Default for MediaOpenState {
    fn default() -> Self {
        MediaOpenState::Okay(None)
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
        if self.comms.is_some() {
            Ok(())
        } else {
            let comms = ConnnectionThread::open(40_000, 41_000)?.start()?;
            self.comms = Some(comms);
            Ok(())
        }
    }

    pub fn local_friendcode_str(&self) -> String {
        self.comms
            .as_ref()
            .map(|t| match t.local_addr() {
                SocketAddr::V4(v) => FriendCodeV4::from_addr(v).as_friend_code().iter().collect(),
                SocketAddr::V6(v) => format!("Unsupported addr: {:?}", v),
            })
            .unwrap_or_else(|| "ERROR: no comms thread found.".to_owned())
    }

    pub fn public_friendcode_str(&self) -> String {
        match self.comms.as_ref().map(|c| c.public_addr()) {
            Some(Some(SocketAddr::V4(v))) => {
                FriendCodeV4::from_addr(v).as_friend_code().iter().collect()
            }
            Some(Some(SocketAddr::V6(v))) => format!("Unsupported addr: {:?}", v),
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
                crate::debug_print("REMOTE: No connections found.".to_owned());
                let cur_state_bad = if let MediaOpenState::Okay(_) = *state_ref.read().unwrap() {
                    false
                } else {
                    true
                };
                if cur_state_bad {
                    *state_ref.write().unwrap() = MediaOpenState::Okay(None);
                }
                continue;
            }
            let has_transfer = if let MediaOpenState::Okay(_) = *state_ref.read().unwrap() {
                if let Some(PlayerEvent::MediaOpen(url)) = player_events.iter().find(|evt| {
                    if let PlayerEvent::MediaOpen(_) = evt {
                        true
                    } else {
                        false
                    }
                }) {
                    comms_ref
                        .send_event(PlayerEvent::MediaOpen(url.clone()).into())
                        .unwrap();
                    player_lock.send_event(PlayerEvent::Pause).unwrap();
                    *state_ref.write().unwrap() = MediaOpenState::Broadcasting(url.clone());
                    crate::debug_print(format!("REMOTE: state transition to Broadcast({})", url));
                    true
                } else {
                    false
                }
            } else {
                true
            };
            if !has_transfer {
                for evt in player_events {
                    crate::debug_print(format!("REMOTE: I'm sending event: {:?}", evt));
                    comms_ref.send_event(evt.into()).unwrap();
                }
                comms_ref
                    .send_event(RemoteEvent::Ping(player_lock.ping().unwrap()))
                    .unwrap();
            } else {
                crate::debug_print(format!("REMOTE: Not sending events: has transfer"));
            }
            crate::debug_print(format!("REMOTE: recieved events {:?}", remote_events));
            for evt in remote_events {
                match evt {
                    RemoteEvent::Jump(tm) => {
                        crate::debug_print(format!("REMOTE: sent Jump {}", tm.as_micros()));
                        if !has_transfer {
                            player_lock.send_event(PlayerEvent::Jump(tm)).unwrap();
                        }
                    }
                    RemoteEvent::Pause => {
                        crate::debug_print(format!("REMOTE: sent Pause!"));
                        if !has_transfer {
                            player_lock.send_event(PlayerEvent::Pause).unwrap();
                        }
                    }
                    RemoteEvent::Play => {
                        crate::debug_print(format!("REMOTE: sent Play!"));
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
                        crate::debug_print(format!("REMOTE: sent MediaOpen: {}", url));
                        let needs_transition =
                            if let MediaOpenState::Okay(ref old_url) = *state_ref.read().unwrap() {
                                old_url != &Some(url.clone())
                            } else {
                                false
                            };
                        if needs_transition {
                            player_lock.send_event(PlayerEvent::Pause).unwrap();
                            *state_ref.write().unwrap() =
                                MediaOpenState::Requesting(url.to_owned());
                            callback.send(evt).unwrap();
                        } else {
                            comms_ref
                                .send_event(RemoteEvent::MediaOpenOkay(url.to_owned()))
                                .unwrap();
                        }
                    }
                    RemoteEvent::RequestTransfer(ref url) => {
                        crate::debug_print(format!("Remote sent RequestTransfer: {}", url));
                        if *state_ref.read().unwrap()
                            == MediaOpenState::Broadcasting(url.to_owned())
                        {
                            callback.send(evt).unwrap();
                        }
                    }
                    RemoteEvent::RespondTransfer { .. } => {
                        crate::debug_print(format!("Remote sent RespondTransfer"));
                        if let MediaOpenState::Requesting { .. } = *state_ref.read().unwrap() {
                            callback.send(evt).unwrap();
                        }
                    }
                    RemoteEvent::MediaOpenOkay(ref url) => {
                        crate::debug_print(format!("Remote sent MediaOpenOkay: {}", url));
                        let state = state_ref.read().unwrap();
                        let is_bad = *state != MediaOpenState::Broadcasting(url.clone())
                            && *state != MediaOpenState::Transfering(url.clone());
                        if is_bad {
                            println!("WARN: got MediaOpenOkay({}) without being in the middle of a broadcast!", url);
                        } else {
                            *state_ref.write().unwrap() = MediaOpenState::Okay(Some(url.clone()));
                            player_lock.send_event(PlayerEvent::Play).unwrap();
                        }
                    }
                    RemoteEvent::Shutdown => {
                        crate::debug_print(format!("Remote sent Shutdown"));
                    }
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
        if *self.state.read().unwrap() != MediaOpenState::Broadcasting(url.to_owned()) {
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
        let evt = RemoteEvent::RespondTransfer {
            size: transfer_host.file_size(),
            transfer_code: Some(transfer_host.transfer_code()?.as_friend_code()),
        };
        if let Some(c) = self.comms.as_mut() {
            c.send_event(evt)?;
            crate::debug_print(format!(
                "LOCAL: state transition: Broadcasting({}) => Transfering({})",
                url, url
            ));
            *self.state.write().unwrap() = MediaOpenState::Transfering(url.to_owned());
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
        remote_addr: FriendCodeV4,
    ) -> MyResult<()> {
        if *self.state.read().unwrap() != MediaOpenState::Requesting(url.clone()) {
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
        if *self.state.read().unwrap() != MediaOpenState::Requesting(url.clone()) {
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
            crate::debug_print(format!(
                "LOCAL: state transition: Requesting({}) => Okay({})",
                url, url
            ));
            *(self.state.write().unwrap()) = MediaOpenState::Okay(Some(url.to_owned()));
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
        self.comms.as_ref().map(|c| !c.has_paniced()).unwrap_or(false)
    }
}