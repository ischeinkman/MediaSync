use futures::stream::SelectAll;
use futures::stream::{LocalBoxStream, StreamExt};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{self};
use tokio::sync::oneshot;
use tokio::sync::RwLock as TaskRwLock;
use web_view::{Handle, WebView};
enum WebviewThreadHandle {
    Tokio(tokio::task::JoinHandle<()>),
    Thread(std::thread::JoinHandle<()>),
}

struct WebviewGlobalContext {
    ctx_handle: mpsc::UnboundedSender<TaskGenerator>,
    #[allow(unused)]
    loop_handle: WebviewThreadHandle,
}

lazy_static::lazy_static! {
    static ref WEBVIEW_CONTEXT : Arc<TaskRwLock<Option<WebviewGlobalContext>>> = {
        Arc::new(TaskRwLock::new(None))
    };
}

impl WebviewGlobalContext {
    async fn get_global<'a>() -> impl std::ops::Deref<Target = Option<WebviewGlobalContext>> + 'a {
        let optlock: tokio::sync::RwLockReadGuard<'_, Option<WebviewGlobalContext>> =
            WEBVIEW_CONTEXT.read().await;
        let is_initialized = optlock.is_some();
        if is_initialized {
            optlock
        } else {
            drop(optlock);
            let mut writehandle = WEBVIEW_CONTEXT.write().await;
            if !writehandle.is_some() {
                let new_ctx = WebviewGlobalContext::spawn_newthread().await;
                *writehandle = Some(new_ctx);
            }
            drop(writehandle);
            WEBVIEW_CONTEXT.read().await
        }
    }
    #[allow(unused)]
    async fn spawn_newthread() -> WebviewGlobalContext {
        let (retsnd, retrecv) = oneshot::channel();
        let spawn_closure = move || async move {
            let (sender_spawner, mut task_reciever) = mpsc::unbounded_channel::<TaskGenerator>();
            let ctx_hndl = sender_spawner;

            let mut tasks = SelectAll::new();
            if let Err(_e) = retsnd.send(ctx_hndl) {
                todo!()
            }
            loop {
                if let Ok(gen) = task_reciever.try_recv() {
                    let ntask = gen();
                    tasks.push(ntask);
                }
                tasks.next().await;
            }
        };
        let tokio_handle = tokio::runtime::Handle::try_current();
        let loop_handle = WebviewThreadHandle::Thread(std::thread::spawn(move || {
            let fut = spawn_closure();
            match tokio_handle {
                Ok(h) => h.block_on(fut),
                Err(_) => futures::executor::block_on(fut),
            }
        }));
        let ctx_handle = retrecv.await.unwrap();
        WebviewGlobalContext {
            ctx_handle,
            loop_handle,
        }
    }
    #[allow(unused)]
    async fn spawn_local() -> WebviewGlobalContext {
        let (retsnd, retrecv) = oneshot::channel();
        let spawn_closure = move || async move {
            let (sender_spawner, mut task_reciever) = mpsc::unbounded_channel::<TaskGenerator>();
            let ctx_hndl = sender_spawner;

            let mut tasks = SelectAll::new();
            if let Err(_e) = retsnd.send(ctx_hndl) {
                todo!()
            }
            loop {
                if let Ok(gen) = task_reciever.try_recv() {
                    let ntask = gen();
                    tasks.push(ntask);
                }
                tasks.next().await;
            }
        };
        let loop_handle = WebviewThreadHandle::Tokio(tokio::task::spawn_local(spawn_closure()));
        let ctx_handle = retrecv.await.unwrap();
        WebviewGlobalContext {
            ctx_handle,
            loop_handle,
        }
    }
}
pub async fn generate_webview<
    T: 'static,
    Fut: Future<Output = WebView<'static, T>>,
    F: FnOnce() -> Fut + Send + 'static,
>(
    gen: F,
) -> Handle<T> {
    let ctx_handle = WebviewGlobalContext::get_global().await;
    let ctx_handle = ctx_handle.as_ref().unwrap();
    let (hndl_send, hndl_recv) = oneshot::channel();
    let raw_taskgen = TaskGenerator::from(Box::new(move || {
        futures::stream::once(async move {
            let wv = gen().await;
            if let Err(_e) = hndl_send.send(wv.handle()) {}
            webview_update_task(wv)
        })
        .flatten()
        .boxed_local()
    }));
    if let Err(_e) = ctx_handle.ctx_handle.send(raw_taskgen) {
        panic!("ERROR: WebviewCTX aborted!");
    }
    hndl_recv.await.unwrap()
}

type TaskGenerator = Box<dyn FnOnce() -> Task + Send>;
type Task = LocalBoxStream<'static, ()>;

fn webview_update_task<T: 'static>(view: WebView<'static, T>) -> Task {
    const FPS: u64 = 1000;
    const NANOS_PER_SECOND: u64 = 1_000_000_000;
    const NANOS_PER_FRAME: u64 = NANOS_PER_SECOND / FPS;

    let throttle_time = Duration::from_nanos(NANOS_PER_FRAME);
    println!("Webview Time per frame: {}", throttle_time.as_secs_f64());
    let step_loop = futures::stream::unfold(view, |mut webview| {
        let res = webview.step();
        let res = match res {
            Some(Ok(_)) => Some(((), webview)),
            Some(e) => Some((e.unwrap(), webview)),
            None => None,
        };
        async { res }
    });
    if tokio::runtime::Handle::try_current().is_ok() {
        log::info!("Webview throttle via tokio.");
        let throttled = tokio::time::throttle(throttle_time, step_loop);
        throttled.boxed_local()
    } else {
        log::info!("Webview throttle via thread.");
        step_loop
            .map(move |_| {
                std::thread::sleep(throttle_time);
            })
            .boxed_local()
    }
}
