use crate::TimeStamp;
use log::{LevelFilter, Log, Metadata, Record};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

#[derive(PartialEq, Eq, Clone, Debug, Default, Hash, Ord, PartialOrd)]
pub struct SinkKey {
    name: Cow<'static, str>,
}

impl SinkKey {
    pub const fn fixed(name: &'static str) -> Self {
        let name = Cow::Borrowed(name);
        SinkKey { name }
    }

    pub const fn dynamic(name: String) -> Self {
        let name = Cow::Owned(name);
        SinkKey { name }
    }
}
#[derive(Clone)]
pub struct LogSinkConfig {
    filter: Option<LevelFilter>,
    only_local: bool,
}

#[allow(dead_code)]
impl LogSinkConfig {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn is_enabled(&self) -> bool {
        self.filter.is_some()
    }
    pub fn enable_all(mut self) -> Self {
        self.filter = Some(LevelFilter::max());
        self
    }
    pub fn disable(mut self) -> Self {
        self.filter = None;
        self
    }
    pub fn with_filter(mut self, filter: LevelFilter) -> Self {
        self.filter = Some(filter);
        self
    }
    pub fn with_nonlocal(mut self, nonlocal: bool) -> Self {
        self.only_local = !nonlocal;
        self
    }
    pub fn matches_level(&self, meta: &Metadata) -> bool {
        if !self.is_enabled() {
            return false;
        }
        self.filter
            .map(|filter| meta.level() < filter)
            .unwrap_or(false)
    }
    pub fn should_log(&self, record: &Record) -> bool {
        let matches_level = self.matches_level(record.metadata());
        let matches_source = record
            .module_path()
            .map(|pt| pt.contains("vlcsync"))
            .unwrap_or(true);
        let log_all = !self.only_local;
        let source_flag = log_all || matches_source;
        matches_level && source_flag
    }
}

impl Default for LogSinkConfig {
    fn default() -> Self {
        Self {
            filter: None,
            only_local: true,
        }
    }
}

pub trait LogSink: Sync + Send {
    fn key(&self) -> SinkKey;
    fn log<'a>(&self, msg: std::fmt::Arguments<'a>) -> crate::DynResult<()>;
    fn flush(&self) -> crate::DynResult<()> {
        Ok(())
    }
}

fn read_uncaring<'a, T>(lock: &'a RwLock<T>) -> std::sync::RwLockReadGuard<'a, T> {
    match lock.read() {
        Ok(l) => l,
        Err(e) => e.into_inner(),
    }
}
fn write_uncaring<'a, T>(lock: &'a RwLock<T>) -> std::sync::RwLockWriteGuard<'a, T> {
    match lock.write() {
        Ok(l) => l,
        Err(e) => e.into_inner(),
    }
}
pub struct LogSinkWrapper {
    configs: RwLock<HashMap<SinkKey, LogSinkConfig>>,
    sinks: RwLock<HashMap<SinkKey, Box<dyn LogSink>>>,
}

lazy_static::lazy_static! {
    static ref CURRENT_LOGWRAPPER : Arc<LogSinkWrapper> = {
        let retvl = Arc::new(LogSinkWrapper::new());
        let staticlogger = Arc::clone(&retvl);
        log::set_boxed_logger(Box::from(LogSinkWrapperHandle::new(staticlogger))).unwrap();

        // We filter ourselves for each sink
        log::set_max_level(log::LevelFilter::max());
        retvl
    };
}

#[allow(dead_code)]
impl LogSinkWrapper {
    fn new() -> Self {
        Self {
            configs: RwLock::new(HashMap::new()),
            sinks: RwLock::new(HashMap::new()),
        }
    }
    pub fn get_handle() -> Arc<LogSinkWrapper> {
        Arc::clone(&CURRENT_LOGWRAPPER)
    }
    pub fn add_logger<L: LogSink + 'static>(&self, logger: L, conf: LogSinkConfig) {
        let mut conf_lock = write_uncaring(&self.configs);
        let mut sink_lock = write_uncaring(&self.sinks);
        conf_lock.insert(logger.key(), conf);
        sink_lock.insert(logger.key(), Box::from(logger));
    }

    pub fn current_config_for(&self, key: &SinkKey) -> LogSinkConfig {
        let conf_lock = read_uncaring(&self.configs);
        conf_lock.get(key).cloned().unwrap_or_default()
    }

    pub fn update_config_for(
        &self,
        key: &SinkKey,
        new_conf: LogSinkConfig,
    ) -> Result<LogSinkConfig, LogSinkConfig> {
        let mut new_conf = new_conf;
        let mut conf_lock = write_uncaring(&self.configs);
        let previous_ref = conf_lock.get_mut(key);
        match previous_ref {
            Some(rf) => {
                std::mem::swap(rf, &mut new_conf);
                Ok(new_conf)
            }
            None => Err(new_conf),
        }
    }
}

impl Log for LogSinkWrapper {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let conf_lock = read_uncaring(&self.configs);
        conf_lock.values().any(|conf| conf.matches_level(metadata))
    }
    fn log(&self, record: &Record) {
        let sink_lock = read_uncaring(&self.sinks);
        let conf_lock = read_uncaring(&self.configs);
        let valids = sink_lock.iter().filter(|(key, _)| {
            conf_lock
                .get(key)
                .map(Cow::Borrowed)
                .unwrap_or_default()
                .should_log(record)
        });
        let errs = valids.filter_map(|(key, sink)| {
            sink.log(format_args!(
                "[{}] [{}] : {}",
                TimeStamp::now().as_millis(),
                record.level(),
                record.args()
            ))
            .err()
            .map(|err| (key, err))
        });
        for (key, err) in errs {
            sink_lock.values().for_each(|snk| {
                let res = snk.log(format_args!(
                    "[{}] [{}] : Error when logging {} : {}",
                    TimeStamp::now().as_millis(),
                    "ERMAX",
                    key.name,
                    err
                ));
                if let Err(_e) = res {}
            });
        }
    }
    fn flush(&self) {
        let sink_lock = read_uncaring(&self.sinks);
        sink_lock.values().try_for_each(|snk| snk.flush()).unwrap();
    }
}

pub struct LogSinkWrapperHandle {
    inner: Arc<LogSinkWrapper>,
}

impl LogSinkWrapperHandle {
    fn new(inner: Arc<LogSinkWrapper>) -> Self {
        Self { inner }
    }
}

impl Log for LogSinkWrapperHandle {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.inner.enabled(metadata)
    }
    fn log(&self, record: &Record) {
        self.inner.log(record)
    }
    fn flush(&self) {
        self.inner.flush()
    }
}

pub struct FileLogSink {
    path: Cow<'static, Path>,
    output: Mutex<File>,
}

#[allow(dead_code)]
impl FileLogSink {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let pathcow = Cow::Owned(path.as_ref().to_owned());
        let output = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let output = Mutex::new(output);
        Ok(Self {
            path: pathcow,
            output,
        })
    }
}

impl LogSink for FileLogSink {
    fn key(&self) -> SinkKey {
        let name = self.path.to_string_lossy();
        SinkKey::dynamic(format!("FileLogSink({})", name))
    }
    fn log<'a>(&self, msg: std::fmt::Arguments<'a>) -> crate::DynResult<()> {
        let mut lock = match self.output.lock() {
            Ok(l) => l,
            Err(e) => e.into_inner(),
        };
        writeln!(lock, "{}", msg)?;
        Ok(())
    }
    fn flush(&self) -> crate::DynResult<()> {
        let mut lock = match self.output.lock() {
            Ok(l) => l,
            Err(e) => e.into_inner(),
        };
        lock.flush()?;
        Ok(())
    }
}

pub struct StdoutLogSink {}
impl StdoutLogSink {
    pub fn new() -> Self {
        Self {}
    }
}
impl LogSink for StdoutLogSink {
    fn key(&self) -> SinkKey {
        SinkKey::fixed("Stdout")
    }
    fn log<'a>(&self, msg: std::fmt::Arguments<'a>) -> crate::DynResult<()> {
        println!("{}", msg);
        Ok(())
    }
    fn flush(&self) -> crate::DynResult<()> {
        std::io::stdout().flush()?;
        Ok(())
    }
}
pub struct StderrLogSink {}
impl StderrLogSink {
    pub fn new() -> Self {
        Self {}
    }
}
impl LogSink for StderrLogSink {
    fn key(&self) -> SinkKey {
        SinkKey::fixed("Stderr")
    }
    fn log<'a>(&self, msg: std::fmt::Arguments<'a>) -> crate::DynResult<()> {
        eprintln!("{}", msg);
        Ok(())
    }
    fn flush(&self) -> crate::DynResult<()> {
        std::io::stderr().flush()?;
        Ok(())
    }
}

pub struct WebLogSink<T> {
    interface: web_view::Handle<T>,
}

impl<T> WebLogSink<T> {
    pub fn new(interface: web_view::Handle<T>) -> Self {
        Self { interface }
    }
}

impl<T> LogSink for WebLogSink<T> {
    fn key(&self) -> SinkKey {
        SinkKey::fixed("WebLogger")
    }
    fn log<'a>(&self, msg: std::fmt::Arguments<'a>) -> crate::DynResult<()> {
        let cmd = format!("frontend_interface.mylog('{}')", msg);
        self.interface.dispatch(move |wv| wv.eval(&cmd))?;
        Ok(())
    }
    fn flush(&self) -> crate::DynResult<()> {
        Ok(())
    }
}
