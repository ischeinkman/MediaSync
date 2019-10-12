use crate::communication::FriendCodeV4;
use crate::communication::*;
use crate::MyResult;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{atomic::AtomicBool, atomic::Ordering, Arc, RwLock};
use std::thread::JoinHandle;

use std::io;
use std::io::{Read, Seek, SeekFrom};

const BUFFER_SIZE: usize = 1024 * 1024 * 16;

pub struct FileTransferHost {
    url: String,
    die_flag: Arc<AtomicBool>,
    file_size: u64,
    local_address: SocketAddr,
    thread_handle: JoinHandle<()>,
}

impl FileTransferHost {
    pub fn new(url: String) -> MyResult<FileTransferHost> {
        let stripped_url = url.trim_start_matches("file://");
        let die_flag = Arc::new(AtomicBool::new(false));
        let listener = open_listener(40_000, 41_000)?;
        let local_address = listener.local_addr()?;
        let file = OpenOptions::new().read(true).open(stripped_url)?;
        let file_size = file.metadata()?.len();
        let thread_handle =
            file_transfer_host_thread(listener, file, file_size, Arc::clone(&die_flag))?;
        Ok(FileTransferHost {
            url,
            die_flag,
            file_size,
            local_address,
            thread_handle,
        })
    }
    pub fn transfer_code(&self) -> MyResult<FriendCodeV4> {
        match self.local_address {
            SocketAddr::V4(v4) => Ok(FriendCodeV4::from_addr(v4)),
            SocketAddr::V6(v6) => Err(format!(
                "Error: friend codes for IPv6 address {:?} is not yet implemented. ",
                v6
            )
            .into()),
        }
    }
    pub fn file_size(&self) -> u64 {
        self.file_size
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn stop(self) -> MyResult<()> {
        self.die_flag.store(true, Ordering::Acquire);
        self.thread_handle.join().map_err(|e| {
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

fn open_listener(min_port: u16, max_port: u16) -> MyResult<TcpListener> {
    let listener = random_listener(min_port, max_port)?;
    Ok(listener)
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
    let handle = thread::spawn(move || loop {
        if die_flag.load(Ordering::Acquire) {
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
        } else {
            thread::sleep(Duration::from_millis(5));
        }
    });

    Ok(handle)
}

pub struct FileTransferClient {
    url: String,
    file_size: u64,
    progress: Arc<RwLock<u64>>,
    local_file_path: PathBuf,
    finished_flag: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl FileTransferClient {
    pub fn new(
        url: String,
        file_size: u64,
        remote_code: FriendCodeV4,
    ) -> MyResult<FileTransferClient> {
        let path = std::path::Path::new(url.trim_start_matches("file://"));
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
        let handle = file_transfer_client_thread(
            file_size,
            Arc::clone(&progress),
            remote_code.as_addr().into(),
            file,
            Arc::clone(&finished_flag),
        )?;
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
        self.finished_flag.load(Ordering::Acquire)
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
}

fn file_transfer_client_thread(
    file_size: u64,
    progress: Arc<RwLock<u64>>,
    remote: SocketAddr,
    mut file: File,
    finished_flag: Arc<AtomicBool>,
) -> MyResult<JoinHandle<()>> {
    let mut connection = TcpStream::connect(remote)?;
    let mut buffer: Vec<u8> = Vec::with_capacity(BUFFER_SIZE);

    let handle = thread::spawn(move || {
        loop {
            let mut cur_pos = progress.write().unwrap();
            if *cur_pos >= file_size {
                break;
            }
            let buffer_size = BUFFER_SIZE.min((file_size - *cur_pos) as usize);
            buffer.resize(buffer_size, 0);
            let read = connection.read(&mut buffer).unwrap();
            let read_slice = &buffer[..read];
            file.write_all(read_slice).unwrap();
            *cur_pos += read as u64;
            thread::sleep(Duration::from_millis(5));
        }
        finished_flag.store(true, Ordering::Acquire);
    });
    Ok(handle)
}
