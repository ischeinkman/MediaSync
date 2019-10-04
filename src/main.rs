mod communication;
mod mprisplayer;

use azul::prelude::*;
use azul::widgets::{label::Label, text_input::*};

use std::thread;
use std::time::Duration;

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
            .with_child(Label::new("VLC Sync").dom());

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

fn main() {
    let mut myapp = MyApp::default();
    println!("MyApp empty.");
    myapp.init().unwrap();
    println!("MyApp initted.");
    myapp.start_local_event_thread().unwrap();
    println!("MyApp local_event_thread initted.");
    let mut app = App::new(
        myapp,
        AppConfig {
            enable_logging: Some(LevelFilter::Debug),
            renderer_type: RendererType::Hardware,
            debug_state: DebugState {
                profiler_dbg: true,
                compact_profiler: true,
                echo_driver_messages: true,

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
