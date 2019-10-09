#![allow(clippy::useless_format)]

mod communication;
use communication::{
    ConnectionsThreadHandle, ConnnectionThread, FileTransferClient, FileTransferHost,
};
mod events;
use events::{PlayerEvent, RemoteEvent};
mod players;
mod traits;
use traits::MediaPlayer;

use communication::FriendCodeV4;

use std::path::PathBuf;

use std::env;
use std::io;
use std::net::SocketAddr;
use std::sync::{self, Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};
type MyResult<T> = Result<T, Box<dyn std::error::Error>>;
pub trait DebugError: Sized {
    fn message(self) -> String;
    fn into_myerror(self) -> Box<dyn std::error::Error> {
        self.message().into()
    }
}

pub fn main() {
    let mut state = AppState::new();
    println!("Welcome to Ilan's MediaSync!");
    println!("Now starting communication thread...");
    state.start_comms().unwrap();
    println!("Communication thread started!");
    let local_friend_code = state.local_friendcode_str();
    println!("Local network friend code: {}", local_friend_code);

    println!("Now searching for available players...");
    let options_vec: Vec<(_, _)> = state.search_players().unwrap();
    println!("Found {} total players.", options_vec.len());

    let digits = (options_vec.len() as f64).log10().ceil() as usize;

    println!("Player List: ");
    for (idx, (list, player)) in options_vec.iter().enumerate() {
        println!(
            "  [{:0digits$}]: {} (List : {})",
            idx,
            player,
            list,
            digits = digits
        );
    }

    let (selection_list, selection_name) = loop {
        println!(
            "Which player should be used? Enter the number that appeared between the [ and the ]."
        );
        let mut raw_input = String::new();
        let _read = io::stdin().read_line(&mut raw_input).unwrap();
        let raw_input = raw_input.trim();
        let parse_res = raw_input.parse::<usize>().map(|idx| options_vec.get(idx));
        match parse_res {
            Ok(Some((list, name))) => {
                break (list, name);
            }
            Ok(None) => {
                println!("Selection {} does not exist. Please try again.", raw_input);
            }
            Err(e) => {
                println!("Selection {} is not valid:", raw_input);
                println!("   {:?}", e);
                println!("Please try again.");
            }
        }
    };
    println!("Now connecting to player {}", selection_name);
    state.open_player(selection_list, selection_name).unwrap();
    crate::debug_print(format!(
        "Connected to player {} - {}",
        selection_list, selection_name
    ));
    println!("Connection successful!");

    let args = Args::parse_argv().unwrap();
    if args.open_public {
        println!("Trying to open public IP.");
        state.open_public().unwrap();
        while state.public_friendcode_str() == "NONE" {}
        println!("Success! Public code: {}", state.public_friendcode_str());
    }

    if let Some(remote) = args.connect_to {
        println!(
            "Trying to connect to {}",
            remote.as_friend_code().iter().collect::<String>()
        );
        state.open_connection(remote.as_addr().into()).unwrap();
        println!("Success!");
    }

    let mut prev_connection_count = 0;
    loop {
        assert!(!state
            .comms
            .as_ref()
            .map(|t| t.has_paniced())
            .unwrap_or(false));
        let pending_event = state.pop_event();
        match pending_event {
            Some(RemoteEvent::MediaOpen(url)) => {
                if !url.starts_with("file:") {
                    state.media_open_okay(url.clone()).unwrap();
                } else {
                    println!("Other player has opened file {}.", url);
                    let response = loop {
                        println!("How should we proceed? Options are:");
                        println!("  [t] : request a transfer");
                        println!("  [l] : open a local file");
                        println!("  [i] : ignore");
                        let mut raw_input = String::new();
                        let _read = io::stdin().read_line(&mut raw_input).unwrap();
                        let input = raw_input.trim().to_lowercase();
                        if input.starts_with('t') {
                            break 't';
                        } else if input.starts_with('l') {
                            break 'l';
                        } else if input.starts_with('i') {
                            break 'i';
                        } else {
                            println!("Error: invalid selection {}.", raw_input);
                        }
                    };
                    match response {
                        't' => {
                            state.request_transfer(url.clone()).unwrap();
                            println!("Now waiting for a response ...");
                            let wait_start = Instant::now();
                            let response = loop {
                                if Instant::now() - wait_start > Duration::from_secs(30) {
                                    break None;
                                }
                                let found_event = state.pop_event().and_then(|evt| match evt {
                                    RemoteEvent::RespondTransfer { .. } => Some(evt),
                                    _ => None,
                                });
                                if found_event.is_some() {
                                    break found_event;
                                }
                            };
                            if let Some(RemoteEvent::RespondTransfer {
                                size,
                                transfer_code,
                            }) = response
                            {
                                if let Some(code) = transfer_code {
                                    state
                                        .start_transfer(
                                            url.clone(),
                                            size,
                                            FriendCodeV4::from_code(code),
                                        )
                                        .unwrap();
                                    let transfer =
                                        state.active_transfers().find(|t| t.url() == url).unwrap();
                                    let mut prev_percent = 0.0;
                                    while !transfer.is_finished() {
                                        let cur_percent = transfer.progress();
                                        if cur_percent - prev_percent > 10.0 {
                                            println!("Progress: {:.2}%", cur_percent);
                                            prev_percent = cur_percent;
                                        }
                                    }
                                    println!("Finished!");
                                    state.media_open_okay(url).unwrap();
                                } else {
                                    println!("Transfer request denied. Falling back to ignore.");
                                }
                            } else {
                                println!("No response found. Falling back to ignore.");
                            }
                        }
                        'l' => {
                            println!("What file should be opened?");
                            let mut path_buff = String::new();
                            let _read = io::stdin().read_line(&mut path_buff).unwrap();
                            let url = format!(
                                "file:///{}",
                                std::path::PathBuf::from(path_buff).to_str().unwrap()
                            );
                            println!("Now opening.");
                            state.media_open_okay(url).unwrap();
                            println!("Open finished.");
                        }
                        'i' => {}
                        _ => {}
                    }
                }
            }
            Some(RemoteEvent::RequestTransfer(url)) => {
                let path = PathBuf::from(url.trim_start_matches("file://"));
                if path.exists() && path.is_file() {
                    println!("Got a request to transfer file {}.", url);
                    println!("Should we perform the transfer? [y/n]");
                    let mut resp_raw = String::new();
                    io::stdin().read_line(&mut resp_raw).unwrap();
                    let resp = resp_raw.to_lowercase().trim().starts_with('y');
                    if resp {
                        println!("Opening transfer port.");
                        state.open_transfer_host(url.clone()).unwrap();
                        let transfer_host = state.get_transfer_host(&url).unwrap();
                        let transfer_host_code = transfer_host.transfer_code().unwrap();
                        println!(
                            "Transfer server opened using code {}.",
                            transfer_host_code
                                .as_friend_code()
                                .iter()
                                .collect::<String>()
                        );
                        println!("Informing peers.");
                        state.broadcast_transfer_host(&url).unwrap();
                        println!("Peers informed.");
                    }
                }
            }
            _ => {}
        }
        let connections = state.connections().unwrap();
        if connections.len() != prev_connection_count {
            println!("Connections: ");
            for con in &connections {
                match con {
                    SocketAddr::V4(addr) => {
                        println!("  {:?}", addr);
                    }
                    SocketAddr::V6(addr) => {
                        println!("  {:?}", addr);
                    }
                }
            }
            prev_connection_count = connections.len();
        }
        thread::sleep(Duration::from_millis(100));
        thread::yield_now();
    }
}

pub struct Args {
    open_public: bool,
    connect_to: Option<FriendCodeV4>,
}

impl Default for Args {
    fn default() -> Args {
        Args {
            open_public: false,
            connect_to: None,
        }
    }
}

impl Args {
    pub fn parse_argv() -> MyResult<Args> {
        let mut retvl = Args::default();
        let mut argiter = env::args();
        while let Some(arg) = argiter.next() {
            match arg.as_ref() {
                "-p" | "--public" => {
                    retvl.open_public = true;
                }
                "-c" | "--connect" => {
                    let arg = argiter
                        .next()
                        .ok_or_else(|| format!("Error: could not find arg for flag {}.", arg))?;
                    let arg = arg.trim();
                    if arg.len() != 9 {
                        return Err(format!("Error: code {} is not valid.", arg).into());
                    }
                    let code_bytes = [
                        arg.chars().nth(0).unwrap(),
                        arg.chars().nth(1).unwrap(),
                        arg.chars().nth(2).unwrap(),
                        arg.chars().nth(3).unwrap(),
                        arg.chars().nth(4).unwrap(),
                        arg.chars().nth(5).unwrap(),
                        arg.chars().nth(6).unwrap(),
                        arg.chars().nth(7).unwrap(),
                        arg.chars().nth(8).unwrap(),
                    ];
                    let code = FriendCodeV4::from_code(code_bytes);
                    retvl.connect_to = Some(code);
                }
                _ => {}
            }
        }
        Ok(retvl)
    }
}

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
                }
                else {
                    false
                }
            }
            else {true};
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
                        let needs_transition = if let MediaOpenState::Okay(ref old_url) = *state_ref.read().unwrap() {
                            old_url != &Some(url.clone())
                        } else {false};
                        if needs_transition {
                            player_lock.send_event(PlayerEvent::Pause).unwrap();
                            *state_ref.write().unwrap() =
                                MediaOpenState::Requesting(url.to_owned());
                            callback.send(evt).unwrap();
                        }
                        else {
                            comms_ref.send_event(RemoteEvent::MediaOpenOkay(url.to_owned())).unwrap();
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
            crate::debug_print(format!("LOCAL: state transition: Broadcasting({}) => Transfering({})", url, url));
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
            let local_path: &std::path::Path = self
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
            crate::debug_print(format!("LOCAL: state transition: Requesting({}) => Okay({})", url, url));
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
}

pub fn debug_print(msg: String) {
    use std::io::Write;
    let mut fl = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("ilanlog.txt")
        .unwrap();
    writeln!(
        &mut fl,
        "{}: {}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        msg
    )
    .unwrap();
}
