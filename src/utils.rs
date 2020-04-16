pub fn array_copy<Itm: Copy, Col: Default + AsRef<[Itm]> + AsMut<[Itm]>>(src: &[Itm]) -> Col {
    let mut retvl = Col::default();
    let retvl_ref = retvl.as_mut();
    let len = retvl_ref.len();
    retvl_ref.copy_from_slice(&src[0..len]);
    retvl
}
pub trait AbsSub {
    type Output;
    fn abs_sub(self, other: Self) -> Self::Output;
}

impl AbsSub for u64 {
    type Output = Self;
    fn abs_sub(self, other: Self) -> Self {
        if self > other {
            self - other
        } else {
            other - self
        }
    }
}

pub fn generate_spawn<
    T: Send + 'static,
    Fut: std::future::Future<Output = T> + 'static,
    Gen: FnOnce() -> Fut + Send + 'static,
>(
    gen: Gen,
) -> tokio::task::JoinHandle<T> {
    tokio::task::spawn_local(gen())
}

pub struct MyWaker {
    wake_flag: std::sync::atomic::AtomicBool,
    thread_id: std::thread::Thread,
}
impl MyWaker {
    pub fn new() -> Self {
        Self {
            wake_flag: std::sync::atomic::AtomicBool::new(false),
            thread_id: std::thread::current(),
        }
    }
    pub fn has_woken(&self) -> bool {
        self.wake_flag.load(std::sync::atomic::Ordering::SeqCst)
    }
}
use std::sync::Arc;

impl futures::task::ArcWake for MyWaker {
    fn wake(self: Arc<Self>) {
        Self::wake_by_ref(&self)
    }
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self
            .wake_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
        arc_self.thread_id.unpark();
    }
}

pub fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    let mut pinned = Box::pin(fut);
    loop {
        let mywaker = Arc::new(MyWaker::new());
        let waker = futures::task::waker(Arc::clone(&mywaker));
        let mut ctx = std::task::Context::from_waker(&waker);
        if let std::task::Poll::Ready(val) = pinned.as_mut().poll(&mut ctx) {
            break val;
        } else if mywaker.has_woken() {
            std::thread::sleep(std::time::Duration::from_millis(1));
            std::thread::yield_now();
        } else {
            std::thread::park_timeout(std::time::Duration::from_millis(100));
        }
    }
}
