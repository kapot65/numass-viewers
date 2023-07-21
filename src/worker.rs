use std::{
    collections::{BTreeMap, HashMap}, cell::RefCell, 
    sync::Arc, time::SystemTime
};

use egui::mutex::Mutex;
use gloo::{
    worker::{HandlerId, Worker, WorkerScope, Spawnable, WorkerBridge}, 
    net::http::Request
};
use serde::{Serialize, Deserialize};

use processing::{
    histogram::PointHistogram, ProcessParams,
    viewer::{PointState, ViewerState}, 
    numass::NumassMeta
};
use crate::app::ProcessingStatus;

pub struct WebWorker {

}

#[derive(Debug, Serialize, Deserialize)]
pub enum WebWorkerRequests {
    CalcHist {
        key: String,
        amplitudes_raw: Vec<u8>,
        state: ViewerState
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum WebWorkerResponses {
    CalcHist {
        key: String,
        histogram: PointHistogram
    }
}

impl Worker for WebWorker {
    type Input = WebWorkerRequests;
    type Message = ();
    type Output = WebWorkerResponses;

    fn create(_scope: &WorkerScope<Self>) -> Self {
        Self {}
    }

    fn update(&mut self, _scope: &WorkerScope<Self>, _msg: Self::Message) {

    }

    fn received(&mut self, scope: &WorkerScope<Self>, msg: Self::Input, id: HandlerId) {
        match msg {
            WebWorkerRequests::CalcHist { 
                key,
                amplitudes_raw, 
                state: processing,
            } => {
                let amplitudes = rmp_serde::from_slice::<Option<BTreeMap<u64, BTreeMap<usize, f32>>>>(&amplitudes_raw).unwrap().unwrap();
                let processed = processing::post_process(amplitudes, &processing.post_process);
                let  histogram = processing::amplitudes_to_histogram(processed, processing.histogram);
                scope.respond(id, WebWorkerResponses::CalcHist {
                    key,
                    histogram
                })
            }
        }
    }
}

pub struct WebThreadPool {
    current: RefCell<usize>,
    threads: Vec<WorkerBridge<WebWorker>>,
    status: Arc<Mutex<ProcessingStatus>>,
    files_cache: Arc<Mutex<HashMap<String, CachedFile>>>
}

struct CachedFile {
    process: ProcessParams,
    modified: SystemTime,
    meta: NumassMeta,
    raw_amplitudes: Vec<u8>,
}


impl WebThreadPool {

    pub fn new(
        state: Arc<Mutex<BTreeMap<String, PointState>>>,
        status: Arc<Mutex<ProcessingStatus>>,
    ) -> Self {

        console_error_panic_hook::set_once();

        let files_cache = Arc::new(Mutex::new(HashMap::<String, CachedFile>::new()));
        let concurrency = gloo::utils::window().navigator().hardware_concurrency() as usize - 1;

        let threads = (0..concurrency).map(|_| {
            let status = Arc::clone(&status);
            let state = Arc::clone(&state);
            let files_cache = Arc::clone(&files_cache);

            crate::worker::WebWorker::spawner()
                .callback(move |resp| {

                    match resp {
                        crate::worker::WebWorkerResponses::CalcHist { 
                            key,
                            histogram 
                        } => {

                            let meta = files_cache.lock().get(&key).map(|file_cache| {
                                file_cache.meta.clone()
                            });

                            let mut conf = state.lock();
                            let counts = Some(histogram.events_all(None));

                            conf.insert(
                                key,
                                PointState {
                                    opened: true,
                                    histogram: Some(histogram),
                                    counts,
                                    meta, // TODO: handle meta
                                },
                            );
                        }
                    }

                    crate::inc_status(Arc::clone(&status));
                        
                })
                .spawn("./worker.js")
        }).collect::<Vec<_>>();

        Self {
            current: RefCell::new(0),
            files_cache,
            status,
            threads
        }
    }

    pub fn send(&self, cmd: WebWorkerRequests) {
        if self.current.take() == self.threads.len() {
            *self.current.borrow_mut() = 0;
        }
        self.threads[self.current.take()].send(cmd);
        *self.current.borrow_mut() += 1;
    }

    pub async fn process_point(&self, filepath: String, state: ViewerState) {

        // get file modification time
        let modified = Request::get(&format!("/api/modified{filepath}"))
            .send()
            .await
            .unwrap()
            .json::<SystemTime>()
            .await
            .unwrap();

        // search and validate file in cache
        let cached = {
            let files_cache = self.files_cache.lock();
            if let Some(entry) = files_cache.get(&filepath) {
                if entry.process == state.process &&
                   entry.modified >= modified {
                    Some(entry.raw_amplitudes.clone())
                } else {
                    None
                }
            } else {
                None
            }
        };

        // get raw amplitudes or fetch from server
        let amplitudes_raw = if let Some(out) = cached {
            Some(out)
        } else {

            let meta = Request::get(&format!("/api/meta{filepath}"))
            .send()
            .await
            .unwrap()
            .json::<Option<NumassMeta>>()
            .await
            .unwrap();

            if let Some(NumassMeta::Reply(processing::numass::Reply::AcquirePoint { .. })) = &meta {

                let amplitudes_raw = Request::post(&format!("/api/process{filepath}"))
                .json(&state.process).unwrap()
                .send()
                .await
                .unwrap()
                .binary()
                .await
                .unwrap();

                self.files_cache.lock().insert(
                    filepath.clone(), CachedFile { 
                        process: state.process,
                        modified, 
                        meta: meta.unwrap(),
                        raw_amplitudes: amplitudes_raw.clone() 
                    }
                );

                Some(amplitudes_raw)

            } else {
                None
            }
        };

        // send to worker
        if let Some(amplitudes_raw) = amplitudes_raw {
            self.send(WebWorkerRequests::CalcHist {
                key: filepath.clone(),
                amplitudes_raw,
                state
            });
        } else {
            crate::inc_status(Arc::clone(&self.status));
        }
    }
}