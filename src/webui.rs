use crate::network::friendcodes::FriendCode;
use crate::network::utils::random_listener;
use crate::network::NetworkManager;
use crate::players::BulkSyncPlayerList;
use crate::traits::sync::{SyncConfig, SyncPlayerList, SyncPlayerWrapper};
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Map as JSMap;
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

async fn webview_update_task<T>(view: web_view::WebView<'static, T>) {
    const FPS: u64 = 240;
    const NANOS_PER_FRAME: u64 = 1_000_000_000 / FPS;
    let step_loop = futures::stream::unfold(view, |mut webview| async {
        let res = webview.step();
        match res {
            Some(Ok(_)) => Some(((), webview)),
            Some(e) => Some((e.unwrap(), webview)),
            None => None,
        }
    });
    let throttled = tokio::time::throttle(Duration::from_nanos(NANOS_PER_FRAME), step_loop);
    throttled
        .map(Ok)
        .forward(futures::sink::drain())
        .await
        .unwrap();
}

async fn update_local_info<T>(network_ref: Arc<NetworkManager>, handle: web_view::Handle<T>) {
    let local_addr = network_ref.local_addr();
    let local_code = FriendCode::from_addr(local_addr).as_friend_code();

    let mut info_map = JSMap::new();
    info_map.insert("local_code".to_owned(), JSValue::String(local_code));
    if let Some(public_addr) = network_ref.public_addr().await {
        let public_code = FriendCode::from_addr(public_addr).as_friend_code();
        info_map.insert("public_code".to_owned(), JSValue::String(public_code));
    }
    let info_obj = JSValue::Object(info_map);
    handle
        .dispatch(move |view| {
            let cmd = format!(
                "frontend_interface.set_local_con_info({})",
                info_obj.to_string()
            );
            view.eval(&cmd)
        })
        .unwrap();
}

async fn on_add_connection<T>(
    network_ref: Arc<NetworkManager>,
    handle: web_view::Handle<T>,
    code: String,
) {
    let parsed = match FriendCode::from_code(code) {
        Ok(p) => p,
        Err(e) => todo!(),
    };
    let connection = match TcpStream::connect(parsed.as_addr()).await {
        Ok(c) => c,
        Err(e) => todo!(),
    };
    network_ref.add_connection(connection).await.unwrap();
}

fn update_remote_connections(view: &mut web_view::WebView<WebuiState>) -> web_view::WVResult {
    let codes = view
        .user_data()
        .connections
        .iter()
        .copied()
        .map(FriendCode::from_addr)
        .map(|code| {
            let mut objmap = JSMap::new();
            objmap.insert(
                "friend_code".to_owned(),
                JSValue::String(code.as_friend_code()),
            );
            JSValue::Object(objmap)
        })
        .collect::<JSValue>();
    let cmd = format!("frontend_interface.set_connections({})", codes.to_string());
    view.eval(&cmd)
}

pub async fn run() -> crate::DynResult<()> {
    log::info!("Start");
    let listener = random_listener(10000, 30000).await?;
    let network_manager = Arc::new(NetworkManager::new(listener).unwrap());
    log::info!("Net built.");
    let (ui_task, mut hrecv) = build_webview(Arc::clone(&network_manager));
    log::info!("UI task built.");
    let handle = loop {
        if let Some(Some(h)) = hrecv.recv().await {
            break h;
        }
    };
    drop(hrecv);
    let href = handle.clone();
    log::info!("Setting logger.");
    log::set_boxed_logger(Box::from(WebLogger::<WebuiState>::from(href))).unwrap();
    log::set_max_level(log::LevelFilter::max());
    log::info!("Logger set.");
    let href = handle.clone();
    let connection_updator = network_manager.new_connections().for_each_concurrent(None, move |res| {
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
    let connection_updator_handle = tokio::spawn(connection_updator);
    log::info!("Connection updator made.");

    let mut player_list = BulkSyncPlayerList::new().unwrap();
    let mut players = player_list.get_players().unwrap();
    let player_list_arg = players
        .iter()
        .map(|(name, _)| {
            let mut fieldmap = JSMap::new();
            fieldmap.insert("name".to_owned(), JSValue::String(name.to_owned()));
            JSValue::Object(fieldmap)
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
    let player_idx = loop {
        match player_select_recv.next().await {
            Some(Ok(idx)) => {
                break idx;
            }
            Some(Err(tokio::sync::broadcast::RecvError::Lagged(_))) => {
                continue;
            }
            a => todo!(),
        }
    };
    log::info!("Player selected.");
    handle
        .dispatch(|view| view.eval("frontend_interface.close_player_list()"))
        .unwrap();
    update_local_info(Arc::clone(&network_manager), handle).await;
    log::info!("Player list close sent.");

    let (_pname, player) = players.remove(player_idx);
    let player = Arc::new(Mutex::new(
        SyncPlayerWrapper::new(player, SyncConfig::new()).unwrap(),
    ));
    let local_fut = crate::local_broadcast_task(Arc::clone(&player), Arc::clone(&network_manager));
    let remote_fut = crate::remote_sink_task(Arc::clone(&player), Arc::clone(&network_manager));

    let local_tasks = tokio::task::LocalSet::new();
    local_tasks.spawn_local(local_fut);
    log::info!("local_fut spawned.");
    local_tasks.spawn_local(remote_fut);
    log::info!("remote_fut spawned.");
    let ret = tokio::join!(local_tasks, ui_task, connection_updator_handle);
    ret.1.unwrap();
    ret.2.unwrap();
    Ok(())
}

fn invoke_handler(
    network_ref: Arc<NetworkManager>,
) -> impl Fn(&mut web_view::WebView<WebuiState>, &str) -> Result<(), web_view::Error> {
    move |view, rawcmd| {
        let cmd: Command = serde_json::from_str(rawcmd).unwrap();
        let handle = view.handle();
        match cmd {
            Command::OpenPublic {} => {
                let network_ref = Arc::clone(&network_ref);
                tokio::task::spawn(async move {
                    network_ref.request_public().await.unwrap();
                    update_local_info(network_ref, handle).await;
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
            _ => todo!(),
        }
    }
}

fn build_webview(
    network_manager: Arc<NetworkManager>,
) -> (
    tokio::task::JoinHandle<()>,
    watch::Receiver<Option<web_view::Handle<WebuiState>>>,
) {
    let (mut hsnd, hrecv) = watch::channel(None);
    let network_ref = Arc::clone(&network_manager);
    let retfut = move || async move {
        let mut webview: WebView<'static, _> = web_view::builder()
            .content(web_view::Content::Html(WEBPAGE))
            .title("Ilan's VLCSync")
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
    let rethandle = tokio::task::spawn(async {
        let local_set = tokio::task::LocalSet::new();
        local_set.spawn_local(retfut());
        local_set.await
    });
    (rethandle, hrecv)
}
