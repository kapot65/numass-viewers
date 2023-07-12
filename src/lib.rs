#![warn(clippy::all, rust_2018_idioms)]

use std::path::PathBuf;

use egui::Ui;
use protobuf::Message;

use processing::{histogram::HistogramParams, PostProcessingParams, Algorithm, numass::protos::rsb_event};

#[cfg(target_arch = "wasm32")]
use {
    std::io::Cursor,
    gloo::net::http::Request,
    dataforge::{read_df_message_sync, DFMessage},
    processing::numass::NumassMeta
};

#[cfg(not(target_arch = "wasm32"))]
use {
    dataforge::read_df_message,
    processing::numass,
};

pub mod app;
pub mod filtered_viewer;
pub mod point_viewer;

#[cfg(target_arch = "wasm32")]
pub mod worker;

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

pub fn post_processing_editor(ui: &mut Ui, ctx: &egui::Context, post_processing: &PostProcessingParams) -> PostProcessingParams {

    ui.label("Postprocessing params");

    let mut convert_to_kev = post_processing.convert_to_kev;
    ui.checkbox(&mut convert_to_kev, "convert to keV");

    let mut use_dead_time = post_processing.use_dead_time;
    let mut effective_dead_time = post_processing.effective_dead_time;

    ui.checkbox(&mut use_dead_time, "use dead time");
    ui.add_enabled(
        use_dead_time,
        egui::Slider::new(&mut effective_dead_time, 0..=30000).text("ns"),
    );

    let mut merge_close_events = post_processing.merge_close_events;
    ui.checkbox(&mut merge_close_events, "merge close events");

    let mut merge_map = post_processing.merge_map;
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
        // #[cfg(not(target_arch = "wasm32"))]

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

    PostProcessingParams { 
        convert_to_kev,
        use_dead_time,
        effective_dead_time,
        merge_close_events,
        merge_map
    }
}

pub fn algorithm_editor(ui: &mut Ui, algorithm: &Algorithm) -> Algorithm {

    let mut algorithm = algorithm.to_owned();

    ui.label("Algorithm params");

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
    });

    match algorithm {
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
    }
}

pub async fn load_point(filepath: PathBuf) -> rsb_event::Point {
    #[cfg(target_arch = "wasm32")]
    {
        let point_data = Request::get(&format!("/files{}", filepath.to_str().unwrap()))
            .send()
            .await
            .unwrap()
            .binary()
            .await
            .unwrap();

        let mut buf = Cursor::new(point_data);
        let message: DFMessage<NumassMeta> =
            read_df_message_sync::<NumassMeta>(&mut buf).unwrap();
        let point = rsb_event::Point::parse_from_bytes(&message.data.unwrap()[..]).unwrap();

        point
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut point_file = tokio::fs::File::open(&filepath).await.unwrap();
        let message = read_df_message::<numass::NumassMeta>(&mut point_file)
            .await
            .unwrap();
        rsb_event::Point::parse_from_bytes(&message.data.unwrap()[..]).unwrap()
    }
}