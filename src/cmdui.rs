use crate::config::SyncConfig;
use crate::network::{friendcodes::FriendCode, friendcodes::FriendCodeError, NetworkManager};
use crate::players::BulkSyncPlayerList;
use crate::traits::{SyncPlayer, SyncPlayerList, SyncPlayerWrapper};
use crate::{local_broadcast_task, remote_sink_task};
use clap::{App, Arg};
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::logging::{LogSinkConfig, LogSinkWrapper, StderrLogSink, StdoutLogSink};

async fn simpleui_select_player<'a>(
    options_vec: &mut Vec<(String, Box<dyn SyncPlayer + 'a>)>,
) -> std::io::Result<(String, SyncPlayerWrapper<Box<dyn SyncPlayer + 'a>>)> {
    let retvl = loop {
        println!(
            "Which player should be used? Enter the number that appeared between the [ and the ]."
        );
        let mut raw_input = String::new();
        let _read = std::io::stdin().read_line(&mut raw_input)?;
        let raw_input = raw_input.trim();
        let parse_res = raw_input.parse::<usize>().map(|idx| {
            if idx < options_vec.len() {
                Some(options_vec.remove(idx))
            } else {
                None
            }
        });
        let config = SyncConfig::new();
        match parse_res {
            Ok(Some((name, player))) => {
                break (name, SyncPlayerWrapper::new(player, config).await.unwrap());
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
    Ok(retvl)
}
pub fn init_parser<'a, 'b>() -> App<'a, 'b> {
    App::new("vlcsync")
        .version("test")
        .arg(
            Arg::with_name("public")
                .long("public")
                .short("p")
                .multiple(false)
                .takes_value(false)
                .help("Try to open a public port."),
        )
        .arg(
            Arg::with_name("connections")
                .value_name("friend code")
                .short("c")
                .long("connect")
                .takes_value(true)
                .multiple(true)
                .validator(|s| {
                    let s = s.trim();
                    if s.len() != 9 {
                        return Err(format!(
                            "Friend codes have length {} but value {} has length {}",
                            9,
                            s,
                            s.len()
                        ));
                    }
                    for c in s.chars() {
                        if !c.is_alphanumeric() {
                            return Err(format!("Invalid char {}.", c));
                        }
                    }
                    Ok(())
                })
                .help("Connect to a given friend code on start."),
        )
}

pub async fn run() -> crate::DynResult<()> {
    let logger = LogSinkWrapper::get_handle();
    logger.add_logger(StdoutLogSink::new(), LogSinkConfig::new().enable_all());
    logger.add_logger(
        StderrLogSink::new(),
        LogSinkConfig::new().with_filter(log::LevelFilter::Warn),
    );
    let arg_parser = init_parser();
    let args = arg_parser.get_matches();

    println!("Welcome to Ilan's MediaSync!");
    println!("Now starting communication thread...");
    let network_manager = NetworkManager::new_random_port(10000, 30000).await?;
    println!("Communication thread started!");
    println!(
        "Local network friend code: {}",
        FriendCode::from_addr(network_manager.local_addr()).as_friend_code()
    );
    println!("Now searching for available players...");
    let mut player_finder = BulkSyncPlayerList::new()?;
    let mut player_list = player_finder.get_players()?;
    if player_list.is_empty() {
        println!("Found no players. Exiting.");
        return Ok(());
    }
    println!("Found {} total players.", player_list.len());

    let digits = (player_list.len() as f64).log10().ceil() as usize;

    println!("Player List: ");
    for (idx, (name, _)) in player_list.iter().enumerate() {
        println!("  [{:0digits$}]: {}", idx, name, digits = digits);
    }
    let (name, player) = simpleui_select_player(&mut player_list).await?;
    println!("Now connecting to player {}", name);
    println!("Connection successful!");

    let cons = args.values_of("connections").into_iter().flatten();
    for code in cons {
        let addr = match FriendCode::from_code(code) {
            Ok(a) => a,
            Err(FriendCodeError::InvalidLength(l)) => {
                eprintln!("ERROR: {} is not a valid friend code.", code);
                eprintln!("       Friend codes  can only be 9 (for IPv4) or 25 (for IPv6) characters long, but this code is length {}.", l);
                return Err("".into());
            }
            Err(FriendCodeError::InvalidCharacter(c)) => {
                eprintln!("ERROR: {} is not a valid friend code.", code);
                eprintln!("       Friend codes cannot contain character {}", c);
                return Err("".into());
            }
        };
        network_manager.connect_to(addr).await?;
    }
    if args.is_present("public") {
        println!("Trying to open public IP.");
        network_manager.request_public().await?;
        println!(
            "Success! Public code: {}",
            FriendCode::from_addr(network_manager.public_addr().await.unwrap()).as_friend_code()
        );
    }
    let network_manager = Arc::new(network_manager);
    let player = Arc::new(Mutex::new(player));

    let remote_stream = remote_sink_task(Arc::clone(&player), Arc::clone(&network_manager));

    let _remote_stream_handle = tokio::task::spawn_local(remote_stream);

    let local_stream = local_broadcast_task(Arc::clone(&player), Arc::clone(&network_manager));
    let _local_stream_handle = tokio::task::spawn_local(local_stream);

    let connection_updator = network_manager
        .new_connections()
        .for_each(|res| async move {
            let naddr = res.unwrap();
            let code = FriendCode::from_addr(naddr);
            log::info!(
                "Got a new connection from {:?} ({})",
                code.as_friend_code(),
                naddr
            );
        });
    let _connection_updator_handle = tokio::spawn(connection_updator);

    Ok(())
}
