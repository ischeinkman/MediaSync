#![allow(clippy::useless_format)]

mod communication;
mod events;
use events::{RemoteEvent};
mod players;
mod traits;

use communication::FriendCodeV4;
mod state;
use state::*;

use std::path::PathBuf;

use std::env;
use std::io;
use std::net::SocketAddr;
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
        assert!(state.comms_thread_up());
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
