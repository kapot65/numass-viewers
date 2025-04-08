#![warn(clippy::all, rust_2018_idioms)]
use std::{path::PathBuf, sync::Arc};

use app::ProcessingStatus;
use egui::mutex::Mutex;
use processing::{utils::events_to_histogram, viewer::EMPTY_POINT};

use processing::{
    histogram::HistogramParams, postprocess::PostProcessParams, process::ProcessParams,
    viewer::PointState,
};

pub mod app;
pub mod bundle_viewer;
pub mod filtered_viewer;
pub mod point_viewer;
pub mod trigger_viewer;

/// Increment processed files counter and reset it if it is finished.
pub fn inc_status(status: Arc<Mutex<ProcessingStatus>>) {
    let mut status = status.lock();
    status.processed += 1;
    if status.processed == status.total {
        *status = ProcessingStatus {
            running: false,
            total: 0,
            processed: 0,
        }
    }
}

#[cfg(target_arch = "wasm32")]
use gloo::worker::oneshot::oneshot;

#[cfg(target_arch = "wasm32")]
#[oneshot]
pub async fn PointProcessor(
    args: (PathBuf, ProcessParams, PostProcessParams, HistogramParams),
) -> Option<PointState> {
    let (filepath, process, post_process, histogram) = args;
    process_point(filepath, process, post_process, histogram).await
}

pub async fn process_point(
    filepath: PathBuf,
    process: ProcessParams,
    post_process: PostProcessParams,
    histogram: HistogramParams,
) -> Option<PointState> {
    let modified = processing::storage::load_modified_time(filepath.clone()).await; // TODO: remove clone

    let events = processing::storage::process_point(&filepath, &process, Some(&post_process)).await;

    events.map(|(_, events)| {
        if let Some((events, preprocess)) = events {

            let histogram = events_to_histogram(events, histogram);

            let counts = Some(histogram.events_all(None));

            PointState {
                opened: true,
                histogram: Some(histogram),
                preprocess: Some(preprocess),
                modified,
                counts,
            }
        } else {
            EMPTY_POINT
        }
    })
}
