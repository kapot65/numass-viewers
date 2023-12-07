#![warn(clippy::all, rust_2018_idioms)]

use std::{path::{PathBuf, Path}, sync::Arc};

use app::ProcessingStatus;
use egui::{Ui, mutex::Mutex};
use processing::{numass::{NumassMeta, Reply, ExternalMeta}, events_to_histogram};
use protobuf::Message;

use processing::{histogram::HistogramParams, PostProcessParams, Algorithm, numass::protos::rsb_event, ProcessParams, viewer::PointState};

#[cfg(target_arch = "wasm32")]
use {
    std::io::Cursor,
    gloo::net::http::Request,
    dataforge::{read_df_message_sync, DFMessage},
    processing::NumassAmps
};

#[cfg(not(target_arch = "wasm32"))]
use {
    processing::{numass, extract_events},
    dataforge::{read_df_message, read_df_header_and_meta}
};

pub mod app;
pub mod hyperlink;
pub mod filtered_viewer;
pub mod point_viewer;
pub mod bundle_viewer;

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

pub fn histogram_params_editor(ui: &mut Ui, histogram: &HistogramParams) -> HistogramParams {

    ui.label("Histogram params");
    let mut min = histogram.range.start;
    ui.add(egui::Slider::new(&mut min, -10.0..=400.0).text("left"));
    let mut max = histogram.range.end;
    ui.add(egui::Slider::new(&mut max, -10.0..=400.0).text("right"));
    let mut bins = histogram.bins;
    ui.add(egui::Slider::new(&mut bins, 10..=2000).text("bins"));

    HistogramParams { range: min..max, bins }
}

pub fn post_process_editor(ui: &mut Ui, ctx: &egui::Context, params: &PostProcessParams) -> PostProcessParams {

    ui.label("Postprocessing params");

    let mut use_dead_time = params.use_dead_time;
    let mut effective_dead_time = params.effective_dead_time;

    ui.checkbox(&mut use_dead_time, "use dead time");
    ui.add_enabled(
        use_dead_time,
        egui::Slider::new(&mut effective_dead_time, 0..=30000).text("ns"),
    );

    let mut merge_close_events = params.merge_close_events;
    ui.checkbox(&mut merge_close_events, "merge close events");

    let mut merge_map = params.merge_map;
    ui.collapsing("merge mapping", |ui| {
        egui_extras::TableBuilder::new(ui)
            // .auto_shrink([false, false])
            .columns(egui_extras::Column::initial(15.0), 8)
            .header(20.0, |mut header| {
                header.col(|_| {});
                for idx in 0..7 {
                    header.col(|ui| {
                        ui.label((idx + 1).to_string());
                    });
                }
            })
            .body(|mut body| {
                for ch_1 in 0usize..7 {
                    body.row(20.0, |mut row| {
                        row.col(|ui| {
                            ui.label(format!("{}<", ch_1 + 1));
                        });
                        for ch_2 in 0usize..7 {
                            row.col(|ui| {
                                if ch_1 == ch_2 {
                                    let checkbox =
                                        egui::Checkbox::new(&mut merge_map[ch_1][ch_2], "");
                                    ui.add_enabled(false, checkbox);
                                } else if ui.checkbox(&mut merge_map[ch_1][ch_2], "").changed()
                                    && merge_map[ch_1][ch_2]
                                {
                                    merge_map[ch_2][ch_1] = false;
                                }
                            });
                        }
                    });
                }
            });

        let image = if ctx.style().visuals.dark_mode {
            egui_extras::image::RetainedImage::from_svg_bytes(
                "Detector.drawio.png",
                include_bytes!("../resources/detector_dark.svg"),
            ).unwrap()
        } else {
            egui_extras::image::RetainedImage::from_svg_bytes(
                "Detector.drawio.png",
                include_bytes!("../resources/detector_light.svg"),
            ).unwrap()
        };

        image.show(ui);
    });

    PostProcessParams { 
        use_dead_time,
        effective_dead_time,
        merge_close_events,
        merge_map
    }
}

pub fn process_editor(ui: &mut Ui, params: &ProcessParams) -> ProcessParams {

    let mut algorithm = params.algorithm.to_owned();

    ui.label("Processing params");

    ui.horizontal(|ui| {
        if ui
            .add(egui::RadioButton::new(algorithm == Algorithm::Max, "Max"))
            .clicked()
        {
            algorithm = Algorithm::Max
        }

        if ui
            .add(egui::RadioButton::new(
                matches!(algorithm, Algorithm::Likhovid { .. }),
                "Likhovid",
            ))
            .clicked()
        {
            algorithm = Algorithm::Likhovid { left: 15, right: 36 }
        }

        if ui
            .add(egui::RadioButton::new(
                matches!(algorithm, Algorithm::FirstPeak { .. }),
                "FirstPeak",
            ))
            .clicked()
        {
            algorithm = Algorithm::FirstPeak { threshold: 10, left: 8 }
        }

        if ui
            .add(egui::RadioButton::new(
                matches!(algorithm, Algorithm::Trapezoid { .. }),
                "Trapezoid",
            ))
            .clicked()
        {
            algorithm = Algorithm::Trapezoid { left: 6, center: 0, right: 6 }
        }
    });

    let algorithm = match algorithm {
        Algorithm::Max => {
            algorithm
        }
        Algorithm::Likhovid { left, right } => {
            let mut left = left;
            ui.add(egui::Slider::new(&mut left, 0..=30).text("left"));
            let mut right = right;
            ui.add(egui::Slider::new(&mut right, 0..=40).text("right"));

            Algorithm::Likhovid { left, right }
        }
        Algorithm::FirstPeak { threshold, left } => {

            let mut left = left;
            ui.add(egui::Slider::new(&mut left, 0..=30).text("left"));

            let mut threshold = threshold;
            ui.add(egui::Slider::new(&mut threshold, 0..=400).text("threshold"));
            Algorithm::FirstPeak { threshold, left }
        }

        Algorithm::Trapezoid { left, center, right } => {

            let mut left = left;
            ui.add(egui::Slider::new(&mut left, 0..=32).text("left"));

            let mut center = center;
            ui.add(egui::Slider::new(&mut center, 0..=32).text("center"));

            let mut right = right;
            ui.add(egui::Slider::new(&mut right, 0..=32).text("right"));

            Algorithm::Trapezoid { left, center, right}
        }
    };

    let mut convert_to_kev = params.convert_to_kev;
    ui.checkbox(&mut convert_to_kev, "convert to keV");

    ProcessParams { algorithm, convert_to_kev }
}

#[cfg(target_arch = "wasm32")]
pub fn api_url(prefix: &str, filepath: &Path) -> String {
    // TODO: change to gloo function when it comes out
    let base_url = js_sys::eval("String(new URL(self.location.href).origin)").unwrap().as_string().unwrap();
    format!("{base_url}/{prefix}{}", filepath.to_str().unwrap())
}

pub async fn load_meta(filepath: &Path) -> Option<NumassMeta> {

    #[cfg(target_arch = "wasm32")]
    {
        Request::get(&api_url("api/meta", filepath))
            .send()
            .await
            .unwrap()
            .json::<Option<NumassMeta>>()
            .await
            .unwrap()
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut point_file = tokio::fs::File::open(&filepath).await.unwrap();
        read_df_header_and_meta::<numass::NumassMeta>(&mut point_file).await.map_or(
            None, |(_, meta)| Some(meta))
    }
}

pub async fn load_point(filepath: &Path) -> rsb_event::Point {
    #[cfg(target_arch = "wasm32")]
    {
        let point_data = Request::get(&api_url("files", filepath))
            .send()
            .await
            .unwrap()
            .binary()
            .await
            .unwrap();

        let mut buf = Cursor::new(point_data);
        let message: DFMessage<NumassMeta> =
            read_df_message_sync::<NumassMeta>(&mut buf).unwrap();

        rsb_event::Point::parse_from_bytes(&message.data.unwrap()[..]).unwrap()
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut point_file = tokio::fs::File::open(&filepath).await.unwrap();
        let message = read_df_message::<numass::NumassMeta>(&mut point_file)
            .await
            .unwrap();
        rsb_event::Point::parse_from_bytes(&message.data.unwrap_or(vec![])[..]).unwrap()
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
    
    let meta = load_meta(&filepath).await;

    if let Some(NumassMeta::Reply(Reply::AcquirePoint { 
        ..
     })) = &meta {

        
        #[cfg(not(target_arch = "wasm32"))]
        let amplitudes = {
            let point = load_point(&filepath).await;
            Some(extract_events(
                &point,
                &process,
            ))
        };

        #[cfg(target_arch = "wasm32")]
        let amplitudes = {
            let amplitudes_raw = Request::post(&api_url("api/process", &filepath))
                    .json(&process).unwrap()
                    .send()
                    .await
                    .unwrap()
                    .binary()
                    .await
                    .unwrap();
            rmp_serde::from_slice::<Option<NumassAmps>>(&amplitudes_raw).unwrap()
        };

        
        if let Some(amplitudes) = amplitudes {
            let processed = processing::post_process(amplitudes, &post_process);
            let histogram = events_to_histogram(processed, histogram);

            let counts = Some(histogram.events_all(None));

            // extract voltage from meta
            let voltage = if let Some(NumassMeta::Reply(Reply::AcquirePoint {
                    external_meta: Some(ExternalMeta {hv1_value: Some(voltage), ..}), .. 
                })) = &meta {
                        Some(*voltage)
                } else {
                    None
                };
            // extract start time from meta
            let start_time = if let Some(NumassMeta::Reply(Reply::AcquirePoint {
                    start_time, .. 
                })) = &meta {
                    Some(start_time.to_owned())
                } else {
                    None
                };

            Some(PointState {
                opened: true,
                histogram: Some(histogram),
                start_time,
                voltage,
                counts
            })
        } else {
            None
        }
    } else {
        None
    }
}