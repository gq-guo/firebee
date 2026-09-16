use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use crate::core::http::{execute, HttpError};
use crate::core::models::{Request, ResponseMeta};

pub struct Job {
    pub id: u64,
    pub request: Request,
    pub timeout: Duration,
    pub cancel: tokio::sync::watch::Receiver<bool>,
}

pub struct JobResult {
    pub id: u64,
    pub result: Result<ResponseMeta, HttpError>,
}

/// 专用线程：持有 tokio runtime，每个 Job spawn 一个异步任务（支持并发与取消）。
pub fn spawn_worker(job_rx: Receiver<Job>, result_tx: Sender<JobResult>) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");
        while let Ok(job) = job_rx.recv() {
            let tx = result_tx.clone();
            rt.spawn(async move {
                let result = execute(&job.request, job.timeout, job.cancel).await;
                let _ = tx.send(JobResult { id: job.id, result });
            });
        }
    });
}
