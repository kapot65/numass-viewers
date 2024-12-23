use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::{
    egui::{self, mutex::Mutex, Ui},
    epaint::Color32,
};
use egui_plot::{HLine, Legend, Plot, PlotPoint, Points, VLine};

use globset::GlobMatcher;
use processing::{storage::LoadState, viewer::ViewerState, widgets::UserInput};

#[cfg(not(target_arch = "wasm32"))]
use {
    crate::process_point,
    home::home_dir,
    processing::{storage::FSRepr, viewer::PointState},
    std::fs::File,
    std::io::Write,
    tokio::spawn,
    which::which,
};

#[cfg(target_arch = "wasm32")]
use {
    crate::{hyperlink::HyperlinkNewWindow, PointProcessor},
    eframe::web_sys::window,
    gloo::{
        net::http::Request,
        worker::{oneshot::OneshotBridge, Spawnable},
    },
    processing::{
        storage::{api_url, FSRepr},
        viewer::{PointState, ViewerMode},
    },
    wasm_bindgen::prelude::*,
    wasm_bindgen_futures::spawn_local as spawn,
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
    pub processed: usize,
}

pub struct DataViewerApp {
    #[cfg(not(target_arch = "wasm32"))]
    pub root: Arc<tokio::sync::Mutex<Option<FSRepr>>>,
    #[cfg(target_arch = "wasm32")]
    pub root: Arc<std::sync::Mutex<Option<FSRepr>>>,
    select_single: bool,
    glob_pattern: String,
    plot_mode: PlotMode,
    processing_status: Arc<Mutex<ProcessingStatus>>,
    processing_params: Arc<Mutex<ViewerState>>,

    state: Arc<Mutex<BTreeMap<String, PointState>>>,
    current_path: Option<String>,

    #[cfg(target_arch = "wasm32")]
    processor_pool: Vec<OneshotBridge<PointProcessor>>,
}

impl DataViewerApp {
    pub fn new() -> Self {
        let state = Arc::new(Mutex::new(BTreeMap::new()));
        let processing_status = Arc::new(Mutex::new(ProcessingStatus {
            running: false,
            total: 0,
            processed: 0,
        }));

        Self {
            #[cfg(not(target_arch = "wasm32"))]
            root: Arc::new(tokio::sync::Mutex::new(None)),
            #[cfg(target_arch = "wasm32")]
            root: Arc::new(std::sync::Mutex::new(None)),
            select_single: false,
            glob_pattern: "*/Tritium_1/set_[123]/p*(HV1=14000)".to_owned(),
            state,
            current_path: None,
            processing_status,
            processing_params: Arc::new(Mutex::new(ViewerState::default())),
            plot_mode: PlotMode::Histogram,
            #[cfg(target_arch = "wasm32")]
            processor_pool: {
                let concurrency =
                    gloo::utils::window().navigator().hardware_concurrency() as usize - 1;
                (0..concurrency)
                    .collect::<std::vec::Vec<usize>>()
                    .into_iter()
                    .map(|_| PointProcessor::spawner().spawn("./worker.js"))
                    .collect::<Vec<_>>()
            },
        }
    }

    fn files_editor(&mut self, ui: &mut Ui) {
        let mut root_copy = {
            if let Ok(root) = self.root.try_lock() {
                root.clone()
            } else {
                ui.spinner();
                return;
            }
        };

        ui.checkbox(&mut self.select_single, "select single");

        ui.horizontal(|ui| {
            ui.add_sized(
                [200.0, 20.0],
                egui::TextEdit::singleline(&mut self.glob_pattern),
            );

            let glob = globset::Glob::new(&self.glob_pattern);
            if ui
                .add_enabled(glob.is_ok(), egui::Button::new("match"))
                .clicked()
            {
                let glob = glob.unwrap().compile_matcher();

                let root = root_copy.clone();
                let state = Arc::clone(&self.state);

                if let Some(root) = root {
                    spawn(async move {
                        let mut matched = vec![];

                        fn process_leaf(
                            leaf: FSRepr,
                            glob: &GlobMatcher,
                            matched: &mut Vec<String>,
                        ) {
                            match leaf {
                                FSRepr::File { path, .. } => {
                                    if glob.is_match(&path) {
                                        matched.push(path.to_str().unwrap().to_owned())
                                    }
                                }
                                FSRepr::Directory { children, .. } => {
                                    for child in children {
                                        process_leaf(child, glob, matched)
                                    }
                                }
                            }
                        }
                        process_leaf(root, &glob, &mut matched);

                        let mut state = state.lock();
                        state.clear();

                        for path in matched {
                            state.entry(path).or_insert(EMPTY_POINT).opened = true;
                        }
                    });
                }
            }
        });

        ui.horizontal(|ui| {
            if ui.button("open").clicked() {
                let root = Arc::clone(&self.root);

                spawn(async move {
                    #[cfg(not(target_arch = "wasm32"))]
                    if let Some(root_path) = rfd::FileDialog::new().pick_folder() {
                        root.lock().await.replace(FSRepr::new(root_path));
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        let resp = Request::get("/api/root").send().await.unwrap();
                        root.lock()
                            .unwrap()
                            .replace(resp.json::<FSRepr>().await.unwrap());
                    }
                });
            }

            let path = root_copy.clone().map(|root| root.to_filename());
            if path.is_some() && ui.button("reload").clicked() {
                if let Some(mut root) = root_copy.clone() {
                    let root_out = Arc::clone(&self.root);

                    spawn(async move {
                        if let Ok(mut out) = root_out.try_lock() {
                            root.update_reccurently().await;
                            out.replace(root);
                        }
                    });
                }
            }

            let ProcessingStatus {
                running,
                total,
                processed,
            } = *self.processing_status.lock();

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
                                    data.push_str("path\ttime\ttime_raw\tcounts\n");
                                }

                                for (name, cache) in state_sorted.iter() {
                                    if let PointState {
                                        start_time: Some(start_time),
                                        counts: Some(counts),
                                        ..
                                    } = cache
                                    {
                                        let point_name = {
                                            let temp = PathBuf::from(name);
                                            temp.file_name().unwrap().to_owned()
                                        };

                                        data.push_str(&format!(
                                            "{point_name:?}\t{start_time:?}\t{}\t{counts}\n",
                                            start_time.timestamp()
                                        ));
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
                                    if let PointState {
                                        counts: Some(counts),
                                        voltage: Some(voltage),
                                        ..
                                    } = cache
                                    {
                                        let point_name = {
                                            let temp = PathBuf::from(name);
                                            temp.file_name().unwrap().to_owned()
                                        };
                                        data.push_str(&format!(
                                            "{point_name:?}\t{voltage}\t{counts}\n"
                                        ));
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
            if let Some(root) = &mut root_copy {
                let mut state_after = FileTreeState {
                    need_load: false,
                    need_process: false,
                };

                file_tree_entry(
                    ui,
                    root,
                    &self.select_single,
                    &mut self.state.lock(),
                    &mut state_after,
                );

                if state_after.need_process && self.select_single {
                    self.process();
                }

                if state_after.need_load {
                    let root_out = Arc::clone(&self.root);
                    let mut root = root.clone();

                    spawn(async move {
                        if let Ok(mut out) = root_out.try_lock() {
                            root.expand_reccurently().await;
                            out.replace(root);
                        }
                    });
                }
            }
        });
    }

    pub fn process(&self) {
        let changed = self.processing_params.lock().changed;
        self.processing_params.lock().changed = false;

        let params = self.processing_params.lock().clone();
        let state = Arc::clone(&self.state);
        let status = Arc::clone(&self.processing_status);

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

        // TODO: move to crate::reset_status
        {
            let mut status = status.lock();
            status.total = files_to_processed.len();
            status.processed = 0;
            status.running = true
        }

        for filepath in files_to_processed {
            let configuration_local = state.clone();
            let status = Arc::clone(&status);

            // get random worker from pool
            #[cfg(target_arch = "wasm32")]
            let mut point_processor = {
                let concurrency = self.processor_pool.len();
                let worker_num =
                    js_sys::eval(format!("Math.floor( Math.random() * {concurrency})").as_str())
                        .unwrap()
                        .as_f64()
                        .unwrap() as usize;
                self.processor_pool[worker_num].fork()
            };

            let processing = params.clone();
            spawn(async move {
                let modified =
                    processing::storage::load_modified_time(filepath.clone().into()).await;
                if let Some(modified) = modified {
                    let conf: egui::mutex::MutexGuard<'_, BTreeMap<String, PointState>> =
                        configuration_local.lock();
                    if let Some(&PointState {
                        modified: Some(modified_2),
                        ..
                    }) = conf.get(&filepath)
                    {
                        if !changed && modified <= modified_2 {
                            crate::inc_status(status);
                            return;
                        }
                    }
                }

                #[cfg(not(target_arch = "wasm32"))]
                let point_state = process_point(
                    filepath.clone().into(),
                    processing.process,
                    processing.post_process,
                    processing.histogram,
                )
                .await;
                #[cfg(target_arch = "wasm32")]
                let point_state = point_processor
                    .run((
                        filepath.clone().into(),
                        processing.process,
                        processing.post_process,
                        processing.histogram,
                    ))
                    .await;

                let point_state = point_state.unwrap_or(EMPTY_POINT);

                let mut conf: egui::mutex::MutexGuard<'_, BTreeMap<String, PointState>> =
                    configuration_local.lock();
                conf.insert(filepath.to_owned(), point_state);
                crate::inc_status(status);
            });
        }
    }
}

impl Default for DataViewerApp {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct FileTreeState {
    pub need_process: bool,
    pub need_load: bool,
}

const EMPTY_POINT: PointState = PointState {
    opened: false,
    histogram: None,
    voltage: None,
    start_time: None,
    acquisition_time: None,
    counts: None,
    modified: None,
};

fn file_tree_entry(
    ui: &mut egui::Ui,
    entry: &mut FSRepr,
    select_single: &bool,
    opened_files: &mut BTreeMap<String, PointState>,
    state_after: &mut FileTreeState,
) {
    match entry {
        FSRepr::File { path, .. } => {
            let key = path.to_str().unwrap().to_string();
            let cache = opened_files.entry(key.clone()).or_insert(EMPTY_POINT);

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

                #[cfg(not(target_arch = "wasm32"))]
                ui.label(filename);
                #[cfg(target_arch = "wasm32")]
                {
                    let hyperlink = HyperlinkNewWindow::new(filename, api_url("api/meta", path));
                    ui.add(hyperlink);
                }
            });

            if let Some(point) = exclusive_point {
                for (key, cache) in opened_files.iter_mut() {
                    if key != &point {
                        cache.opened = false;
                    }
                }
                state_after.need_process = true;
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
                state_after.need_process = true;
            }
        }

        FSRepr::Directory {
            path,
            children,
            load_state,
            ..
        } => {
            let header = egui::CollapsingHeader::new(path.file_name().unwrap().to_str().unwrap())
                .id_source(path.to_str().unwrap())
                .show(ui, |ui| {
                    for child in children {
                        file_tree_entry(ui, child, select_single, opened_files, state_after)
                    }
                });

            if header.fully_open() && load_state == &LoadState::NotLoaded {
                *load_state = LoadState::NeedLoad;
                state_after.need_load = true;
            }
        }
    }
}

fn params_editor(ui: &mut Ui, ctx: &egui::Context, state: ViewerState) -> ViewerState {
    let process = state.process.input(ui, ctx);

    ui.separator();

    let post_process = state.post_process.input(ui, ctx);

    ui.separator();

    let histogram = state.histogram.input(ui, ctx);

    let changed = state.changed
        || (process != state.process
            || post_process != state.post_process
            || histogram != state.histogram);

    ViewerState {
        process,
        post_process,
        histogram,
        changed,
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

            let thickness = if ctx.style().visuals.dark_mode {
                1.0
            } else {
                2.0
            };

            let mut left_border = 0.0;
            let mut right_border = 0.0;

            let opened_files = state
                .iter()
                .filter(|(_, cache)| cache.opened)
                .collect::<Vec<_>>();

            #[cfg(not(target_arch = "wasm32"))]
            let height = {
                let mut y = 0.0;
                ctx.input(|i| y = i.viewport().inner_rect.unwrap().size().y);
                y
            };
            #[cfg(target_arch = "wasm32")]
            let height = window().unwrap().inner_height().unwrap().as_f64().unwrap() as f32;

            match self.plot_mode {
                PlotMode::Histogram => {
                    let plot = Plot::new("Histogram Plot")
                        .legend(Legend::default())
                        .height(height - 35.0);

                    plot.show(ui, |plot_ui| {
                        let bounds = plot_ui.plot_bounds();
                        left_border = bounds.min()[0] as f32;
                        right_border = bounds.max()[0] as f32;

                        if opened_files.len() == 1 {
                            if let (
                                _,
                                PointState {
                                    opened: true,
                                    histogram: Some(hist),
                                    ..
                                },
                            ) = opened_files[0]
                            {
                                hist.draw_egui_each_channel(plot_ui, Some(thickness));
                            }
                        } else {
                            opened_files.iter().for_each(|(name, cache)| {
                                if let PointState {
                                    opened: true,
                                    histogram: Some(hist),
                                    ..
                                } = cache
                                {
                                    hist.draw_egui(plot_ui, Some(name), Some(thickness), None);
                                }
                            })
                        }
                    });
                }
                PlotMode::PPT => {
                    let plot = Plot::new("Point/Time")
                        .legend(Legend::default())
                        .x_axis_formatter(|mark, _, _| {
                            chrono::NaiveDateTime::from_timestamp_millis(mark.value as i64)
                                .unwrap()
                                .to_string()
                        })
                        .height(height - 35.0);

                    plot.show(ui, |plot_ui| {
                        let points = opened_files
                            .iter()
                            .filter_map(|(_, cache)| {
                                if let PointState {
                                    start_time: Some(start_time),
                                    counts: Some(counts),
                                    acquisition_time: Some(acquisition_time),
                                    ..
                                } = cache
                                {
                                    Some([
                                        start_time.timestamp_millis() as f64,
                                        *counts as f64 / (*acquisition_time as f64),
                                    ])
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>();

                        plot_ui.points(Points::new(points).radius(3.0));
                    });
                }
                PlotMode::PPV => {
                    let plot = Plot::new("Point/Voltage")
                        .legend(Legend::default())
                        .height(height - 35.0);

                    plot.show(ui, |plot_ui| {
                        let points = opened_files
                            .iter()
                            .filter_map(|(_, cache)| {
                                if let PointState {
                                    voltage: Some(voltage),
                                    counts: Some(counts),
                                    acquisition_time: Some(acquisition_time),
                                    ..
                                } = cache
                                {
                                    Some([
                                        *voltage as f64,
                                        *counts as f64 / (*acquisition_time as f64),
                                    ])
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>();

                        plot_ui.points(Points::new(points).radius(3.0));

                        if plot_ui.response().clicked() {
                            if let Some(pos) = plot_ui.pointer_coordinate() {
                                let clicked_file = opened_files.iter().find(|(_, cache)| {
                                    if let PointState {
                                        voltage: Some(voltage),
                                        counts: Some(counts),
                                        acquisition_time: Some(acquisition_time),
                                        ..
                                    } = cache
                                    {
                                        let point_pos = PlotPoint::new(
                                            *voltage as f64,
                                            *counts as f64 / (*acquisition_time as f64),
                                        );
                                        let distance = point_pos.to_pos2().distance(pos.to_pos2());
                                        if distance < 100.0 {
                                            return true;
                                        }
                                        false
                                    } else {
                                        false
                                    }
                                });

                                if let Some((path, cache)) = clicked_file {
                                    let path = (**path).clone();
                                    self.current_path =
                                        if let Some(p) = self.current_path.to_owned() {
                                            if p != path {
                                                println!("Clicked on {:?} ({:?})", path, cache);
                                                Some(path)
                                            } else {
                                                None
                                            }
                                        } else {
                                            Some(path)
                                        };
                                } else {
                                    self.current_path = None;
                                }
                            }
                        }

                        if let Some(current) = &self.current_path {
                            if let PointState {
                                voltage: Some(voltage),
                                counts: Some(counts),
                                acquisition_time: Some(acquisition_time),
                                ..
                            } = state[current]
                            {
                                plot_ui.hline(
                                    HLine::new(counts as f64 / (acquisition_time as f64))
                                        .color(Color32::WHITE),
                                );
                                plot_ui.vline(VLine::new(voltage as f64).color(Color32::WHITE));
                                // plot_ui.line(Line::new(vec![
                                //     [voltage as f64, counts as f64 / (acquisition_time as f64)],]).color(Color32::WHITE));
                            }
                        }
                    });
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                let marked_point = if let Some(path) = &self.current_path {
                    Some(path)
                } else if opened_files.len() == 1 {
                    Some(opened_files[0].0)
                } else {
                    None
                };

                #[cfg(not(target_arch = "wasm32"))]
                let filtered_viewer_in_path = which("filtered-viewer").is_ok();
                #[cfg(target_arch = "wasm32")]
                let filtered_viewer_in_path = true;

                let filtered_viewer_button = ui
                    .add_enabled(
                        marked_point.is_some() && filtered_viewer_in_path,
                        egui::Button::new("waveforms (in window)"),
                    )
                    .on_disabled_hover_ui(|ui| {
                        if !filtered_viewer_in_path {
                            ui.colored_label(Color32::RED, "filtered-viewer must be in PATH");
                        }
                        if marked_point.is_some() {
                            ui.colored_label(Color32::RED, "exact one file must be opened/marked");
                        }
                    });

                let process = self.processing_params.lock().process.clone();
                let postprocess = self.processing_params.lock().post_process;

                if filtered_viewer_button.clicked() {
                    let filepath = marked_point.unwrap();

                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let mut command = tokio::process::Command::new("filtered-viewer");

                        command
                            .arg(filepath)
                            .arg("--process")
                            .arg(serde_json::to_string(&process).unwrap())
                            .arg("--postprocess")
                            .arg(serde_json::to_string(&postprocess).unwrap());

                        if self.plot_mode == PlotMode::Histogram {
                            command
                                .arg("--min")
                                .arg(left_border.max(0.0).to_string())
                                .arg("--max")
                                .arg(right_border.max(0.0).to_string());
                        };

                        command.spawn().unwrap();
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        let search = serde_qs::to_string(&ViewerMode::FilteredEvents {
                            filepath: PathBuf::from(filepath),
                            process,
                            postprocess,
                            range: left_border.max(0.0)..right_border.max(0.0), // TODO: fix
                        })
                        .unwrap();
                        window()
                            .unwrap()
                            .open_with_url(&format!("/?{search}"))
                            .unwrap();
                    }
                }

                #[cfg(not(target_arch = "wasm32"))]
                let point_viewer_in_path = which("point-viewer").is_ok();
                #[cfg(target_arch = "wasm32")]
                let point_viewer_in_path = true;

                let point_viewer_button = ui
                    .add_enabled(
                        marked_point.is_some() && point_viewer_in_path,
                        egui::Button::new("waveforms (all)"),
                    )
                    .on_disabled_hover_ui(|ui| {
                        if !point_viewer_in_path {
                            ui.colored_label(Color32::RED, "point-viewer must be in PATH");
                        }
                        if marked_point.is_some() {
                            ui.colored_label(Color32::RED, "exact one file must be opened/marked");
                        }
                    });

                if point_viewer_button.clicked() {
                    let filepath = marked_point.unwrap();
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        tokio::process::Command::new("point-viewer")
                            .arg(filepath)
                            .spawn()
                            .unwrap();
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        let search = serde_qs::to_string(&ViewerMode::Waveforms {
                            filepath: PathBuf::from(filepath),
                        })
                        .unwrap();
                        window()
                            .unwrap()
                            .open_with_url(&format!("/?{search}"))
                            .unwrap();
                    }
                }

                #[cfg(not(target_arch = "wasm32"))]
                let trigger_viewer_in_path = which("trigger-viewer").is_ok();
                #[cfg(target_arch = "wasm32")]
                let trigger_viewer_in_path = true;

                let trigger_viewer_button = ui
                    .add_enabled(
                        marked_point.is_some() && trigger_viewer_in_path,
                        egui::Button::new("triggers"),
                    )
                    .on_disabled_hover_ui(|ui| {
                        if !trigger_viewer_in_path {
                            ui.colored_label(Color32::RED, "trigger-viewer must be in PATH");
                        }
                        if marked_point.is_some() {
                            ui.colored_label(Color32::RED, "exact one file must be opened/marked");
                        }
                    });

                if trigger_viewer_button.clicked() {
                    let filepath = marked_point.unwrap();
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        tokio::process::Command::new("trigger-viewer")
                            .arg(filepath)
                            .spawn()
                            .unwrap();
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        todo!("implement trigger viewer for wasm")
                    }
                }

                #[cfg(not(target_arch = "wasm32"))]
                let bundle_viewer_in_path = which("bundle-viewer").is_ok();
                #[cfg(target_arch = "wasm32")]
                let bundle_viewer_in_path = true;

                let bundle_viewer_button = ui
                    .add_enabled(
                        marked_point.is_some() && bundle_viewer_in_path,
                        egui::Button::new("bundles"),
                    )
                    .on_disabled_hover_ui(|ui| {
                        if !bundle_viewer_in_path {
                            ui.colored_label(Color32::RED, "bundle-viewer must be in PATH");
                        }
                        if marked_point.is_some() {
                            ui.colored_label(Color32::RED, "exact one file must be opened/marked");
                        }
                    });

                if bundle_viewer_button.clicked() {
                    let filepath = marked_point.unwrap();
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        tokio::process::Command::new("bundle-viewer")
                            .arg(filepath)
                            .spawn()
                            .unwrap();
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        let search = serde_qs::to_string(&ViewerMode::Bundles {
                            filepath: PathBuf::from(filepath),
                        })
                        .unwrap();
                        window()
                            .unwrap()
                            .open_with_url(&format!("/?{search}"))
                            .unwrap();
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
