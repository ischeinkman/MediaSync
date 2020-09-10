use futures::future::{self, FutureExt, LocalBoxFuture};
use futures::stream::TryStreamExt;
use hyper::body::{self, Buf};
use hyper::client::{Client, HttpConnector};
use hyper::header::{self, HeaderValue};
use hyper::StatusCode;
use hyper::{Body, Request};
use std::cell::{Cell, RefCell};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::process::Stdio;
use tokio::process::{Child, Command};

use crate::messages::{PlayerPosition, PlayerState};
use crate::traits::{SyncPlayer, SyncPlayerList};
use crate::DynResult;

#[cfg(target_os = "windows")]
const BIN_PATH: &str = "C:\\Program Files\\VideoLAN\\VLC\\vlc.exe";

#[cfg(target_family = "unix")]
const BIN_PATH: &str = "vlc";

const DEFAULT_PASS: &str = "vlcsynchttppass";

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct HttpConfig {
    pub bind_addr: SocketAddr,
    pub password: String,
}

impl Default for HttpConfig {
    fn default() -> Self {
        let bind_addr = (Ipv4Addr::LOCALHOST, 9090).into();
        let password = DEFAULT_PASS.to_owned();
        Self {
            bind_addr,
            password,
        }
    }
}

impl HttpConfig {
    pub fn authorization_header(&self) -> String {
        let mut retvl = "Basic ".to_string();
        let auth = format!(":{}", self.password);
        base64::encode_config_buf(auth, base64::STANDARD, &mut retvl);
        retvl
    }

    pub fn status_url(&self) -> String {
        let mut retvl: String = "http://".to_owned();
        if self.bind_addr.ip().is_loopback() {
            use std::fmt::Write;
            retvl.push_str("localhost");
            write!(&mut retvl, ":{}", self.bind_addr.port()).unwrap();
        } else {
            retvl.push_str(&self.bind_addr.to_string());
        }
        retvl.push_str("/requests/status.json");
        retvl
    }
}

fn vlc_process(conf: &HttpConfig) -> std::io::Result<Child> {
    let retvl = Command::new(BIN_PATH)
        .arg("--extraintf")
        .arg("http")
        .arg("--http-host")
        .arg(&conf.bind_addr.ip().to_string())
        .arg("--http-port")
        .arg(format!("{}", conf.bind_addr.port()))
        .arg("--http-password")
        .arg(&conf.password)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    Ok(retvl)
}

pub enum SubprocessStatus {
    Started(Child),
    NotYetStarted,
    ReusingExisting,
}

impl SubprocessStatus {
    pub fn started(&self) -> bool {
        match self {
            SubprocessStatus::NotYetStarted => false,
            SubprocessStatus::Started(_) => true,
            SubprocessStatus::ReusingExisting => true,
        }
    }
    pub async fn start(&mut self, conf: &HttpConfig) -> std::io::Result<()> {
        if self.started() {
            return Ok(());
        }
        let child = vlc_process(conf)?;
        let start = std::time::Instant::now();
        loop {
            match std::net::TcpStream::connect(conf.bind_addr) {
                Ok(strm) => {
                    drop(strm);
                    break;
                }
                Err(e) => {
                    if start.elapsed() >= std::time::Duration::from_millis(200) {
                        return Err(e);
                    } else {
                        tokio::task::yield_now().await;
                    }
                }
            }
        }
        *self = SubprocessStatus::Started(child);
        Ok(())
    }
}

struct BufReadWrapper<T: Buf> {
    inner: T,
}

impl<T: Buf> io::Read for BufReadWrapper<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let readlen = self.inner.remaining().min(buf.len());
        if readlen == 0 {
            return Ok(0);
        }
        let bufslice = &mut buf[..readlen];
        self.inner.copy_to_slice(bufslice);
        Ok(readlen)
    }
}

#[derive(Debug)]
pub struct HttpStatusError {
    code: StatusCode,
}

impl std::fmt::Display for HttpStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "HTTP Error Code {}: {}",
            self.code.as_u16(),
            self.code.canonical_reason().unwrap_or_default()
        ))
    }
}

impl std::error::Error for HttpStatusError {}
pub struct VlcHttpPlayer {
    conf: HttpConfig,
    client: Client<HttpConnector>,
    subprocess: RefCell<SubprocessStatus>,
    length_estimator: Cell<LengthEstimator>,
    results_cache: Cell<Option<(std::time::Instant, PlayerPosition, PlayerState)>>,
}

impl VlcHttpPlayer {
    pub async fn from_existing(config: HttpConfig) -> DynResult<Self> {
        let retvl = Self {
            conf: config,
            client: Client::new(),
            subprocess: RefCell::new(SubprocessStatus::ReusingExisting),
            length_estimator: Cell::new(LengthEstimator::new()),
            results_cache: Cell::new(None),
        };
        let (pos, state) = retvl.do_http_request(None).await?;
        retvl
            .results_cache
            .set(Some((std::time::Instant::now(), pos, state)));
        Ok(retvl)
    }
    pub async fn new_subprocess(config: HttpConfig) -> DynResult<Self> {
        let retvl = Self {
            conf: config,
            client: Client::new(),
            subprocess: RefCell::new(SubprocessStatus::NotYetStarted),
            length_estimator: Cell::new(LengthEstimator::new()),
            results_cache: Cell::new(None),
        };
        Ok(retvl)
    }
    async fn do_http_request(
        &self,
        command: Option<HttpCommand>,
    ) -> DynResult<(PlayerPosition, PlayerState)> {
        let started = match self.subprocess.try_borrow() {
            Ok(cur) => cur.started(),
            Err(_) => true,
        };
        if !started {
            let mut cur = self.subprocess.replace(SubprocessStatus::NotYetStarted);
            cur.start(&self.conf).await.unwrap();
            self.subprocess.replace(cur);
        } else {
        }
        let auth_header = HeaderValue::from_str(&self.conf.authorization_header()).unwrap();
        let mut req = Request::new(Body::empty());
        req.headers_mut().insert(header::AUTHORIZATION, auth_header);
        let base_url = self.conf.status_url();
        let url = match command {
            Some(HttpCommand::Seek(SeekCommand::Seconds(secs))) => {
                format!("{}?command=seek&val={}s", base_url, secs)
            }
            Some(HttpCommand::Seek(SeekCommand::Percent(pc))) => {
                format!("{}?command=seek&val={}%25", base_url, pc)
            }
            Some(HttpCommand::SetState(PlayerState::Playing)) => {
                format!("{}?command=pl_play", base_url)
            }
            Some(HttpCommand::SetState(PlayerState::Paused)) => {
                format!("{}?command=pl_pause", base_url)
            }
            None => base_url,
        };
        *req.uri_mut() = url.parse().unwrap();
        let resp = self.client.request(req).await.unwrap();
        if !resp.status().is_success() {
            log::error!("Error when making request to URL: {}", url);
            return Err(HttpStatusError {
                code: resp.status(),
            }
            .into());
        }
        let (sample, state) = Self::parse_resp(resp).await.unwrap();
        let mut est = self.length_estimator.get();
        let pos = est.estimate_position(sample);
        self.length_estimator.set(est);
        Ok((pos, state))
    }

    async fn parse_resp(resp: hyper::Response<Body>) -> DynResult<(LengthSample, PlayerState)> {
        let body = resp.into_body();
        let resp_bytes = body::aggregate(body).await;
        let reader = BufReadWrapper {
            inner: resp_bytes.unwrap(),
        };
        let json: serde_json::Value = serde_json::from_reader(reader).unwrap();
        let root_obj = match json {
            serde_json::Value::Object(obj) => obj,
            _ => todo!(),
        };
        let pos = root_obj["position"].as_f64().unwrap_or(0.0);
        let length = root_obj["length"].as_u64().unwrap_or(0);
        let disp_time = root_obj["time"].as_u64().unwrap_or(0);

        let sample = LengthSample {
            reported_length: length,
            reported_pos: pos,
            reported_time: disp_time,
        };

        let rawstate = root_obj["state"].as_str().unwrap();
        let state = match rawstate {
            "playing" => PlayerState::Playing,
            "paused" => PlayerState::Paused,
            "stopped" => PlayerState::Paused,
            e => {
                return Err(format!("Invalid state in json: {}", e).into());
            }
        };

        Ok((sample, state))
    }
}

impl SyncPlayer for VlcHttpPlayer {
    fn get_pos(&self) -> LocalBoxFuture<'_, DynResult<PlayerPosition>> {
        let now = std::time::Instant::now();
        if let Some((prev_time, prev_pos, _)) = self.results_cache.get() {
            let dt = now.duration_since(prev_time);
            if dt <= std::time::Duration::from_millis(10) {
                return future::ready(Ok(prev_pos)).boxed_local();
            }
        }
        let fut = async move {
            let (pos, state) = self.do_http_request(None).await?;
            let now = std::time::Instant::now();
            self.results_cache.set(Some((now, pos, state)));
            Ok(pos)
        };
        fut.boxed_local()
    }
    fn get_state(&self) -> LocalBoxFuture<'_, DynResult<PlayerState>> {
        let now = std::time::Instant::now();
        if let Some((prev_time, _, prev_state)) = self.results_cache.get() {
            let dt = now.duration_since(prev_time);
            if dt <= std::time::Duration::from_millis(10) {
                return future::ready(Ok(prev_state)).boxed_local();
            }
        }
        let fut = async move {
            let (pos, state) = self.do_http_request(None).await.unwrap();
            let now = std::time::Instant::now();
            self.results_cache.set(Some((now, pos, state)));
            Ok(state)
        };
        fut.boxed_local()
    }
    fn set_pos(&mut self, state: PlayerPosition) -> LocalBoxFuture<'_, DynResult<()>> {
        let fut = async move {
            let command = self.length_estimator.get().estimate_seek(state);
            let (pos, state) = self
                .do_http_request(Some(HttpCommand::Seek(command)))
                .await?;
            let now = std::time::Instant::now();
            self.results_cache.set(Some((now, pos, state)));
            Ok(())
        };
        fut.boxed_local()
    }
    fn set_state(&mut self, state: PlayerState) -> LocalBoxFuture<'_, DynResult<()>> {
        let fut = async move {
            let command = Some(HttpCommand::SetState(state));
            let mut nxt = self.do_http_request(command).await.unwrap();
            if nxt.1 != state {
                nxt = self.do_http_request(command).await.unwrap();
            }
            let now = std::time::Instant::now();
            self.results_cache.set(Some((now, nxt.0, nxt.1)));
            Ok(())
        };
        fut.boxed_local()
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct LengthSample {
    reported_pos: f64,
    reported_time: u64,
    reported_length: u64,
}
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct LengthEstimator {
    reported_length: u64,
    min_error: u16,
    max_error: u16,
}

impl Default for LengthEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl LengthEstimator {
    fn new() -> Self {
        Self {
            reported_length: 0,
            min_error: 0,
            max_error: 1000,
        }
    }
    fn min_length_millis(&self) -> u64 {
        self.reported_length * 1000 + u64::from(self.min_error)
    }
    fn max_length_millis(&self) -> u64 {
        self.reported_length * 1000 + u64::from(self.max_error)
    }
    fn reset(&mut self) {
        self.min_error = 0;
        self.max_error = 1000;
    }
    fn push_sample(&mut self, sample: LengthSample) {
        if sample.reported_length != self.reported_length {
            self.reset();
            self.reported_length = sample.reported_length;
        }
        let (sample_lmin, sample_lmax) = Self::sample_length_error(sample);
        if sample_lmin > self.min_error {
            self.min_error = sample_lmin;
        }
        if sample_lmax < self.max_error {
            self.max_error = sample_lmax;
        }
        if self.max_error < self.min_error {
            log::error!("Bad error range: {} > {}", self.min_error, self.max_error);
            assert!(self.min_error <= self.max_error);
        }
    }
    fn estimate_position(&mut self, sample: LengthSample) -> PlayerPosition {
        let min_reported = sample.reported_time * 1000;
        let max_reported = min_reported + 1000;

        self.push_sample(sample);
        let min_length = self.min_length_millis();
        let min_postime = (sample.reported_pos * (min_length as f64)) as u64;
        let max_length = self.max_length_millis();
        let max_postime = (sample.reported_pos * (max_length as f64)) as u64;
        let min_time = min_postime.max(min_reported);
        let max_time = max_postime.min(max_reported);
        if min_time > max_time {
            log::warn!("Vlc HTTP Length estimator produced bad range ({}, {}) from params ({}, {}) and sample ({}, {}). Now reseting estimate.", 
                min_time, max_time, self.min_length_millis(), self.max_length_millis(), sample.reported_time, sample.reported_pos
            );
            self.reset();
            self.push_sample(sample);
        }
        let center = (max_time + min_time) / 2;
        /*log::info!(
            "({}, {}, {}) + ({}, {}) => ({}, {}), ({}, {}) => ({}, {}) => {}_{} +- {}",
            sample.reported_pos,
            sample.reported_time,
            sample.reported_length,
            self.min_length_millis(),
            self.max_length_millis(),
            min_postime,
            max_postime,
            min_reported,
            max_reported,
            min_time,
            max_time,
            center / 1000,
            center % 1000,
            (max_time - min_time) / 2
        );*/
        PlayerPosition::from_millis(center)
    }

    fn estimate_seek(&self, new_pos: PlayerPosition) -> SeekCommand {
        /*
        pa = t/(Lm)
        pb = t/(LM)

        pa * LM - pb * Lm
        = t(LM/Lm - Lm/LM)
        */

        let length_ratio_big =
            (self.max_length_millis() as f64) / (self.min_length_millis() as f64);
        let length_ratio_small =
            (self.min_length_millis() as f64) / (self.max_length_millis() as f64);
        let ratio_diff = length_ratio_big - length_ratio_small;
        let time_range = ratio_diff * (new_pos.as_millis() as f64);
        if time_range < 300.0 {
            let avg_len = (self.max_length_millis() + self.min_length_millis()) / 2;
            let pos_est = (new_pos.as_millis() as f64 * 100.0) / (avg_len as f64);
            SeekCommand::Percent(pos_est)
        } else {
            SeekCommand::Seconds(new_pos.as_millis() / 1000)
        }
    }

    fn sample_length_error(sample: LengthSample) -> (u16, u16) {
        if sample.reported_pos <= f64::from(std::f32::EPSILON) {
            return (0, 1000);
        }
        let reported_millis = sample.reported_time * 1000;
        let sample_min_time = (reported_millis as f64) / sample.reported_pos;
        let sample_min_millis = (sample_min_time * 1000.0) as u64;
        let sample_min_error = if sample_min_millis <= reported_millis
            || sample_min_millis >= reported_millis + 1000
        {
            0
        } else {
            sample_min_millis - reported_millis
        };
        let sample_error_range = (1000.0 / sample.reported_pos) as u64;
        let sample_max_error = sample_min_error + sample_error_range;
        (sample_min_error as u16, sample_max_error.max(1000) as u16)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum SeekCommand {
    Percent(f64),
    Seconds(u64),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum HttpCommand {
    Seek(SeekCommand),
    SetState(PlayerState),
}

/*
p * La + p * Le = ta + te

To estimate Le:
ta <= p * La + p * Le <= ta + 1 sec
ta - p * LA <= p * Le <= ta + 1 sec - p * La
ta/p - La <= Le <= ta/p - La + 1 sec/p

To estimate te, from Le+ and Le-:

p * La + p * Le- <= ta + te <= p * La + p * Le+
p * La + p * Le-  <= ta + te <= p * La + p * Le+
*/

pub struct VlcHttpList {}

impl VlcHttpList {
    pub fn should_offer_subprocess(&self) -> bool {
        true
    }
    pub fn existing_configs(&mut self) -> impl IntoIterator<Item = HttpConfig> {
        None
    }
}

impl SyncPlayerList for VlcHttpList {
    fn new() -> DynResult<Self> {
        Ok(Self {})
    }
    fn get_players(&mut self) -> DynResult<Vec<(String, Box<dyn SyncPlayer>)>> {
        let fut = async {
            let conf_iter = self.existing_configs().into_iter().map(DynResult::Ok);
            let confs = futures::stream::iter(conf_iter);
            let mut mapped: Vec<_> = confs
                .and_then(|conf| async move {
                    let name = format!("Vlc Player at {}", conf.bind_addr);
                    let player = VlcHttpPlayer::from_existing(conf).await?;
                    let player: Box<dyn SyncPlayer> = Box::new(player);
                    Ok((name, player))
                })
                .try_collect()
                .await?;
            if self.should_offer_subprocess() {
                let conf = HttpConfig::default();
                let name = format!("New Vlc Subprocess via {}", conf.bind_addr);
                let player = VlcHttpPlayer::new_subprocess(conf).await.unwrap();
                let player: Box<dyn SyncPlayer> = Box::new(player);
                mapped.push((name, player));
            }

            Ok(mapped)
        };
        crate::utils::block_on(fut)
    }
}
