use crate::network::friendcodes::FriendCode;
use crate::network::utils::random_listener;
use crate::network::NetworkManager;
use crate::players::BulkSyncPlayerList;
use crate::traits::sync::{SyncConfig, SyncPlayer, SyncPlayerList, SyncPlayerWrapper};
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JSValue;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio::sync::watch;
use tokio::sync::Mutex;
use web_view::WebView;
mod logger;

use logger::WebLogger;
const WEBPAGE: &str = include_str!("../static/webui.html");

pub struct WebuiState {
    connections: Vec<SocketAddr>,
    player_select_callback: Option<broadcast::Sender<usize>>,
}

impl WebuiState {
    pub fn new() -> Self {
        Self {
            connections: Vec::new(),
            player_select_callback: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "cmd")]
pub enum Command {
    OpenPublic {},
    SelectPlayer { idx: usize },
    AddConnection { code: String },
}

async fn webview_update_task<'a, T>(view: web_view::WebView<'a, T>) {
    const FPS: u64 = 1000;
    const NANOS_PER_SECOND: u64 = 1_000_000_000;
    const NANOS_PER_FRAME: u64 = NANOS_PER_SECOND / FPS;
    let throttle_time = Duration::from_nanos(NANOS_PER_FRAME);
    println!("Time per frame: {}", throttle_time.as_secs_f64());
    let step_loop = futures::stream::unfold(view, |mut webview| {
        let res = webview.step();
        let res = match res {
            Some(Ok(_)) => Some(((), webview)),
            Some(e) => Some((e.unwrap(), webview)),
            None => None,
        };
        async { res }
    });
    let throttled = tokio::time::throttle(throttle_time, step_loop);
    throttled
        .map(Ok)
        .forward(futures::sink::drain())
        .await
        .unwrap();
}

async fn update_local_info<T>(network_ref: &Arc<NetworkManager>, handle: web_view::Handle<T>) {
    let local_addr = network_ref.local_addr();
    let local_code = FriendCode::from_addr(local_addr).as_friend_code();

    let mut info_map = JsArgs::new().with_str("local_code", local_code);
    if let Some(public_addr) = network_ref.public_addr().await {
        let public_code = FriendCode::from_addr(public_addr).as_friend_code();
        info_map = info_map.with_str("public_code", public_code);
    }
    handle
        .dispatch(move |view| {
            let cmd = format!(
                "frontend_interface.set_local_con_info({})",
                info_map.to_jsobject().to_string()
            );
            view.eval(&cmd)
        })
        .unwrap();
}

async fn on_add_connection<T>(
    network_ref: Arc<NetworkManager>,
    _handle: web_view::Handle<T>,
    code: String,
) {
    let parsed = match FriendCode::from_code(code) {
        Ok(p) => p,
        Err(_e) => todo!(),
    };
    let connection = match TcpStream::connect(parsed.as_addr()).await {
        Ok(c) => c,
        Err(_e) => todo!(),
    };
    network_ref.add_connection(connection).await.unwrap();
}

pub struct JsArgs {
    fields: serde_json::Map<String, JSValue>,
}
impl JsArgs {
    pub fn new() -> Self {
        Self {
            fields: serde_json::Map::new(),
        }
    }
    pub fn with_str(mut self, name: &str, value: String) -> Self {
        let owned_name = name.to_owned();
        self.fields.insert(owned_name, JSValue::String(value));
        self
    }

    pub fn to_jsobject(self) -> JSValue {
        JSValue::Object(self.fields)
    }
}

fn update_remote_connections(view: &mut web_view::WebView<WebuiState>) -> web_view::WVResult {
    let coniter = view.user_data().connections.iter().copied();
    let codes = coniter
        .map(FriendCode::from_addr)
        .map(|code| {
            JsArgs::new()
                .with_str("friend_code", code.as_friend_code())
                .to_jsobject()
        })
        .collect::<JSValue>();
    let cmd = format!("frontend_interface.set_connections({})", codes.to_string());
    view.eval(&cmd)
}

fn setup_logger(handle: &web_view::Handle<WebuiState>) {
    let href = handle.clone();
    log::info!("Setting logger.");
    log::set_boxed_logger(Box::from(WebLogger::<WebuiState>::from(href))).unwrap();
    log::set_max_level(log::LevelFilter::max());
    log::info!("Logger set.");
}

fn setup_con_updator(handle: &web_view::Handle<WebuiState>, netref: &Arc<NetworkManager>) {
    let href = handle.clone();
    let connection_updator = netref
        .new_connections()
        .for_each_concurrent(None, move |res| {
            let naddr = res.unwrap();
            href.dispatch(move |view| {
                if !view.user_data().connections.contains(&naddr) {
                    view.user_data_mut().connections.push(naddr);
                }
                update_remote_connections(view)
            })
            .unwrap();
            async {}
        });
    tokio::spawn(connection_updator);
    log::info!("Connection updator made.");
}

async fn select_player(
    handle: &web_view::Handle<WebuiState>,
    local_tasks: &tokio::task::LocalSet,
) -> Box<dyn SyncPlayer> {
    let mut player_list = BulkSyncPlayerList::new().unwrap();
    let mut players = player_list.get_players().unwrap();
    let player_list_arg = players
        .iter()
        .map(|(name, _)| {
            JsArgs::new()
                .with_str("name", name.to_owned())
                .to_jsobject()
        })
        .collect::<JSValue>();
    log::info!("Player list collected.");
    let (player_select_snd, mut player_select_recv) = broadcast::channel(1);
    handle
        .dispatch(move |view| {
            view.user_data_mut().player_select_callback = Some(player_select_snd);
            let cmd = format!(
                "frontend_interface.open_player_list({})",
                player_list_arg.to_string()
            );
            view.eval(&cmd)
        })
        .unwrap();
    log::info!("Player list command sent.");
    let player_idx = local_tasks
        .run_until(async {
            loop {
                match player_select_recv.next().await {
                    Some(Ok(idx)) => {
                        break idx;
                    }
                    Some(Err(tokio::sync::broadcast::RecvError::Lagged(_))) => {
                        continue;
                    }
                    _a => todo!(),
                }
            }
        })
        .await;
    log::info!("Player selected.");
    handle
        .dispatch(|view| view.eval("frontend_interface.close_player_list()"))
        .unwrap();
    log::info!("Player list close sent.");
    players.remove(player_idx).1
}

pub async fn run() -> crate::DynResult<()> {
    let local_tasks = tokio::task::LocalSet::new();

    log::info!("Start");
    let network_manager = {
        let listener = random_listener(10000, 30000).await?;
        Arc::new(NetworkManager::new(listener).unwrap())
    };
    log::info!("Net built.");

    let handle = build_webview(&local_tasks, &network_manager).await;
    log::info!("UI task built.");
    setup_logger(&handle);
    setup_con_updator(&handle, &network_manager);

    let player = {
        let raw_player = select_player(&handle, &local_tasks).await;
        let wrapped_player = SyncPlayerWrapper::new(raw_player, SyncConfig::new()).unwrap();
        Arc::new(Mutex::new(wrapped_player))
    };
    local_tasks
        .run_until(update_local_info(&network_manager, handle))
        .await;
    let local_fut = crate::local_broadcast_task(Arc::clone(&player), Arc::clone(&network_manager));
    let remote_fut = crate::remote_sink_task(Arc::clone(&player), Arc::clone(&network_manager));

    local_tasks.spawn_local(local_fut);
    log::info!("local_fut spawned.");
    local_tasks.spawn_local(remote_fut);
    log::info!("remote_fut spawned.");
    local_tasks.await;
    Ok(())
}

fn invoke_handler(
    network_ref: Arc<NetworkManager>,
) -> impl Fn(&mut web_view::WebView<WebuiState>, &str) -> Result<(), web_view::Error> + 'static {
    move |view, rawcmd| {
        let cmd: Command = serde_json::from_str(rawcmd).unwrap();
        let handle = view.handle();
        match cmd {
            Command::OpenPublic {} => {
                let network_ref = Arc::clone(&network_ref);
                tokio::task::spawn(async move {
                    network_ref.request_public().await.unwrap();
                    update_local_info(&network_ref, handle).await;
                });
                Ok(())
            }
            Command::SelectPlayer { idx } => {
                log::info!("Player select: {}", idx);
                if let Some(cb) = view.user_data_mut().player_select_callback.as_mut() {
                    cb.send(idx).unwrap();
                    Ok(())
                } else {
                    log::warn!("Warning: selected a player without a valid CB?");
                    Ok(())
                }
            }
            Command::AddConnection { code } => {
                tokio::task::spawn(on_add_connection(Arc::clone(&network_ref), handle, code));
                Ok(())
            }
        }
    }
}

async fn build_webview(
    task_set: &tokio::task::LocalSet,
    network_manager: &Arc<NetworkManager>,
) -> web_view::Handle<WebuiState> {
    let (mut hsnd, mut hrecv) = watch::channel(None);
    let network_ref = Arc::clone(&network_manager);
    let retfut = async move {
        let mut webview: WebView<'static, _> = web_view::builder()
            .content(web_view::Content::Html(WEBPAGE))
            .title("Ilan's VLCSync")
            .debug(true)
            .user_data(WebuiState::new())
            .invoke_handler(invoke_handler(network_ref))
            .build()
            .unwrap();
        webview.step();
        if hsnd.broadcast(Some(webview.handle())).is_err() {
            log::warn!("ERROR SENDING HANDLE!");
        } else {
            log::info!("Handle sending.");
        }
        hsnd.closed().await;
        log::info!("Yielding.");
        tokio::task::yield_now().await;
        log::info!("Yielt.");
        let ui_task = webview_update_task(webview);
        log::info!("Made ui_task");
        ui_task.await;
    };
    task_set.spawn_local(retfut);
    task_set
        .run_until(async {
            loop {
                if let Some(Some(h)) = hrecv.recv().await {
                    break h;
                }
            }
        })
        .await
}
