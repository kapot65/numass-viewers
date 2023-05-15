use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::{epaint::Color32, egui::{self, mutex::Mutex, Ui, plot::{Legend, Line, Plot, Points}}};

use processing::{ProcessingParams, numass::{NumassMeta, Reply}};
use backend::{FSRepr, FileCache};
use crate::{color_same_as_egui, algorithm_editor, post_processing_editor, histogram_params_editor};

#[cfg(not(target_arch = "wasm32"))]
use {
    backend::{expand_dir, process_file},
    home::home_dir,
    std::fs::File,
    std::io::Write,
    tokio::spawn,
    which::which,
};

#[cfg(target_arch = "wasm32")]
use {
    backend::ProcessRequest, eframe::web_sys::window, gloo_net::http::Request,
    wasm_bindgen::prelude::*, wasm_bindgen_futures::spawn_local as spawn,
};

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    fn download(filename: &str, text: &str);
}

#[derive(PartialEq, Clone, Copy)]
pub enum PlotMode {
    Histogram,
    PPT,
    PPV,
}

#[derive(Clone, Copy)]
struct ProcessingStatus {
    pub running: bool,
    pub total: usize,
    pub processed: usize
}

pub struct DataViewerApp {
    pub root: Arc<Mutex<Option<FSRepr>>>,
    plot_mode: PlotMode,
    processing_status: Arc<Mutex<ProcessingStatus>>,
    processing_params: Arc<Mutex<ProcessingParams>>,
    state: Arc<Mutex<BTreeMap<String, FileCache>>>,
}

impl DataViewerApp {
    pub fn new() -> Self {
        Self {
            root: Arc::new(Mutex::new(None)),
            state: Arc::new(Mutex::new(BTreeMap::new())),
            processing_status: Arc::new(Mutex::new(ProcessingStatus { 
                running: false, total: 0, processed: 0 })),
            processing_params: Arc::new(Mutex::new(processing::ProcessingParams::default())),
            plot_mode: PlotMode::Histogram,
        }
    }

    fn files_editor(&self, ui: &mut Ui) {
        let root_lock = self.root.lock().clone();

        ui.horizontal(|ui| {
            if ui.button("open").clicked() {
                let root = self.root.clone();

                spawn(async move {
                    #[cfg(not(target_arch = "wasm32"))]
                    if let Some(root_path) = rfd::FileDialog::new().pick_folder() {
                        *root.lock() = expand_dir(root_path)
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        let resp = Request::get("/api/files").send().await.unwrap();
                        *root.lock() = Some(resp.json::<FSRepr>().await.unwrap())
                    }
                });
            }

            let path = root_lock.clone().map(|root| match root {
                FSRepr::File { path } => path,
                FSRepr::Directory { path, children: _ } => path,
            });
            if path.is_some() && ui.button("reload").clicked() {
                #[cfg(not(target_arch = "wasm32"))]
                #[allow(clippy::unnecessary_unwrap)]
                {
                    *self.root.lock() = expand_dir(path.unwrap());
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let root = self.root.clone();
                    spawn(async move {
                        let resp = Request::get("/api/files").send().await.unwrap();
                        *root.lock() = Some(resp.json::<FSRepr>().await.unwrap())
                    })
                }
            }

            let ProcessingStatus {
                running, total, processed
            } =  *self.processing_status.lock();

            if running {
                ui.horizontal(|ui| {
                    ui.label(format!("{processed}/{total}"));
                    ui.spinner();
                });
            } else if ui.button("apply").clicked() {
                self.process()
            }

            if ui.button("clear").clicked() {
                self.state.lock().clear()
            }

            if ui.button("save").clicked() {
                let state = self.state.lock().clone();
                let plot_mode = self.plot_mode;

                spawn(async move {
                    #[cfg(not(target_arch = "wasm32"))]
                    let save_folder = rfd::FileDialog::new()
                        .set_directory(home_dir().unwrap())
                        .pick_folder();
                    #[cfg(target_arch = "wasm32")]
                    let save_folder = Some(PathBuf::new());

                    if let Some(save_folder) = save_folder {

                        let state_sorted = {
                            let mut state = state.iter().collect::<Vec<_>>();
                            state.sort_by(|(key_1, _), (key_2, _)| natord::compare(key_1, key_2));
                            state
                        };

                        match plot_mode {

                            PlotMode::Histogram => {
                                for (name, cache) in state_sorted.iter() {
                                    if let Some(histogramm) = &cache.histogram {
                                        let point_name = {
                                            let temp = PathBuf::from(name);
                                            temp.file_name().unwrap().to_owned()
                                        };
    
                                        let mut data = String::new();
                                        {
                                            let mut row = String::new();
                                            row.push_str("bin\t");
                                            for ch_num in histogramm.channels.keys() {
                                                row.push_str(&format!("ch {}\t", *ch_num + 1));
                                            }
                                            row.push('\n');
    
                                            data.push_str(&row);
                                        }
    
                                        for (idx, bin) in histogramm.x.iter().enumerate() {
                                            let mut row = String::new();
    
                                            row.push_str(&format!("{bin:.4}\t"));
                                            for val in histogramm.channels.values() {
                                                row.push_str(&format!("{}\t", val[idx]));
                                            }
                                            row.push('\n');
                                            data.push_str(&row);
                                        }
    
                                        let mut filepath = save_folder.clone();
                                        filepath.push(point_name);
    
                                        #[cfg(not(target_arch = "wasm32"))]
                                        {
                                            let mut out_file = File::create(filepath).unwrap();
                                            out_file.write_all(data.as_bytes()).unwrap();
                                        }
                                        #[cfg(target_arch = "wasm32")]
                                        download(filepath.to_str().unwrap(), &data);
                                    }
                                }
                            }

                            PlotMode::PPT => {

                                let mut data = String::new();
                                {
                                    data.push_str("path\ttime\tcounts\n");
                                }

                                for (name, cache) in state_sorted.iter() {

                                    if let FileCache { meta: Some(
                                        NumassMeta::Reply(Reply::AcquirePoint {
                                            start_time, .. }) ), histogram: Some(histogram), .. } = cache {
        
                                            let point_name = {
                                                let temp = PathBuf::from(name);
                                                temp.file_name().unwrap().to_owned()
                                            };

                                            let counts = histogram.channels.values().map(|ch| ch.iter().sum::<f32>()).sum::<f32>();

                                            data.push_str(&format!("{point_name:?}\t{start_time:?}\t{counts}\n"));
                                    }
                                }

                                let mut filepath = save_folder;
                                filepath.push("PPT.tsv");

                                #[cfg(not(target_arch = "wasm32"))]
                                {
                                    let mut out_file = File::create(filepath).unwrap();
                                    out_file.write_all(data.as_bytes()).unwrap();
                                }
                                #[cfg(target_arch = "wasm32")]
                                download(filepath.to_str().unwrap(), &data);
                            }
                            PlotMode::PPV => {

                                let mut data = String::new();
                                {
                                    data.push_str("path\tvoltage\tcounts\n");
                                }
    
                                for (name, cache) in state_sorted.iter() {


                                    if let FileCache { meta: Some(
                                        NumassMeta::Reply(Reply::AcquirePoint {
                                            external_meta: Some(external_meta), .. }) ), histogram: Some(histogram), .. } = cache {
                                        let voltage =  external_meta.get("HV1_value").unwrap().as_str().unwrap().parse::<f64>().unwrap();
                                        let counts = histogram.channels.values().map(|ch| ch.iter().sum::<f32>()).sum::<f32>();

                                        let point_name = {
                                            let temp = PathBuf::from(name);
                                            temp.file_name().unwrap().to_owned()
                                        };
    
                                        data.push_str(&format!("{point_name:?}\t{voltage}\t{counts}\n"));
                                    }

                                }

                                let mut filepath = save_folder;
                                filepath.push("PPV.tsv");

                                #[cfg(not(target_arch = "wasm32"))]
                                {
                                    let mut out_file = File::create(filepath).unwrap();
                                    out_file.write_all(data.as_bytes()).unwrap();
                                }
                                #[cfg(target_arch = "wasm32")]
                                download(filepath.to_str().unwrap(), &data);
                            }
                        }
                    }
                });
            }
        });

        egui::containers::ScrollArea::new([false, true]).show(ui, |ui| {
            if let Some(root) = &root_lock {
                file_tree_entry(ui, root, &mut self.state.lock());
            }
        });
    }

    pub fn process(&self) {
        let params = self.processing_params.lock().clone();
        let state = Arc::clone(&self.state);
        let status = Arc::clone(&self.processing_status);

        spawn(async move {

            let files_to_processed = {
                state
                    .lock()
                    .iter()
                    .filter_map(|(filepath, cache)| {
                        if cache.opened {
                            let need_recalc = true;
                            if need_recalc {
                                Some(filepath.clone())
                            } else if let Some(processed) = cache.processed {
                                let meta = std::fs::metadata(filepath).unwrap();
                                if processed >= meta.modified().unwrap() {
                                    None
                                } else {
                                    Some(filepath.clone())
                                }
                            } else {
                                Some(filepath.clone())
                            }
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            };

            if files_to_processed.is_empty() {
                return;
            }

            {
                let mut status = status.lock();
                status.total = files_to_processed.len();
                status.processed = 0;
                status.running = true
            }

            for filepath in files_to_processed {
                let configuration_local = state.clone();
                let status = Arc::clone(&status);

                let processing = params.clone();
                spawn(async move {
                    #[cfg(not(target_arch = "wasm32"))]
                    let cache = { process_file(PathBuf::from(&filepath), processing) };
                    #[cfg(target_arch = "wasm32")]
                    let cache = {
                        let value = Request::post("/api/process")
                            .json(&ProcessRequest::CalcHist {
                                filepath: PathBuf::from(&filepath),
                                processing,
                            })
                            .unwrap()
                            .send()
                            .await
                            .unwrap()
                            .json::<serde_json::Value>()
                            .await
                            .unwrap();
                        serde_json::from_value::<Option<FileCache>>(value).unwrap()
                    };
                    let mut conf = configuration_local.lock();
                    conf.insert(
                        filepath.to_owned(),
                        cache.unwrap_or(FileCache {
                            opened: false,
                            processed: None,
                            histogram: None,
                            meta: None,
                        }),
                    );

                    let mut status = status.lock();
                    status.processed += 1;
                    if status.processed == status.total {
                        *status = ProcessingStatus {
                            running: false,
                            total: 0,
                            processed: 0
                        }
                    }
                });
            }
        });
    }
}

impl Default for DataViewerApp {
    fn default() -> Self {
        Self::new()
    }
}

fn file_tree_entry(
    ui: &mut egui::Ui,
    entry: &FSRepr,
    opened_files: &mut BTreeMap<String, FileCache>,
) {
    match entry {
        FSRepr::File { path } => {
            let cache = opened_files
                .entry(path.to_str().unwrap().to_string())
                .or_insert(FileCache {
                    opened: false,
                    processed: None,
                    histogram: None,
                    meta: None,
                });

            let mut change_set = None;

            ui.horizontal(|ui| {
                if ui.checkbox(&mut cache.opened, "").changed() && path.ends_with("meta") {
                    change_set = Some(cache.opened);
                };
                let filename = path.file_name().unwrap().to_str().unwrap();
                ui.label(filename);
            });

            if let Some(opened) = change_set {
                let parent_folder = path.parent().unwrap().to_str().unwrap();
                let filtered_keys = opened_files
                    .keys()
                    .filter(|key| key.contains(parent_folder))
                    .cloned()
                    .collect::<Vec<_>>();
                for key in filtered_keys {
                    opened_files.get_mut(&key).unwrap().opened = opened;
                }
            }
        }

        FSRepr::Directory { path, children } => {
            egui::CollapsingHeader::new(path.file_name().unwrap().to_str().unwrap())
                .id_source(path.to_str().unwrap())
                .show(ui, |ui| {
                    for child in children {
                        file_tree_entry(ui, child, opened_files)
                    }
                });
        }
    }
}

fn params_editor(ui: &mut Ui, processing_params: ProcessingParams) -> ProcessingParams {

    let algorithm = algorithm_editor(ui, &processing_params.algorithm);

    ui.separator();

    let post_processing = post_processing_editor(ui, &processing_params.post_processing);

    ui.separator();

    let histogram = histogram_params_editor(ui, &processing_params.histogram);

    ProcessingParams {
        algorithm,
        post_processing,
        histogram,
    }
}

impl eframe::App for DataViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));

        egui::SidePanel::left("left").show(ctx, |ui| {
            let mut processing_params = self.processing_params.lock();
            *processing_params = params_editor(ui, processing_params.clone());
            drop(processing_params);
            self.files_editor(ui);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let state = self.state.lock();

            let mut left_border = 0.0;
            let mut right_border = 0.0;

            let opened_files = state.iter().filter(|(_, cache)| {
                cache.opened
            }).collect::<Vec<_>>();

            #[cfg(not(target_arch = "wasm32"))]
            let height = _frame.info().window_info.size.y;
            #[cfg(target_arch = "wasm32")]
            let height = window().unwrap().inner_height().unwrap().as_f64().unwrap() as f32;

            match self.plot_mode {
                PlotMode::Histogram => {
                    let plot = Plot::new("Histogram Plot").legend(Legend { 
                        text_style: egui::TextStyle::Body,
                        background_alpha: 1.0, position: egui::plot::Corner::RightTop
                    })
                    .height(height - 35.0);

                    plot.show(ui, |plot_ui| {

                        let bounds = plot_ui.plot_bounds();
                        left_border = bounds.min()[0] as f32;
                        right_border = bounds.max()[0] as f32;

                        let lines = if opened_files.len() == 1 {
                            let (_, cache) = opened_files[0];
                            if !(cache.opened && cache.histogram.is_some()) {
                                return;
                            }
                            let hist = cache.histogram.clone().unwrap();

                            hist.channels.iter().map(|(ch_num, y)| {
                                (format!("ch #{}", ch_num + 1), color_same_as_egui(*ch_num as usize), hist.step, hist.x.clone(), y.clone())
                            }).collect::<Vec<_>>()
                        } else {
                            opened_files.iter().enumerate()
                            .filter(|(_, (_, cache))| {cache.histogram.is_some()}) // TODO change to filtermap
                            .map(|(idx, (filepath, cache))| {
                                let hist = cache.histogram.clone().unwrap();

                                let mut y_all = vec![0.0; hist.x.len()];
                                for (_, y) in hist.channels {
                                    for (idx, val) in y.iter().enumerate() {
                                        y_all[idx] += val;
                                    }
                                }

                                (filepath.to_string(), color_same_as_egui(idx), hist.step, hist.x, y_all)
                            }).collect::<Vec<_>>()
                        };

                        for (name, color, step, x, y) in lines {
                            let mut events_in_window = 0;

                            let line_data = y.iter().enumerate().flat_map(|(idx, y)| {
                                if x[idx] > left_border && x[idx] < right_border {
                                    events_in_window += *y as i32;
                                }
                                [
                                    [(x[idx] - step / 2.0)  as f64, *y as f64],
                                    [(x[idx] + step / 2.0)  as f64, *y as f64]
                                ]
                            }).collect::<Vec<_>>();

                            plot_ui.line(Line::new(line_data)
                            .width(if ctx.style().visuals.dark_mode { 1.0 } else { 2.0 })
                            .color(color)
                            .name(
                                format!("{name}\t({events_in_window})")
                            ))
                        }
                    });
                }
                PlotMode::PPT => {
                    let plot = Plot::new("Point/Time").legend(Legend {
                        text_style: egui::TextStyle::Body,
                        background_alpha: 1.0, position: egui::plot::Corner::RightTop
                    })
                    .x_axis_formatter(|value, _| chrono::NaiveDateTime::from_timestamp_millis(value as i64).unwrap().to_string())
                    .height(height - 35.0);

                    plot.show(ui, |plot_ui| {
                        let points = opened_files.iter().filter_map(|(_, cache)| {
                            if let FileCache { meta: Some(
                                NumassMeta::Reply(Reply::AcquirePoint {
                                    start_time, .. }) ), histogram: Some(histogram), .. } = cache {

                                let counts = histogram.channels.values().map(|ch| ch.iter().sum::<f32>()).sum::<f32>();
                                Some([start_time.timestamp_millis() as f64, counts as f64])
                            } else {
                                None
                            }
                        }).collect::<Vec<_>>();

                        plot_ui.points(Points::new(points).radius(3.0));
                    });
                }
                PlotMode::PPV => {
                    let plot = Plot::new("Point/Voltage").legend(Legend {
                        text_style: egui::TextStyle::Body,
                        background_alpha: 1.0, position: egui::plot::Corner::RightTop
                    })
                    .height(height - 35.0);

                    plot.show(ui, |plot_ui| {
                        let points = opened_files.iter().filter_map(|(_, cache)| {
                            if let FileCache { meta: Some(
                                NumassMeta::Reply(Reply::AcquirePoint {
                                    external_meta: Some(external_meta), .. }) ), histogram: Some(histogram), .. } = cache {
                                let voltage =  external_meta.get("HV1_value").unwrap().as_str().unwrap().parse::<f64>().unwrap();
                                let counts = histogram.channels.values().map(|ch| ch.iter().sum::<f32>()).sum::<f32>();
                                Some([voltage, counts as f64])
                            } else {
                                None
                            }
                        }).collect::<Vec<_>>();

                        plot_ui.points(Points::new(points).radius(3.0));
                    });
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {

                #[cfg(not(target_arch = "wasm32"))]
                let filtered_viewer_in_path = which("filtered-viewer").is_ok();
                #[cfg(target_arch = "wasm32")]
                let filtered_viewer_in_path = true;

                let filtered_viewer_button = ui.add_enabled(
                    opened_files.len() == 1 && filtered_viewer_in_path,
                    egui::Button::new("waveforms (in window)")).on_disabled_hover_ui(|ui| {
                    if !filtered_viewer_in_path {
                        ui.colored_label(Color32::RED, "filtered-viewer must be in PATH");
                    }
                    if opened_files.len() != 1 {
                        ui.colored_label(Color32::RED, "exact one file must be opened");
                    }
                });

                let algorithm = self.processing_params.lock().algorithm;
                let convert_kev = self.processing_params.lock().post_processing.convert_to_kev;

                if filtered_viewer_button.clicked() {
                    let (filepath, _) = opened_files[0];
                    #[cfg(not(target_arch = "wasm32"))] {
                        let mut command = tokio::process::Command::new("filtered-viewer");
                        
                        command.arg(filepath)
                        .arg("--min").arg(left_border.max(0.0).to_string())
                        .arg("--max").arg(right_border.max(0.0).to_string())
                        .arg("--algorithm").arg(serde_json::to_string(&algorithm).unwrap());

                        if convert_kev {
                            command.arg("--convert-kev");
                        }
                        
                        command.spawn().unwrap();
                    }
                    #[cfg(target_arch = "wasm32")] {
                        let search = serde_qs::to_string(&ProcessRequest::FilterEvents {
                            filepath: PathBuf::from(filepath),
                            algorithm,
                            convert_kev,
                            range: left_border.max(0.0)..right_border.max(0.0),
                            neighborhood: 5000 }).unwrap();
                        window().unwrap().open_with_url(&format!("/?{search}")).unwrap();
                    }
                }

                #[cfg(not(target_arch = "wasm32"))]
                let point_viewer_in_path = which("point-viewer").is_ok();
                #[cfg(target_arch = "wasm32")]
                let point_viewer_in_path = true;

                let point_viewer_button = ui.add_enabled(opened_files.len() == 1 && point_viewer_in_path,
                egui::Button::new("waveforms (all)")).on_disabled_hover_ui(|ui| {
                    if !point_viewer_in_path {
                        ui.colored_label(Color32::RED, "point-viewer must be in PATH");
                    }
                    if opened_files.len() != 1 {
                        ui.colored_label(Color32::RED, "exact one file must be opened");
                    }
                });

                if point_viewer_button.clicked() {
                    let (filepath, _) = opened_files[0];
                    #[cfg(not(target_arch = "wasm32"))] {
                        tokio::process::Command::new("point-viewer").arg(filepath).spawn().unwrap();
                    }
                    #[cfg(target_arch = "wasm32")] {
                        let search = serde_qs::to_string(&ProcessRequest::SplitTimeChunks {
                            filepath: PathBuf::from(filepath)
                        }).unwrap();
                        window().unwrap().open_with_url(&format!("/?{search}")).unwrap();
                    }
                }

                // let mut plo = processing_params.algorithm;
                ui.radio_value(&mut self.plot_mode, PlotMode::Histogram, "Hist");
                ui.radio_value(&mut self.plot_mode, PlotMode::PPT, "PPT");
                ui.radio_value(&mut self.plot_mode, PlotMode::PPV, "PPV");
            });
        });
    }
}