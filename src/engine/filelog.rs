//! 引擎日志落盘：追加写 + 简单大小轮转。
//!
//! 打开失败后静默停写，绝不让日志问题影响转发主流程。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

/// 日志文件轮转阈值：超过后 `proxyone.log` 改名为 `proxyone.log.1` 重新开始。
const LOG_ROTATE_BYTES: u64 = 5 * 1024 * 1024;

pub(super) struct FileLogger {
    file: Option<File>,
    path: PathBuf,
}

impl FileLogger {
    /// 打开日志文件；`dir` 为 `None`（目录不可用）或打开失败时静默停写。
    pub(super) fn open(dir: Option<PathBuf>) -> Self {
        let path = dir.map(|d| d.join("proxyone.log"));
        let file = path.as_ref().and_then(|p| {
            std::fs::create_dir_all(p.parent()?).ok();
            OpenOptions::new().create(true).append(true).open(p).ok()
        });
        if file.is_none() {
            eprintln!("日志文件不可用，仅输出到内存/控制台");
        }
        Self {
            file,
            path: path.unwrap_or_default(),
        }
    }

    pub(super) fn write_line(&mut self, line: &str) {
        let Some(f) = self.file.as_mut() else { return };
        if f.metadata()
            .map(|m| m.len() > LOG_ROTATE_BYTES)
            .unwrap_or(false)
        {
            self.rotate();
        }
        let Some(f) = self.file.as_mut() else { return };
        let _ = f.write_all(line.as_bytes());
        let _ = f.write_all(b"\n");
    }

    fn rotate(&mut self) {
        self.file = None;
        let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .ok();
    }
}
