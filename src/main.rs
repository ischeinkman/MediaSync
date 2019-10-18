#![allow(clippy::useless_format)]

mod communication;
mod events;
use events::RemoteEvent;
mod players;
mod traits;

use communication::FriendCode;
mod state;
use state::*;

use std::io;
use std::net::SocketAddr;
use std::thread;
use std::time::{Duration, Instant};

mod clapui;
use url::Url;

type MyResult<T> = Result<T, Box<dyn std::error::Error>>;

pub trait DebugError: Sized {
    fn message(self) -> String;
    fn into_myerror(self) -> Box<dyn std::error::Error> {
        self.message().into()
    }
}

fn simpleui_init() -> AppState {
    let mut state = AppState::new();
    println!("Welcome to Ilan's MediaSync!");
    println!("Now starting communication thread...");
    state.start_comms().unwrap();
    println!("Communication thread started!");
    let local_friend_code = state.local_friendcode_str();
    println!("Local network friend code: {}", local_friend_code);
    state
}
fn simpleui_show_player_list(args: &Args, state: &mut AppState) -> Vec<(String, String)> {
    println!("Now searching for available players...");
    let all_players = state.search_players().unwrap();
    let options_vec = if args.show_debug {
        all_players
    } else {
        all_players
            .into_iter()
            .filter(|(list, _)| list != "Debug")
            .collect()
    };
    if options_vec.is_empty() {
        println!("Found no players. Exiting.");
        return Vec::new();
    }
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
    options_vec
}

fn simpleui_select_player(_args: &Args, state: &mut AppState, options_vec: &[(String, String)]) {
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
    println!("Connection successful!");
}

fn simpleui_on_remote_open(url: &str) -> OnTransferAction {
    println!("Other player has opened file {}.", url);
    loop {
        println!("How should we proceed? Options are:");
        println!("  [t] : request a transfer");
        println!("  [l] : open a local file");
        println!("  [i] : ignore");
        let mut raw_input = String::new();
        let _read = io::stdin().read_line(&mut raw_input).unwrap();
        let input = raw_input.trim().to_lowercase();
        if input.starts_with('t') {
            break OnTransferAction::RequestTransfer;
        } else if input.starts_with('l') {
            break OnTransferAction::OpenLocal;
        } else if input.starts_with('i') {
            break OnTransferAction::Ignore;
        } else {
            println!("Error: invalid selection {}.", raw_input);
        }
    }
}

fn simpleui_on_remote_requests_transfer(state: &mut AppState, url: &str) -> MyResult<()> {
    let path = Url::parse(url)?
        .to_file_path()
        .map_err(|_| format!("Error: could not parse {:?}", url))?;
    if path.exists() && path.is_file() {
        println!("Got a request to transfer file {}.", url);
        println!("Should we perform the transfer? [y/n]");
        let mut resp_raw = String::new();
        io::stdin().read_line(&mut resp_raw)?;
        let resp = resp_raw.to_lowercase().trim().starts_with('y');
        if resp {
            println!("Opening transfer port.");
            state.open_transfer_host(url.to_owned())?;
            let transfer_host = state.get_transfer_host(&url).unwrap();
            let transfer_host_code = transfer_host.transfer_code();
            println!(
                "Transfer server opened using code {}.",
                transfer_host_code.as_friend_code()
            );
            println!("Informing peers.");
            state.broadcast_transfer_host(&url)?;
            println!("Peers informed.");
        }
    } else {
        return Err(format!(
            "Could not find path {:?}! Parsed it from URL {:?}.",
            path, url
        )
        .into());
    }
    Ok(())
}

fn simpleui_request_transfer(
    state: &mut AppState,
    url: &str,
    timeout: Duration,
) -> MyResult<Option<(u64, [char; 9])>> {
    state.request_transfer(url.to_owned())?;
    println!("Now waiting for a response ...");
    let wait_start = Instant::now();
    let response = 'resp: loop {
        if Instant::now() - wait_start > timeout {
            break None;
        }
        while let Some(evt) = state.pop_event() {
            if let RemoteEvent::RespondTransfer {
                size,
                transfer_code,
            } = evt
            {
                break 'resp transfer_code.map(|c| (size, c));
            } else {
            }
        }
        thread::yield_now();
    };
    Ok(response)
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
enum OnTransferAction {
    RequestTransfer,
    OpenLocal,
    Ignore,
}

pub fn main() {
    let args = Args::parse_argv();
    let mut state = simpleui_init();

    let options_vec = simpleui_show_player_list(&args, &mut state);
    if options_vec.is_empty() {
        return;
    }
    simpleui_select_player(&args, &mut state, &options_vec);

    if args.open_public {
        println!("Trying to open public IP.");
        state.open_public().unwrap();
        while state.public_friendcode_str() == "NONE" {}
        println!("Success! Public code: {}", state.public_friendcode_str());
    }

    for remote in &args.connect_to {
        println!("Trying to connect to {}", remote.as_friend_code());
        state.open_connection(remote.as_addr()).unwrap();
        println!("Success!");
    }

    let mut prev_connection_count = 0;
    loop {
        while let Some(evt) = state.pop_event() {
            match evt {
                RemoteEvent::MediaOpen(url) => {
                    if !url.starts_with("file:") {
                        state.media_open_okay(url.clone()).unwrap();
                        continue;
                    }
                    let response = simpleui_on_remote_open(&url);
                    match response {
                        OnTransferAction::RequestTransfer => {
                            let response = simpleui_request_transfer(
                                &mut state,
                                &url,
                                Duration::from_secs(60),
                            )
                            .unwrap();
                            if let Some((size, code)) = response {
                                let code = FriendCode::from_code_v4(code);
                                println!("Now starting transfer.");
                                state.start_transfer(url.clone(), size, code).unwrap();
                                let transfer =
                                    state.active_transfers().find(|t| t.url() == url).unwrap();
                                let mut prev_percent = 0.0;
                                while !transfer.is_finished() {
                                    let cur_percent = transfer.progress();
                                    if cur_percent - prev_percent > 0.10 {
                                        println!("Progress: {:.2}%", 100.0 * cur_percent);
                                        prev_percent = cur_percent;
                                    }
                                }
                                println!("Finished!");
                                state.media_open_okay(url).unwrap();
                            } else {
                                println!("No response found. Falling back to ignore.");
                            }
                        }
                        OnTransferAction::OpenLocal => {
                            println!("What file should be opened?");
                            let mut path_buff = String::new();
                            let _read = io::stdin().read_line(&mut path_buff).unwrap();
                            let url = Url::from_file_path(&path_buff)
                                .map_err(|_| {
                                    format!("Could not format url from path {:?}", path_buff)
                                })
                                .unwrap();
                            println!("Now opening.");
                            state.media_open_okay(url.into_string()).unwrap();
                            println!("Open finished.");
                        }
                        OnTransferAction::Ignore => {}
                    }
                }
                RemoteEvent::RequestTransfer(url) => {
                    simpleui_on_remote_requests_transfer(&mut state, &url).unwrap();
                }
                _ => {}
            }
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
        thread::yield_now();
    }
}

pub struct Args {
    show_debug: bool,
    open_public: bool,
    connect_to: Vec<FriendCode>,
    mpv_socket: Vec<String>,
}

impl Default for Args {
    fn default() -> Args {
        Args {
            show_debug: false,
            open_public: false,
            connect_to: Vec::new(),
            mpv_socket: Vec::new(),
        }
    }
}

impl Args {
    pub fn parse_argv() -> Args {
        let clap_args = clapui::init_parser();
        clapui::map_args(clap_args.get_matches())
    }
}
