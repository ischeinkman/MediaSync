mod protocols;

mod utils;

mod players;

mod network;

mod cmdui;
mod logging;
mod traits;
#[cfg(feature = "webui")]
mod webui;
#[cfg(feature = "webui")]
mod webview_helper;
use network::NetworkManager;
use protocols::{Message, TimeStamp};
use traits::sync::{SyncPlayer, SyncPlayerWrapper};

use futures::future::FutureExt;
use futures::StreamExt;
use futures::TryStreamExt;

use std::sync::Arc;
use tokio::runtime::Builder;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

pub type MyError = Box<dyn std::error::Error + Send + Sync>;

type DynResult<T> = Result<T, MyError>;

pub const PUSH_FREQUENCY_MILLIS: u64 = 100;

pub fn main() -> DynResult<()> {
    let mut runtime = Builder::new()
        .threaded_scheduler()
        .enable_io()
        .enable_time()
        .build()?;
    let local_set = tokio::task::LocalSet::new();

    if use_gui() && cfg!(feature = "webui") {
        #[cfg(feature = "webui")]
        runtime
            .block_on(async {
                let res = local_set.spawn_local(webui::run());
                local_set.await;
                res.await
            })
            .unwrap()
            .unwrap();
    } else {
        runtime
            .block_on(async {
                let res = local_set.spawn_local(cmdui::run());
                local_set.await;
                res.await
            })
            .unwrap()
            .unwrap();
    }
    println!("Ending.");
    Ok(())
}

fn use_gui() -> bool {
    std::env::args().len() <= 1 || std::env::args().any(|a| a == "--webui")
}

async fn local_broadcast_task(
    player: Arc<Mutex<SyncPlayerWrapper<Box<dyn SyncPlayer>>>>,
    network_manager: Arc<NetworkManager>,
) {
    let local_stream = {
        let prev_instant = Instant::now();
        let now = TimeStamp::now();
        let next_instant = Instant::now();

        let offset_millis = now.as_millis() % PUSH_FREQUENCY_MILLIS;
        let delay_millis = 2 * PUSH_FREQUENCY_MILLIS - offset_millis;

        let avg_instant = prev_instant + (next_instant - prev_instant) / 2;
        let start_time = avg_instant + Duration::from_millis(delay_millis);

        let time_stream = tokio::time::interval_at(start_time, Duration::from_millis(delay_millis));

        time_stream
            .map(Ok)
            .try_for_each(move |_tm| {
                let player_ref = Arc::clone(&player);
                let network_ref = Arc::clone(&network_manager);
                async move {
                    let mut player_lock = player_ref.lock().await;
                    let player_state = match player_lock.get_sync_status().await {
                        Ok(v) => v,
                        Err(e) => {
                            return Err(e);
                        }
                    };
                    network_ref.broadcast_event(player_state.into()).await?;
                    Ok(())
                }
            })
            .map(|res| {
                if res.is_err() {
                    log::error!("Got error in local_broadcast_task. Now dying.");
                }
                res.unwrap();
            })
    };
    local_stream.await
}

async fn remote_sink_task(
    player: Arc<Mutex<SyncPlayerWrapper<Box<dyn SyncPlayer>>>>,
    network_manager: Arc<NetworkManager>,
) {
    let player_ref = Arc::clone(&player);
    let network_ref = Arc::clone(&network_manager);
    network_manager
        .remote_event_stream()
        .try_for_each(move |evt| {
            let player_ref = Arc::clone(&player_ref);
            let network_ref = Arc::clone(&network_ref);
            async move {
                let mut player_lock = player_ref.lock().await;
                let outpt = match evt {
                    Message::Sync(msg) => player_lock.push_sync_status(msg),
                };
                match outpt.await {
                    Ok(true) => {
                        network_ref.broadcast_event(evt).await?;
                        tokio::task::yield_now().await;
                        Ok(())
                    }
                    Ok(false) => Ok(()),
                    Err(e) => Err(e),
                }
            }
        })
        .map(|res| {
            if let Err(e) = res {
                if let Some(e) = e.downcast_ref::<std::io::Error>() {
                    match e.kind() {
                        std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::UnexpectedEof => {}
                        _ => {
                            Result::<(), _>::Err(e).unwrap();
                        }
                    }
                } else {
                    Result::<(), _>::Err(e).unwrap();
                }
            } else {
                res.unwrap();
            }
        })
        .await
}
