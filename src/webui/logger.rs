use log::{Record, Metadata};
pub struct WebLogger<T> {
    interface: web_view::Handle<T>,
}

impl<'a, T> From<&'a web_view::WebView<'static, T>> for WebLogger<T> {
    fn from(view: &'a web_view::WebView<'static, T>) -> Self {
        Self {
            interface: view.handle(),
        }
    }
}
impl<T> From<web_view::Handle<T>> for WebLogger<T> {
    fn from(interface: web_view::Handle<T>) -> Self {
        Self { interface }
    }
}

impl<T> log::Log for WebLogger<T> {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }
    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        if let Some(pt) = record.module_path() {
            if !pt.contains("vlcsync") {
                return;
            }
        }
        let cmdlog = crate::cmdui::CmdUi {};
        cmdlog.log(record);
        let cmd = format!(
            "frontend_interface.mylog('[{}] : {}')",
            record.level(),
            record.args()
        );
        self.interface.dispatch(move |wv| {
            wv.eval(&cmd).unwrap();
            Ok(())
        }).unwrap();
    }
    fn flush(&self) {
    }
}