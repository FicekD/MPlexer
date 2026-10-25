use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use log::{Level, Log, Metadata, Record};

const MAX_LINES: usize = 500;

#[derive(Clone)]
pub struct LogLine {
    pub time: String,
    pub level: Level,
    pub target: String,
    pub msg: String,
}

pub type LogBuffer = Arc<Mutex<VecDeque<LogLine>>>;

struct TeeLogger {
    inner: env_logger::Logger,
    buffer: LogBuffer,
}

impl Log for TeeLogger {
    fn enabled(&self, meta: &Metadata) -> bool {
        self.inner.enabled(meta)
    }

    fn log(&self, record: &Record) {
        if !self.inner.enabled(record.metadata()) {
            return;
        }
        self.inner.log(record);

        let mut buf = self.buffer.lock().unwrap();
        if buf.len() >= MAX_LINES {
            buf.pop_front();
        }
        buf.push_back(LogLine {
            time: clock(),
            level: record.level(),
            target: record.target().to_string(),
            msg: record.args().to_string(),
        });
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

pub fn init() -> LogBuffer {
    let inner = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,mplexer=debug"),
    )
    .build();
    let level = inner.filter();

    let buffer: LogBuffer = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_LINES)));
    let logger = TeeLogger {
        inner,
        buffer: buffer.clone(),
    };
    log::set_boxed_logger(Box::new(logger)).expect("logger already installed");
    log::set_max_level(level);
    buffer
}

fn clock() -> String {
    jiff::Zoned::now().strftime("%H:%M:%S").to_string()
}
