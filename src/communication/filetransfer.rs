use crate::communication::FriendCode;
use crate::communication::*;
use crate::communication::networking::PublicAddr;
use crate::MyResult;
use std::cell::Cell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{atomic::AtomicBool, atomic::Ordering, Arc, RwLock};
use std::thread::JoinHandle;

use std::io;
use std::io::{Read, Seek, SeekFrom};

use url::Url;

const BUFFER_SIZE: usize = 1024 * 1024 * 16;

pub struct FileTransferHost {
    url: String,
    die_flag: Arc<AtomicBool>,
    file_size: u64,
    address: PublicAddr,
    thread_handle: Cell<Option<JoinHandle<()>>>,
}

impl FileTransferHost {
    pub fn new(url: String) -> MyResult<FileTransferHost> {
        let stripped_url = Url::parse(&url)?
            .to_file_path()
            .map_err(|_| format!("Could not parse path from url {:?}", url))?;
        let die_flag = Arc::new(AtomicBool::new(false));
        let listener = random_listener(40_000, 41_000)?;
        let local_addr = listener.local_addr()?;
        let address = PublicAddr::request_public(local_addr)?;

        let file = OpenOptions::new().read(true).open(stripped_url)?;
        let file_size = file.metadata()?.len();
        let thread_handle = Cell::new(Some(file_transfer_host_thread(
            listener,
            file,
            file_size,
            Arc::clone(&die_flag),
        )?));
        Ok(FileTransferHost {
            url,
            die_flag,
            file_size,
            address,
            thread_handle,
        })
    }
    pub fn transfer_code(&self) -> FriendCode {
        FriendCode::from_addr(self.address.addr())
    }
    pub fn file_size(&self) -> u64 {
        self.file_size
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    fn stop(&mut self) -> MyResult<()> {
        self.die_flag.store(true, Ordering::SeqCst);
        let thread_handle = self
            .thread_handle
            .replace(None)
            .ok_or("This should be unreachable?")?;
        thread_handle.join().map_err(|e| {
            if let Some(serr) = e.downcast_ref::<String>() {
                format!("File transfer panic: String {{ {} }}", serr)
            } else if let Some(ioerr) = e.downcast_ref::<io::Error>() {
                format!("File transfer panic: IoError: {{ {} }}", ioerr)
            } else {
                "File transfer panic: Unknown".to_owned()
            }
        })?;
        Ok(())
    }
}

impl Drop for FileTransferHost {
    fn drop(&mut self) {
        self.stop().unwrap()
    }
}

fn file_transfer_host_thread(
    listener: TcpListener,
    mut file: File,
    file_size: u64,
    die_flag: Arc<AtomicBool>,
) -> MyResult<JoinHandle<()>> {
    let mut connections: Vec<TcpStream> = Vec::new();
    let mut positions: HashMap<SocketAddr, u64> = HashMap::new();
    let mut buffer: Vec<u8> = Vec::with_capacity(BUFFER_SIZE);
    let handle = thread::Builder::new().name("FTH thread".to_owned()).spawn(move || loop {
        if die_flag.load(Ordering::SeqCst) {
            return;
        }
        match listener.accept() {
            Ok((stream, addr)) => {
                connections.push(stream);
                positions.insert(addr, 0);
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {}
            e => {
                e.unwrap();
            }
        }
        connections = connections
            .into_iter()
            .filter(|mut c| {
                let pos = positions.get_mut(&c.peer_addr().unwrap()).unwrap();
                if *pos >= file_size {
                    return false;
                }

                let bytes_left = file_size - *pos;
                let buffer_size = (BUFFER_SIZE as u64).min(bytes_left);
                buffer.resize(buffer_size as usize, 0);
                file.seek(SeekFrom::Start(*pos as u64)).unwrap();
                let read = file.read(&mut buffer).unwrap();
                c.write_all(&buffer[..read]).unwrap();
                *pos += read as u64;

                true
            })
            .collect();
        if connections.is_empty() {
            thread::yield_now();
        }
    })?;

    Ok(handle)
}

pub struct FileTransferClient {
    url: String,
    file_size: u64,
    progress: Arc<RwLock<u64>>,
    local_file_path: PathBuf,
    finished_flag: Arc<AtomicBool>,
    handle: Cell<Option<JoinHandle<()>>>,
}

impl FileTransferClient {
    pub fn new(
        url: String,
        file_size: u64,
        remote_code: FriendCode,
    ) -> MyResult<FileTransferClient> {
        let path = Url::parse(&url)?
            .to_file_path()
            .map_err(|_| format!("Invalid url {:?}", url))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| format!("Invalid url {}", url))?;
        let local_file_path = PathBuf::from(file_name);
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(file_name)?;
        let finished_flag = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(RwLock::new(0));
        let handle = Cell::new(Some(file_transfer_client_thread(
            file_size,
            Arc::clone(&progress),
            remote_code.as_addr(),
            file,
            Arc::clone(&finished_flag),
        )?));
        Ok(FileTransferClient {
            url,
            file_size,
            progress,
            local_file_path,

            finished_flag,
            handle,
        })
    }

    pub fn is_finished(&self) -> bool {
        self.finished_flag.load(Ordering::SeqCst)
    }

    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn local_file_path(&self) -> &Path {
        &self.local_file_path
    }

    pub fn progress_bytes(&self) -> u64 {
        if self.is_finished() {
            self.file_size
        } else {
            *self.progress.read().unwrap()
        }
    }

    pub fn progress(&self) -> f64 {
        (self.progress_bytes() as f64) / (self.file_size as f64)
    }

    fn stop(&mut self) -> MyResult<()> {
        let thread_handle = self
            .handle
            .replace(None)
            .ok_or("This should be unreachable?")?;
        thread_handle.join().map_err(|e| {
            if let Some(serr) = e.downcast_ref::<String>() {
                format!("File transfer panic: String {{ {} }}", serr)
            } else if let Some(ioerr) = e.downcast_ref::<io::Error>() {
                format!("File transfer panic: IoError: {{ {} }}", ioerr)
            } else {
                "File transfer panic: Unknown".to_owned()
            }
        })?;
        Ok(())
    }
}

impl Drop for FileTransferClient {
    fn drop(&mut self) {
        self.stop().unwrap();
    }
}

fn file_transfer_client_thread(
    file_size: u64,
    progress: Arc<RwLock<u64>>,
    remote: SocketAddr,
    mut file: File,
    finished_flag: Arc<AtomicBool>,
) -> MyResult<JoinHandle<()>> {
    let mut connection = TcpStream::connect_timeout(&remote, Duration::from_secs(30))?;
    connection.set_nonblocking(true)?;
    let mut buffer: Vec<u8> = Vec::with_capacity(BUFFER_SIZE);

    let handle = thread::Builder::new().name("FTC thread".to_owned()).spawn(move || {
        loop {
            let cur_pos = *progress.read().unwrap();
            if cur_pos >= file_size {
                break;
            }
            let buffer_size = BUFFER_SIZE.min((file_size - cur_pos) as usize);
            buffer.resize(buffer_size, 0);
            let read = match connection.read(&mut buffer) {
                Ok(b) => b, 
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => 0, 
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => 0, 
                e => e.unwrap(),
            };
            if read > 0 {
                let read_slice = &buffer[..read];
                file.write_all(read_slice).unwrap();
                file.flush().unwrap();
                *progress.write().unwrap() = cur_pos + read as u64;
            }
            thread::yield_now();
        }
        finished_flag.store(true, Ordering::SeqCst);
    })?;
    Ok(handle)
}
