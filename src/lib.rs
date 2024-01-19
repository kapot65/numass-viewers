#![warn(clippy::all, rust_2018_idioms)]
use std::{path::PathBuf, sync::Arc};

use app::ProcessingStatus;
use egui::mutex::Mutex;
use processing::{numass::{NumassMeta, Reply, ExternalMeta}, utils::events_to_histogram};

use processing::{
    histogram::HistogramParams, 
    process::ProcessParams,
    postprocess::PostProcessParams,
    viewer::PointState
};

pub mod app;
pub mod hyperlink;
pub mod filtered_viewer;
pub mod point_viewer;
pub mod bundle_viewer;
pub mod widgets;

/// Increment processed files counter and reset it if it is finished.
pub fn inc_status(status: Arc<Mutex<ProcessingStatus>>) {
    let mut status = status.lock();
    status.processed += 1;
    if status.processed == status.total {
        *status = ProcessingStatus {
            running: false,
            total: 0,
            processed: 0
        }
    }
}

#[cfg(target_arch = "wasm32")]
use gloo::worker::oneshot::oneshot;

#[cfg(target_arch = "wasm32")]
#[oneshot]
pub async fn PointProcessor(args: (PathBuf, ProcessParams, PostProcessParams, HistogramParams)) -> Option<PointState> {
    let (filepath, process, post_process, histogram) = args;
    process_point(filepath, process, post_process, histogram).await
}

pub async fn process_point(filepath: PathBuf, process: ProcessParams, post_process: PostProcessParams, histogram: HistogramParams) -> Option<PointState> {
    
    let events = processing::storage::process_point(&filepath, &process).await;

    events.map(|(meta, events)| {
        if let Some(events) = events {

            let processed = processing::postprocess::post_process(events, &post_process);

            let histogram = events_to_histogram(processed, histogram);

            let counts = Some(histogram.events_all(None));

            // extract voltage from meta
            let voltage = if let NumassMeta::Reply(Reply::AcquirePoint {
                external_meta: Some(ExternalMeta {hv1_value: Some(voltage), ..}), .. 
            }) = &meta {
                    Some(*voltage)
            } else {
                None
            };

            // extract start time from meta
            let start_time = if let NumassMeta::Reply(Reply::AcquirePoint {
                start_time, .. 
            }) = &meta {
                Some(start_time.to_owned())
            } else {
                None
            };

            PointState {
                opened: true,
                histogram: Some(histogram),
                start_time,
                voltage,
                counts
            }
        } else {
            PointState {
                opened: false,
                histogram: None,
                start_time: None,
                voltage: None,
                counts: None
            }
        }
    })
}