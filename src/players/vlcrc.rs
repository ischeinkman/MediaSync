use crate::messages::TimeStamp;
use crate::messages::{PlayerPosition, PlayerState};
use crate::traits::sync::{SyncPlayer, SyncPlayerList};
use crate::utils::AbsSub;
use crate::DynResult;
use futures::future::FutureExt;
use futures::future::LocalBoxFuture;
use std::process::{Child, Command};

use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;

#[cfg(target_os = "windows")]
const BIN_PATH: &str = "C:\\Program Files (x86)\\VideoLAN\\VLC\\vlc.exe";

#[cfg(target_family = "unix")]
const BIN_PATH: &str = "vlc";

fn vlc_process() -> std::io::Result<Child> {
    let retvl = Command::new(BIN_PATH)
        .arg("--extraintf")
        .arg("rc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    Ok(retvl)
}

pub struct VlcRcPlayer {
    vlc_process: RefCell<Option<Child>>,
    previous_pos: Cell<PlayerPosition>,
    previous_pos_query: Cell<TimeStamp>,
}

fn run_command(
    proc: impl std::ops::DerefMut<Target = Child>,
    cmd: &str,
) -> std::io::Result<String> {
    let mut child_borrow = proc;
    let stdin = child_borrow.stdin.as_mut().unwrap();
    writeln!(stdin, "{}", cmd)
        .and_then(|_| stdin.flush())
        .unwrap();
    let stdout = child_borrow.stdout.as_mut().unwrap();
    let mut stdout_buff = BufReader::new(stdout);
    let mut secs_line = Vec::new();
    stdout_buff.read_until(b'>', &mut secs_line)?;
    while secs_line.starts_with(b"VLC") {
        secs_line.clear();
        stdout_buff.read_until(b'>', &mut secs_line)?;
    }
    if secs_line.starts_with(&[b'>']) {
        secs_line.remove(0);
    }
    if secs_line.ends_with(b">") {
        secs_line.pop();
    }
    Ok(String::from_utf8_lossy(&secs_line).into())
}

impl VlcRcPlayer {
    pub fn new() -> Self {
        Self {
            vlc_process: RefCell::new(None),
            previous_pos: Cell::new(PlayerPosition::from_millis(0)),
            previous_pos_query: Cell::new(TimeStamp::now()),
        }
    }
    fn run_command(&self, cmd: &str) -> std::io::Result<String> {
        let mut childopt = self.vlc_process.borrow_mut();
        if childopt.is_none() {
            let nchild = vlc_process().unwrap();
            *childopt = Some(nchild);
        }
        let child_borrow = childopt.as_mut().unwrap();
        run_command(child_borrow, cmd)
    }
    fn run_command_mut(&mut self, cmd: &str) -> std::io::Result<String> {
        let childopt = self.vlc_process.get_mut();
        if childopt.is_none() {
            let nchild = vlc_process().unwrap();
            *childopt = Some(nchild);
        }
        let child_borrow = childopt.as_mut().unwrap();
        run_command(child_borrow, cmd)
    }
}

impl SyncPlayer for VlcRcPlayer {
    fn get_pos(&self) -> LocalBoxFuture<'_, DynResult<PlayerPosition>> {
        let fut = async move {
            let raw_time = self.run_command("get_time").unwrap();
            let trimmed_secs = raw_time.trim();
            let returned_pos = if trimmed_secs.is_empty() {
                PlayerPosition::from_millis(0)
            } else {
                let parsed: u64 = trimmed_secs.parse().unwrap();
                PlayerPosition::from_millis(parsed * 1000)
            };
            let previous_pos = self.previous_pos.get();
            let previous_pos_query = self.previous_pos_query.get();
            let now = TimeStamp::now();
            if returned_pos == previous_pos
                && self.get_state().await.unwrap() == PlayerState::Playing
            {
                let dt = now.abs_sub(previous_pos_query);
                let retvl = returned_pos + dt;
                Ok(retvl)
            } else {
                self.previous_pos.set(returned_pos);
                self.previous_pos_query.set(now);
                Ok(returned_pos)
            }
        };
        fut.boxed_local()
    }
    fn get_state<'a>(&'a self) -> LocalBoxFuture<'a, DynResult<PlayerState>> {
        let fut = async move {
            let raw_state = self.run_command("status").unwrap();
            let trimmed_state = raw_state.trim();
            if trimmed_state.contains("playing") {
                Ok(PlayerState::Playing)
            } else if trimmed_state.contains("paused") || trimmed_state.contains("stopped") {
                Ok(PlayerState::Paused)
            } else {
                Err(format!(
                    "Error: vlc rc interface gave invalid is_playing result of {}",
                    trimmed_state
                )
                .into())
            }
        };
        fut.boxed_local()
    }
    fn set_pos(&mut self, state: PlayerPosition) -> LocalBoxFuture<'_, DynResult<()>> {
        let fut = async move {
            let seconds =
                state.as_millis() / 1000 + if state.as_millis() % 1000 > 500 { 1 } else { 0 };
            let current = self.get_pos().await?.as_millis() / 1000;
            if current.abs_sub(seconds) > 1 {
                self.run_command_mut(format!("seek {}", seconds).as_ref())
                    .unwrap();
            }
            Ok(())
        };
        fut.boxed_local()
    }
    fn set_state<'a>(&'a mut self, state: PlayerState) -> LocalBoxFuture<'a, DynResult<()>> {
        match state {
            PlayerState::Playing => {
                self.run_command_mut("play").unwrap();
            }
            PlayerState::Paused => {
                self.run_command_mut("pause").unwrap();
                let cur_secs = self.run_command_mut("get_time").unwrap();
                self.run_command_mut(format!("seek {}", cur_secs).as_ref())
                    .unwrap();
            }
        };

        futures::future::ready(Ok(())).boxed_local()
    }
}

pub struct VlcRcList {}

impl SyncPlayerList for VlcRcList {
    fn new() -> DynResult<Self> {
        Ok(VlcRcList {})
    }
    fn get_players(&mut self) -> DynResult<Vec<(String, Box<dyn SyncPlayer>)>> {
        Ok(vec![(
            "Remote VLC subprocess".to_owned(),
            Box::new(VlcRcPlayer::new()),
        )])
    }
}
