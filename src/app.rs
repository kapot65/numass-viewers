use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::{epaint::Color32, egui::{self, mutex::Mutex, Ui, plot::{Legend, Plot, Points}}};

use processing::{viewer::ViewerState, numass::{NumassMeta, Reply}};

use crate::{process_editor, post_process_editor, histogram_params_editor};

#[cfg(not(target_arch = "wasm32"))]
use {
    processing::{extract_amplitudes, viewer::{FSRepr, FileCache}},
    numass::{self, protos::rsb_event},
    dataforge::{read_df_message, DFMessage},
    home::home_dir,
    std::fs::File,
    std::io::Write,
    protobuf::Message,
    tokio::spawn,
    which::which,
};

#[cfg(target_arch = "wasm32")]
use {
    eframe::web_sys::window, gloo::net::http::Request,
    wasm_bindgen::prelude::*, wasm_bindgen_futures::spawn_local as spawn,
    processing::viewer::{FSRepr, FileCache, ViewerMode}, 
    crate::worker::WebThreadPool
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
pub struct ProcessingStatus {
    pub running: bool,
    pub total: usize,
    pub processed: usize
}

pub struct DataViewerApp {
    pub root: Arc<Mutex<Option<FSRepr>>>,
    select_single: bool,
    plot_mode: PlotMode,
    processing_status: Arc<Mutex<ProcessingStatus>>,
    processing_params: Arc<Mutex<ViewerState>>,

    state: Arc<Mutex<BTreeMap<String, FileCache>>>,

    #[cfg(target_arch = "wasm32")]
    thread_pool: Arc<WebThreadPool>,
}


impl DataViewerApp {
    pub fn new() -> Self {

        let state = Arc::new(Mutex::new(BTreeMap::new()));
        let processing_status = Arc::new(Mutex::new(ProcessingStatus { 
            running: false, total: 0, processed: 0 }));

        #[cfg(target_arch = "wasm32")]
        let thread_pool = Arc::new(WebThreadPool::new(
            Arc::clone(&state), 
            Arc::clone(&processing_status)));

        Self {
            root: Arc::new(Mutex::new(None)),
            select_single: false,
            state,
            processing_status,
            processing_params: Arc::new(Mutex::new(ViewerState::default())),
            plot_mode: PlotMode::Histogram,
            #[cfg(target_arch = "wasm32")]
            thread_pool
        }
    }

    fn files_editor(&mut self, ui: &mut Ui) {
        let root_lock = self.root.lock().clone();

        ui.checkbox(&mut self.select_single, "select single");

        ui.horizontal(|ui| {
            if ui.button("open").clicked() {
                let root = self.root.clone();

                spawn(async move {
                    #[cfg(not(target_arch = "wasm32"))]
                    if let Some(root_path) = rfd::FileDialog::new().pick_folder() {
                        *root.lock() = FSRepr::expand_dir(root_path)
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
                    *self.root.lock() = FSRepr::expand_dir(path.unwrap());
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
                                    if let Some(histogram) = &cache.histogram {
                                        let point_name = {
                                            let temp = PathBuf::from(name);
                                            temp.file_name().unwrap().to_owned()
                                        };
    
                                        let data = histogram.to_csv('\t');
    
                                        let mut filepath = save_folder.clone();
                                        filepath.push(point_name);
    
                                        #[cfg(not(target_arch = "wasm32"))]
                                        {
                                            std::fs::write(filepath, data).unwrap();
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

                let mut updated = false;
                file_tree_entry(
                    ui, root, 
                    &self.select_single,
                    &mut self.state.lock(),
                    &mut updated,
                );

                if updated && self.select_single {
                    self.process();
                }
            }
        });
    }

    pub fn process(&self) {
        let params = self.processing_params.lock().clone();
        let state = Arc::clone(&self.state);
        let status = Arc::clone(&self.processing_status);

        #[cfg(target_arch = "wasm32")]
        let thread_pool = Arc::clone(&self.thread_pool);

        spawn(async move {

            let files_to_processed = {
                state
                    .lock()
                    .iter()
                    .filter_map(|(filepath, cache)| {
                        if cache.opened {
                            Some(filepath.clone())
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
                #[cfg(not(target_arch = "wasm32"))]
                let configuration_local = state.clone();
                #[cfg(not(target_arch = "wasm32"))]
                let status = Arc::clone(&status);

                #[cfg(target_arch = "wasm32")]
                let thread_pool = Arc::clone(&thread_pool);

                let processing = params.clone();
                spawn(async move {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let mut point_file = tokio::fs::File::open(&filepath).await.unwrap();
                        if let Ok(DFMessage {
                            meta,
                            data,
                        }) = read_df_message::<numass::NumassMeta>(&mut point_file).await {

                            if let numass::NumassMeta::Reply(numass::Reply::AcquirePoint { .. }) = meta.clone() {
                                let point = rsb_event::Point::parse_from_bytes(&data.unwrap()).unwrap(); // return None for bad parsing
                                let amplitudes = Some(extract_amplitudes(
                                    &point,
                                    &params.process,
                                ));
                                
                                if let Some(amplitudes) = amplitudes {
                        
                                    let processed = processing::post_process(amplitudes, &processing.post_process);
                                    let  histogram = processing::amplitudes_to_histogram(processed, processing.histogram);

                                    let mut conf: egui::mutex::MutexGuard<'_, BTreeMap<String, FileCache>> = configuration_local.lock();
                                    conf.insert(
                                        filepath.to_owned(),
                                        FileCache {
                                            opened: true,
                                            histogram: Some(histogram),
                                            meta: Some(meta),
                                        },
                                    );
                                }
                            }
                            
                            {
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
                        }
                    }
                    #[cfg(target_arch = "wasm32")]
                    thread_pool.process_point(filepath, processing).await;
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
    select_single: &bool,
    opened_files: &mut BTreeMap<String, FileCache>,
    updated: &mut bool,
) {
    match entry {
        FSRepr::File { path } => {
            let key = path.to_str().unwrap().to_string();
            let cache = opened_files
                .entry(key.clone())
                .or_insert(FileCache {
                    opened: false,
                    histogram: None,
                    meta: None,
                });

            let mut change_set = None;
            let mut exclusive_point = None;

            ui.horizontal(|ui| {
                
                if ui.checkbox(&mut cache.opened, "").changed() {

                    if cache.opened && *select_single {
                        exclusive_point = Some(key)
                    }

                    if path.ends_with("meta") {
                        change_set = Some(cache.opened)
                    };
                }

                let filename = path.file_name().unwrap().to_str().unwrap();
                ui.label(filename);
            });

            if let Some(point) = exclusive_point {
                for (key, cache) in opened_files.iter_mut() {
                    if key != &point {
                        cache.opened = false;
                    }
                }
                *updated = true;
            } else if let Some(opened) = change_set {
                let parent_folder = path.parent().unwrap().to_str().unwrap();
                let filtered_keys = opened_files
                    .keys()
                    .filter(|key| key.contains(parent_folder))
                    .cloned()
                    .collect::<Vec<_>>();
                for key in filtered_keys {
                    opened_files.get_mut(&key).unwrap().opened = opened;
                }
                *updated = true;
            }
        }

        FSRepr::Directory { path, children } => {
            egui::CollapsingHeader::new(path.file_name().unwrap().to_str().unwrap())
                .id_source(path.to_str().unwrap())
                .show(ui, |ui| {
                    for child in children {
                        file_tree_entry(ui, child, select_single, opened_files, updated)
                    }
                });
        }
    }
}

fn params_editor(ui: &mut Ui, ctx: &egui::Context, state: ViewerState) -> ViewerState {

    let process = process_editor(ui, &state.process);

    ui.separator();

    let post_process = post_process_editor(ui, ctx, &state.post_process);

    ui.separator();

    let histogram = histogram_params_editor(ui, &state.histogram);

    ViewerState {
        process,
        post_process,
        histogram,
    }
}

impl eframe::App for DataViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));

        egui::SidePanel::left("left").show(ctx, |ui| {
            let mut processing_params = self.processing_params.lock();
            *processing_params = params_editor(ui, ctx, processing_params.clone());
            drop(processing_params);

            ui.separator();

            self.files_editor(ui);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let state = self.state.lock();

            let thickness = if ctx.style().visuals.dark_mode { 1.0 } else { 2.0 };

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

                        if opened_files.len() == 1 {
                            if let (_, FileCache {opened: true, histogram: Some(hist), .. }) = opened_files[0] {
                                hist.draw_egui_each_channel(plot_ui, Some(thickness));
                            }
                        } else {
                            opened_files.iter().for_each(|(name, cache)| {
                                if let FileCache {opened: true, histogram: Some(hist), .. } = cache {
                                    hist.draw_egui(plot_ui, Some(name), Some(thickness), None);
                                }
                            })
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

                let process = self.processing_params.lock().process;

                if filtered_viewer_button.clicked() {
                    let (filepath, _) = opened_files[0];
                    #[cfg(not(target_arch = "wasm32"))] {
                        let mut command = tokio::process::Command::new("filtered-viewer");
                        
                        command.arg(filepath)
                        .arg("--min").arg(left_border.max(0.0).to_string())
                        .arg("--max").arg(right_border.max(0.0).to_string())
                        .arg("--processing").arg(serde_json::to_string(&process).unwrap());
                        
                        command.spawn().unwrap();
                    }
                    #[cfg(target_arch = "wasm32")] {
                        let search = serde_qs::to_string(&ViewerMode::FilterEvents {
                            filepath: PathBuf::from(filepath),
                            processing: process,
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
                        let search = serde_qs::to_string(&ViewerMode::SplitTimeChunks {
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