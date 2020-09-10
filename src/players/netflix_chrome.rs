use crate::messages::{PlayerPosition, PlayerState};
use crate::traits::{SyncPlayer, SyncPlayerList};
use crate::DynResult;

use futures::stream::{FuturesUnordered, StreamExt};

use futures::{future::LocalBoxFuture, FutureExt};

use headless_chrome::Browser;

pub struct NetflixPlayer {
    browser: Browser,
    #[allow(unused)]
    proc: chrome_process::Process,
}

fn cmd_to_js(cmd: &str) -> String {
    const JS_API: &str = include_str!("../../static/netflix_injector.js");
    let retvl = format!(
        r"( 
            function() {{ 
                console.log('HELLO!'); 
                {}; 
                return {};
            }} 
        )()",
        JS_API, cmd
    );
    retvl
}

impl NetflixPlayer {
    pub async fn new() -> DynResult<Self> {
        let proc = chrome_process::Process::new().await?;

        let browser = Browser::connect(proc.debug_ws_url.clone())?;
        let tab = browser.wait_for_initial_tab()?;
        tab.enable_log()?;
        tab.navigate_to("https://www.netflix.com")?;
        tab.wait_until_navigated()?;
        Ok(Self { browser, proc })
    }
}

impl SyncPlayer for NetflixPlayer {
    fn get_pos<'a>(&'a self) -> LocalBoxFuture<'a, DynResult<PlayerPosition>> {
        async move {
            let tabs = await_threadmutex(self.browser.get_tabs()).await;
            let mut all_poses = tabs
                .iter()
                .map(|tab| async move {
                    let js = cmd_to_js("mediasync.get_pos()");
                    let res = tab.evaluate(&js, false).unwrap();
                    let millis_opt = res.value.and_then(|v| v.as_u64());
                    match millis_opt {
                        Some(m) => DynResult::Ok(PlayerPosition::from_millis(m)),
                        None => {
                            Err(format!("Invalid JS value: {:?}", res.unserializable_value).into())
                        }
                    }
                })
                .collect::<FuturesUnordered<_>>();
            let mut current = match all_poses.next().await {
                Some(res) => res,
                None => {
                    return Err("No windows open?".into());
                }
            };
            while let Some(res) = all_poses.next().await {
                if let Ok(pos) = res {
                    if current.is_err() {
                        current = Ok(pos)
                    } else {
                        return Err("Have too many Netflix tabs open! Can't match them.".into());
                    }
                }
            }
            current
        }
        .boxed_local()
    }
    fn get_state<'a>(&'a self) -> LocalBoxFuture<'a, DynResult<PlayerState>> {
        async move {
            let tabs = await_threadmutex(self.browser.get_tabs()).await;
            let mut all_states = tabs
                .iter()
                .map(|tab| async move {
                    let js = cmd_to_js("mediasync.get_state()");
                    let res = tab.evaluate(&js, false).unwrap();
                    let millis_opt = res.value.and_then(|v| v.as_bool());
                    match millis_opt {
                        Some(true) => Ok(PlayerState::Paused),
                        Some(false) => Ok(PlayerState::Playing),
                        None => {
                            Err(format!("Invalid JS value: {:?}", res.unserializable_value).into())
                        }
                    }
                })
                .collect::<FuturesUnordered<_>>();
            let mut current = match all_states.next().await {
                Some(res) => res,
                None => {
                    return Err("No windows open?".into());
                }
            };
            while let Some(res) = all_states.next().await {
                if let Ok(pos) = res {
                    if current.is_err() {
                        current = Ok(pos)
                    } else {
                        return Err("Have too many Netflix tabs open! Can't match them.".into());
                    }
                }
            }
            current
        }
        .boxed_local()
    }
    fn set_pos<'a>(&'a mut self, state: PlayerPosition) -> LocalBoxFuture<'a, DynResult<()>> {
        async move {
            let tabs = await_threadmutex(self.browser.get_tabs()).await;
            let mut netflix_tabs = tabs
                .iter()
                .filter(|t| t.get_url().contains("netflix.com/watch"));
            let tab = match netflix_tabs.next() {
                Some(t) => t,
                None => {
                    log::warn!("Tried to set state without an active Netflix tab.");
                    return Ok(());
                }
            };
            let state_ms = state.as_millis();
            let js = cmd_to_js(&format!("mediasync.seek({})", state_ms));
            let _res = tab.evaluate(&js, false).unwrap();
            if let Some(_second) = netflix_tabs.next() {
                Err("Have too many Netflix tabs open! Can't match them.".into())
            } else {
                Ok(())
            }
        }
        .boxed_local()
    }
    fn set_state<'a>(&'a mut self, state: PlayerState) -> LocalBoxFuture<'a, DynResult<()>> {
        async move {
            let tabs = await_threadmutex(self.browser.get_tabs()).await;
            let mut netflix_tabs = tabs
                .iter()
                .filter(|t| t.get_url().contains("netflix.com/watch"));
            let tab = match netflix_tabs.next() {
                Some(t) => t,
                None => {
                    log::warn!("Tried to set state without an active Netflix tab.");
                    return Ok(());
                }
            };
            let js = match state {
                PlayerState::Playing => cmd_to_js("mediasync.play()"),
                PlayerState::Paused => cmd_to_js("mediasync.pause()"),
            };
            let _res = tab.evaluate(&js, false).unwrap();
            if let Some(_second) = netflix_tabs.next() {
                Err("Have too many Netflix tabs open! Can't match them.".into())
            } else {
                Ok(())
            }
        }
        .boxed_local()
    }
}

use std::sync::TryLockError;
use std::task::Poll;

async fn await_threadmutex<'a, T: 'a>(m: &'a std::sync::Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    let fut = futures::future::poll_fn(move |ctx| match m.try_lock() {
        Ok(l) => Poll::Ready(l),
        Err(TryLockError::WouldBlock) => {
            ctx.waker().wake_by_ref();
            Poll::Pending
        }
        Err(TryLockError::Poisoned(p)) => Poll::Ready(p.into_inner()),
    });
    fut.await
}

use tokio::sync::{RwLock, RwLockWriteGuard};

pub struct LazyWrapper {
    inner: RwLock<Option<NetflixPlayer>>,
}

impl LazyWrapper {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(None),
        }
    }
    pub async fn init_mut(&mut self) -> DynResult<RwLockWriteGuard<'_, Option<NetflixPlayer>>> {
        let mut lock = self.inner.write().await;
        if lock.is_none() {
            let new_player = NetflixPlayer::new().await?;
            *lock = Some(new_player);
        }
        Ok(lock)
    }
    pub async fn init(&self) -> DynResult<()> {
        let has_initted = self.inner.read().await.is_some();
        if !has_initted {
            let mut init_lock = self.inner.write().await;
            if init_lock.is_none() {
                let new_player = NetflixPlayer::new().await?;
                *init_lock = Some(new_player);
            }
        }
        Ok(())
    }
}

impl SyncPlayer for LazyWrapper {
    fn get_pos<'a>(&'a self) -> LocalBoxFuture<'a, DynResult<PlayerPosition>> {
        async move {
            self.init().await?;
            let lock = self.inner.read().await;
            lock.as_ref().unwrap().get_pos().await
        }
        .boxed_local()
    }
    fn get_state<'a>(&'a self) -> LocalBoxFuture<'a, DynResult<PlayerState>> {
        async move {
            self.init().await?;
            let lock = self.inner.read().await;
            lock.as_ref().unwrap().get_state().await
        }
        .boxed_local()
    }
    fn set_pos<'a>(&'a mut self, state: PlayerPosition) -> LocalBoxFuture<'a, DynResult<()>> {
        async move {
            let mut lock = self.init_mut().await?;
            let player = lock.as_mut().unwrap();
            player.set_pos(state).await
        }
        .boxed_local()
    }
    fn set_state<'a>(&'a mut self, state: PlayerState) -> LocalBoxFuture<'a, DynResult<()>> {
        async move {
            let mut lock = self.init_mut().await?;
            let player = lock.as_mut().unwrap();
            player.set_state(state).await
        }
        .boxed_local()
    }
}

pub struct NetflixPlayerList {}

impl SyncPlayerList for NetflixPlayerList {
    fn new() -> DynResult<Self> {
        Ok(Self {})
    }

    fn get_players(&mut self) -> DynResult<Vec<(String, Box<dyn SyncPlayer>)>> {
        Ok(vec![(
            "Netflix (Chrome)".to_owned(),
            Box::from(LazyWrapper::new()),
        )])
    }
}

mod chrome_process {
    //! Adapted from the headless_chrome process module at
    //! https://github.com/atroche/rust-headless-chrome/blob/3ce562002c6e4f1218d179abc0f0639a10983fa6/src/browser/process.rs
    //!
    //! Needed to remove all of the sandboxing done so that we can control
    //! the user's regular Chrome and re-use existing cookies and the like.
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
    use tokio::net;
    use tokio::process::{Child, Command};

    use headless_chrome::browser::default_executable;

    #[cfg(target_os = "windows")]
    fn data_dir() -> String {
        const DEFAULT_DATA_DIR: &str =
            "--user-data-dir=%userprofile%\\AppData\\Local\\VlcSync\\chrome_data_dir";
        DEFAULT_DATA_DIR.to_owned()
    }

    #[cfg(target_family = "unix")]
    fn data_dir() -> String {
        let home = std::env::var("HOME").unwrap();
        let flag = format!("--user-data-dir={}/.config/vlcsync/chrome_data_dir", home);
        flag
    }
    pub struct Process {
        child_process: Child,
        pub debug_ws_url: String,
    }

    impl Drop for Process {
        fn drop(&mut self) {
            log::info!("Killing Chrome. PID: {}", self.child_process.id());
            if self.child_process.kill().is_ok() {}
        }
    }

    const DEFAULT_ARGS: &[&str] = &[
        "--enable-features=NetworkService,NetworkServiceInProcess",
        "--metrics-recording-only",
        "--no-first-run",
        "--enable-automation",
        "--enable-logging",
        "--verbose",
        "--log-level=0",
        "--no-first-run",
    ];

    impl Process {
        pub async fn new() -> crate::DynResult<Self> {
            let mut process = Self::start_process().await?;

            log::info!("Started Chrome. PID: {}", process.id());

            let url = Self::ws_url_from_output(&mut process).await?;
            log::debug!("Found debugging WS URL: {:?}", url);

            Ok(Self {
                child_process: process,
                debug_ws_url: url,
            })
        }

        async fn start_process() -> crate::DynResult<Child> {
            let debug_port = get_available_port().await.ok_or("No available ports!")?;
            let port_option = format!("--remote-debugging-port={}", debug_port);

            let mut args = DEFAULT_ARGS.to_vec();
            args.push(port_option.as_str());
            let data_dir = data_dir();
            args.push(data_dir.as_str());

            let path = default_executable().map_err(|e| e)?;

            log::info!("Launching Chrome binary at {:?}", &path);
            let mut command = Command::new(&path);

            let process = command.args(&args).stderr(Stdio::piped()).spawn()?;
            Ok(process)
        }

        async fn ws_url_from_reader<R>(reader: BufReader<R>) -> crate::DynResult<Option<String>>
        where
            R: AsyncRead + Unpin,
        {
            let mut lns = reader.lines();
            while let Some(ln) = lns.next_line().await? {
                let mut ln: String = ln;
                if ln.starts_with("ERROR") && ln.contains("bind") {
                    return Err(format!("Port in use! Got log line: {}", ln).into());
                }
                let needle = "listening on ";
                let url_start = ln.find(needle).map(|start| start + needle.len());
                if let Some(start_idx) = url_start {
                    let res = ln.split_off(start_idx);
                    return Ok(Some(res));
                }
            }
            Ok(None)
        }

        async fn ws_url_from_output(child_process: &mut Child) -> crate::DynResult<String> {
            use futures::future::FutureExt;
            let mut timer = tokio::time::delay_for(Duration::from_secs(30)).fuse();
            loop {
                let my_stderr = BufReader::new(child_process.stderr.as_mut().unwrap());
                let read_fut = Self::ws_url_from_reader(my_stderr).fuse();
                futures::pin_mut!(read_fut);
                futures::select! {
                    r = read_fut => {
                        match r {
                            Ok(output_option) => {
                                if let Some(output) = output_option {
                                    break Ok(output);
                                }
                            }
                            Err(err) => {
                                break Err(err);
                            }
                        }
                    },
                    _ = timer => {
                        break Err("Timed out!".into());
                    }
                }
            }
        }
    }

    async fn get_available_port() -> Option<u16> {
        let mut count = 0;
        loop {
            let port = crate::network::utils::random_port(8000, 10000);
            if let Ok(con) = net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await {
                drop(con);
                return Some(port);
            }
            count += 1;
            if count >= 10 {
                return None;
            }
        }
    }
}
