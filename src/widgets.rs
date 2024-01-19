//! Common widgets for the application

use egui::Ui;
use processing::{histogram::HistogramParams, postprocess::PostProcessParams, process::{Algorithm, ProcessParams}};

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
    });

    ui.horizontal(|ui| {
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