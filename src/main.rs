mod communication;
mod mprisplayer;

use azul::prelude::*;
use azul::widgets::{label::Label, text_input::*};

use std::thread;
use std::time::Duration;
use std::env;
use std::net::{SocketAddr, SocketAddrV4};
use std::collections::HashMap;

type MyResult<T> = Result<T, Box<dyn std::error::Error>>;

pub struct MyApp {
    player: Option<mprisplayer::MprisPlayer<'static>>,
    comms: Option<communication::Communicator>,
    local_event_thread: Option<thread::JoinHandle<()>>,
    code_input: TextInputState,
}

impl MyApp {
    pub fn init(&mut self) -> MyResult<()> {
        if self.player.is_none() {
            let player =
                mprisplayer::MprisPlayer::find_open()?.ok_or_else(|| "TODO:msg".to_owned())?;
            self.player = Some(player);
        }
        if self.comms.is_none() {
            let comms = communication::Communicator::open(40000, 41000)?;
            self.comms = Some(comms);
        }
        Ok(())
    }
    pub fn start_local_event_thread(&mut self) -> MyResult<()> {
        let mut player = self
            .player
            .as_ref()
            .ok_or_else(|| "Error: starting thread without player".to_owned())?
            .handle();
        let comms = self
            .comms
            .as_ref()
            .ok_or_else(|| "Error: starting thread without comms".to_owned())?
            .handle();
        let handle = thread::spawn(move || {
            let interval_goal = Duration::from_millis(20);
            let mut prev_iteration = std::time::Instant::now();
            loop {
                let local_events = player
                    .check_events(std::time::Instant::now() - prev_iteration, true)
                    .unwrap();
                for local_evt in local_events.into_iter() {
                    //println!("Local event: {:?}", local_evt.clone());
                    comms.send_message(local_evt).unwrap();
                }

                let comms_events = comms.check_message().unwrap();
                for remote_evt in comms_events.into_iter() {
                    player.on_message(remote_evt).unwrap();
                }

                prev_iteration = std::time::Instant::now();
                std::thread::sleep(interval_goal);
            }
        });
        self.local_event_thread = Some(handle);
        Ok(())
    }
}

impl Default for MyApp {
    fn default() -> Self {
        Self {
            player: None,
            comms: None,
            local_event_thread: None,
            code_input: TextInputState::new(""),
        }
    }
}

const CSS: &str = "
    #root {
        width : 100%;
        height : 100%;
        flex-direction : column;
        justify-content : center; 
        align-items : center;
    }
    #code {
        background-color : cyan;
        border-radius : 5px;
        margin : 5px;
        font-size : 18px;
    }
";

impl Layout for MyApp {
    fn layout(&self, _: LayoutInfo<Self>) -> Dom<Self> {
        let mut retvl = Dom::div()
            .with_id("root")
            .with_child(Label::new("Ilan's Player Syncer").dom());

        if let Some(player) = &self.player {
            retvl.add_child(
                Label::new(format!("Player: {}", player.name()))
                    .dom()
                    .with_id("playername"),
            );
        }
        if let Some(comms) = &self.comms {
            let encoded_addr =
                communication::encode_ipv4(u32::from_be_bytes(comms.public_ip.ip().octets()));
            let encoded_port = communication::encode_port(comms.public_ip.port());
            let encoded_ip_port: String = encoded_addr.iter().chain(encoded_port.iter()).collect();
            retvl.add_child(
                Label::new(format!("Code: {}", encoded_ip_port))
                    .dom()
                    .with_id("code"),
            );
        }
        retvl
    }
}

#[allow(unused)]
fn azul_main() {
    let mut myapp = MyApp::default();
    println!("MyApp empty.");
    myapp.init().unwrap();
    println!("MyApp initted.");
    myapp.start_local_event_thread().unwrap();
    println!("MyApp local_event_thread initted.");
    let mut app = App::new(
        myapp,
        AppConfig {
            //enable_logging: Some(LevelFilter::Debug),
            renderer_type: RendererType::Hardware,
            debug_state: DebugState {
                //profiler_dbg: true,
                //compact_profiler: true,
                //echo_driver_messages: true,

                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    println!("App created.");
    let window = app
        .create_window(
            WindowCreateOptions {
                state: WindowState {
                    ..Default::default()
                },
                ..Default::default()
            },
            css::override_native(CSS).unwrap(),
        )
        .unwrap();
    println!("Window created.");
    app.run(window).unwrap();
    println!("App run finished..");
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum ProtocolMessage {
    TimePing(Duration),
    Pause(Duration),
    Play(Duration),
    Jump(Duration),
    MediaChange(String),
    Shutdown,
}

pub trait DebugError: Sized {
    fn message(self) -> String;
    fn into_myerror(self) -> Box<dyn std::error::Error> {
        self.message().into()
    }
}

pub trait MediaPlayer {
    fn name(&self) -> &str;
    fn on_message(&mut self, message: ProtocolMessage) -> MyResult<()>;
}

pub fn main() {
    let args = Args::parse(env::args(), vec!["--connect".to_owned(), "-c".to_owned()], vec![]).unwrap();
    println!("Now starting Ilan's Media Sync...");
    let mut app = MyApp::default();
    app.init().unwrap();
    println!("Initialized successfully.");
    println!("Using player: {}", app.player.as_ref().map(|p| p.name().to_owned()).unwrap_or_else(|| "NONE".to_owned()));
    let friend_code = app.comms.as_ref().map(|c| communication::encode_socketaddrv4(c.public_ip).iter().collect()).unwrap_or_else(|| "NONE".to_owned());
    println!("Friend code: {}", friend_code);
    if args.key_value.contains_key(&"-c".to_owned()) || args.key_value.contains_key(&"--connect".to_owned()) {
        let remote :&str = args.key_value.get(&"-c".to_owned()).or_else(|| args.key_value.get(&"--connect".to_owned())).ok_or_else(||"Error finding argument to --connect!".to_owned()).unwrap();
        let remote = remote.trim();
        if remote.len() != 9 {
            println!("Error: {} is not a valid friend code!", remote);
        }
        println!("Trying to connect to friend using code {}", remote);
        let mut chars_iter = remote.chars();
        let addr_chars = [
            chars_iter.next().ok_or_else(|| "Error getting friend's code char at index 0".to_owned()).unwrap(),
            chars_iter.next().ok_or_else(|| "Error getting friend's code char at index 1".to_owned()).unwrap(),
            chars_iter.next().ok_or_else(|| "Error getting friend's code char at index 2".to_owned()).unwrap(),
            chars_iter.next().ok_or_else(|| "Error getting friend's code char at index 3".to_owned()).unwrap(),
            chars_iter.next().ok_or_else(|| "Error getting friend's code char at index 4".to_owned()).unwrap(),
            chars_iter.next().ok_or_else(|| "Error getting friend's code char at index 5".to_owned()).unwrap(),
            chars_iter.next().ok_or_else(|| "Error getting friend's code char at index 6".to_owned()).unwrap(),
            chars_iter.next().ok_or_else(|| "Error getting friend's code char at index 7".to_owned()).unwrap(),
            chars_iter.next().ok_or_else(|| "Error getting friend's code char at index 8".to_owned()).unwrap(),
        ];
        let remote_addr : SocketAddrV4 = communication::decode_socketaddrv4(addr_chars);
        println!("Found remote address {} : {}", *remote_addr.ip(), remote_addr.port());
        println!("Trying to connect...");
        let comms = app.comms.as_mut().ok_or_else(||("Error: comms did not init successfully before trying connection?".to_owned())).unwrap();
        comms.open_remote(SocketAddr::V4(remote_addr)).unwrap();
        println!("Connected!");
    }
    println!("Starting local event thread...");
    app.start_local_event_thread().unwrap();
    println!("Success! Everything should hopefully be in working order.");
    let mut prev_clients = 0;
    let mut prev_servers = 0;
    loop {

    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct Args {
    key_value : HashMap<String, String>, 
    flags : Vec<String>, 
    nameless : Vec<String>,
}

impl Args {
    pub fn parse(iter : impl IntoIterator<Item = impl AsRef<str>>, valid_keys : Vec<String>, valid_flags : Vec<String>) -> MyResult<Self> {
        let mut retvl = Args::default();
        let mut arg_iter = iter.into_iter();
        while let Some(arg) = arg_iter.next() {
            let arg = arg.as_ref().to_owned();
            if !arg.starts_with('-') {
                retvl.nameless.push(arg);
            }
            else if valid_flags.contains(&arg) {
                retvl.flags.push(arg);
            }
            else if valid_keys.contains(&arg) {
                let value = arg_iter.next().ok_or_else(||format!("Error parsing args: found now value for key {}", arg))?.as_ref().to_owned();
                retvl.key_value.insert(arg, value);
            }
            else if arg.contains('=') {
                let mut kv_iter = arg.splitn(2, '=');
                let key = kv_iter.next().ok_or_else(|| format!("Error parsing arg {}: somehow can't split in the split branch? Dafuq?", arg))?.to_owned();
                let value = kv_iter.next().ok_or_else(|| format!("Error parsing arg {}: somehow can't split in the split branch? Dafuq?", arg))?.to_owned();
                if valid_keys.contains(&key) {
                    retvl.key_value.insert(key, value);
                }
                else {
                    return Err(format!("Error: found invalid key-value pair: '{}' => '{}'", key, value).into());
                }
            }

        }

        Ok(retvl)
    }
}