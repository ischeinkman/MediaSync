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

#[derive(PartialEq)]
pub enum AllowOnlyOneError<T: PartialEq> {
    NoneFound,
    GotSecond(T),
}
impl<T: PartialEq> std::fmt::Debug for AllowOnlyOneError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Found multiple values in stream.")
    }
}

impl<T: PartialEq> std::fmt::Display for AllowOnlyOneError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Found multiple values in stream.")
    }
}

impl<T: PartialEq> std::error::Error for AllowOnlyOneError<T> {}

pub struct AllowOnlyOne<T: PartialEq> {
    state: Result<T, AllowOnlyOneError<T>>,
}

impl<T: PartialEq> AsRef<Result<T, AllowOnlyOneError<T>>> for AllowOnlyOne<T> {
    fn as_ref(&self) -> &Result<T, AllowOnlyOneError<T>> {
        &self.state
    }
}

impl<T: PartialEq> From<AllowOnlyOne<T>> for Result<T, AllowOnlyOneError<T>> {
    fn from(wrapped: AllowOnlyOne<T>) -> Self {
        wrapped.state
    }
}

impl<T: PartialEq> AllowOnlyOne<T> {
    pub fn new() -> Self {
        Self {
            state: Err(AllowOnlyOneError::NoneFound),
        }
    }
    pub fn from_first(cur: T) -> Self {
        Self { state: Ok(cur) }
    }
    pub fn from_second(second: T) -> Self {
        Self {
            state: Err(AllowOnlyOneError::GotSecond(second)),
        }
    }
    pub fn has_second(&self) -> bool {
        if let Err(AllowOnlyOneError::GotSecond(_)) = self.state {
            true
        } else {
            false
        }
    }
    pub fn push_next(&mut self, next: T) {
        let nxt = std::mem::take(self).with_next(next);
        std::mem::replace(self, nxt);
    }
    pub fn with_next(self, next: T) -> Self {
        match self.state {
            Ok(cur) => {
                if cur == next {
                    Self::from_first(cur)
                } else {
                    Self::from_second(next)
                }
            }
            Err(AllowOnlyOneError::NoneFound) => Self::from_first(next),
            Err(AllowOnlyOneError::GotSecond(err)) => Self::from_second(err),
        }
    }

    pub fn into_res(self) -> Result<T, AllowOnlyOneError<T>> {
        self.state
    }
}

impl<T: PartialEq> Default for AllowOnlyOne<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: PartialEq> std::iter::Extend<T> for AllowOnlyOne<T> {
    fn extend<Iter: IntoIterator<Item = T>>(&mut self, iter: Iter) {
        if self.has_second() {
            return;
        }
        for item in iter {
            self.push_next(item);
        }
    }
}

impl<T: PartialEq> std::iter::FromIterator<T> for AllowOnlyOne<T> {
    fn from_iter<Iter: IntoIterator<Item = T>>(iter: Iter) -> Self {
        let mut retvl = Self::new();
        for itm in iter {
            retvl = retvl.with_next(itm);
            if retvl.has_second() {
                break;
            }
        }
        retvl
    }
}

