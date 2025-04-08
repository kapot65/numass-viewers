use std::{collections::BTreeMap, path::Path};
use std::path::PathBuf;
use std::sync::Arc;

use eframe::{
    egui::{self, mutex::Mutex, Ui},
    epaint::Color32,
};
use egui::Visuals;
use egui_plot::{HLine, Legend, Plot, PlotPoint, Points, VLine};

use processing::{
    histogram::PointHistogram,
    preprocess::Preprocess,
    storage::LoadState,
    utils::construct_filename,
    viewer::{ViewerState, EMPTY_POINT},
    widgets::UserInput,
};

#[cfg(not(target_arch = "wasm32"))]
use {
    crate::process_point,
    home::home_dir,
    processing::{storage::FSRepr, viewer::PointState},
    tokio::spawn,
    which::which,
};

#[cfg(target_arch = "wasm32")]
use {
    crate::PointProcessor,
    eframe::web_sys::window,
    gloo::{
        net::http::Request,
        worker::{oneshot::OneshotBridge, Spawnable},
    },
    processing::{
        storage::{api_url, FSRepr},
        viewer::{PointState, ToROOTOptions, ViewerMode},
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

#[derive(Debug)]
struct FileTreeState {
    pub need_process: bool,
    pub need_load: bool,
}

pub struct DataViewerApp {
    #[cfg(not(target_arch = "wasm32"))]
    pub root: Arc<tokio::sync::Mutex<Option<FSRepr>>>,
    #[cfg(target_arch = "wasm32")]
    pub root: Arc<std::sync::Mutex<Option<FSRepr>>>,

    select_single: bool,

    /// Фильтр по имени файла (прячет файлы, не содержащие подстроки в имени в виджете файлового дерева)
    name_contains: String,

    plot_mode: PlotMode,
    processing_params: ViewerState,
    current_path: Option<String>,

    processing_status: Arc<Mutex<ProcessingStatus>>,
    state: Arc<Mutex<BTreeMap<String, PointState>>>,

    #[cfg(target_arch = "wasm32")]
    processor_pool: Vec<OneshotBridge<PointProcessor>>,
}

impl DataViewerApp {
    /// Draws processing parameters editor and handles input from user.
    ///
    /// Updated values will be written to [processing_params](DataViewerApp::processing_params) immediately.
    fn params_editor(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let process = self.processing_params.process.input(ui, ctx);

        ui.separator();

        let post_process = self.processing_params.post_process.input(ui, ctx);

        ui.separator();

        let histogram = self.processing_params.histogram.input(ui, ctx);

        let changed = self.processing_params.changed
            || (process != self.processing_params.process
                || post_process != self.processing_params.post_process
                || histogram != self.processing_params.histogram);

        self.processing_params = ViewerState {
            process,
            post_process,
            histogram,
            changed,
        };
    }

    /// files open button with logic embedded
    fn files_open_button(&mut self, ui: &mut Ui) {
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
                    let root_local = resp.json::<FSRepr>().await.unwrap();
                    root.lock().unwrap().replace(root_local);
                }
            });
        }
    }

    /// files reload button with logic embedded
    /// # Arguments
    ///
    /// * `root` - delocked [root](DataViewerApp::root) instance (used to prevent multiple lockings since we should already have a copy).
    ///
    fn files_reload_button(&mut self, ui: &mut Ui, root: &Option<FSRepr>) {
        let path = root.clone().map(|root| root.to_filename());
        if path.is_some() && ui.button("reload").clicked() {
            if let Some(mut root) = root.clone() {
                let root_out = Arc::clone(&self.root);

                spawn(async move {
                    root.update_reccurently().await;
                    if let Ok(mut out) = root_out.try_lock() {
                        out.replace(root);
                    }
                });
            }
        }
    }

    /// files process input (spinner + apply button) with logic embedded
    fn files_process_button(&mut self, ui: &mut Ui) {
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
    }

    fn files_save_button(&mut self, ui: &mut Ui) {
        if ui.button("save").clicked() {
            let state = self.state.lock().clone();
            let plot_mode = self.plot_mode;
            let processing_params = self.processing_params.clone();

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
                            DataViewerApp::files_save_histograms(&save_folder, &state_sorted)
                        }
                        PlotMode::PPT => {
                            DataViewerApp::files_save_ppt(
                                &save_folder,
                                &state_sorted,
                                &processing_params,
                            );
                        }
                        PlotMode::PPV => {
                            DataViewerApp::files_save_ppv(
                                &save_folder,
                                &state_sorted,
                                &processing_params,
                            );
                        }
                    }
                }
            });
        }
    }

    fn files_save_root_button(&mut self, ui: &mut Ui) {
        if ui.button("save(root)").clicked() {
            let state = self.state.lock().clone();

            #[cfg(target_arch = "wasm32")]
            {
                for (name, cache) in state.iter() {
                    if let PointState { opened: true, .. } = cache {
                        let search = serde_qs::to_string(&ToROOTOptions {
                            filepath: PathBuf::from(name),
                            process: self.processing_params.process.clone(),
                            postprocess: self.processing_params.post_process,
                        })
                        .unwrap();
                        window()
                            .unwrap()
                            .open_with_url(&format!("/api/to-root?{search}"))
                            .unwrap();
                    }
                }
            }

            #[cfg(not(target_arch = "wasm32"))]
            {
                let processing_params = self.processing_params.clone();

                spawn(async move {
                    let save_folder = rfd::FileDialog::new()
                        .set_directory(home_dir().unwrap())
                        .pick_folder();
                    // #[cfg(target_arch = "wasm32")]
                    // let save_folder = Some(PathBuf::new());

                    if let Some(save_folder) = save_folder {
                        let state_sorted = {
                            let mut state = state.iter().collect::<Vec<_>>();
                            state.sort_by(|(key_1, _), (key_2, _)| natord::compare(key_1, key_2));
                            state
                        };

                        // let mut out_names = String::new();

                        for (name, cache) in state_sorted.iter() {
                            if let PointState { opened: true, .. } = cache {
                                let out_name = construct_filename(name, Some("root"));

                                // if cache.opened {
                                //     out_names += &format!("{}\n", out_name);
                                // }

                                let mut command = tokio::process::Command::new("convert-to-root");
                                command
                                    .arg(name)
                                    .arg("--process")
                                    .arg(serde_json::to_string(&processing_params.process).unwrap())
                                    .arg("--postprocess")
                                    .arg(
                                        serde_json::to_string(&processing_params.post_process)
                                            .unwrap(),
                                    )
                                    .arg("--output")
                                    .arg(save_folder.join(PathBuf::from(out_name)));

                                command.spawn().unwrap();
                            }
                        }
                        // DataViewerApp::save_text_file(&save_folder, "opened", Some("tsv"), &out_names);
                    }
                });
            };
        }
    }

    /// Draws file editor and handles user inputs.
    fn files_editor(&mut self, ui: &mut Ui) {
        let mut root_copy = {
            if let Ok(root) = self.root.try_lock() {
                root.clone()
            } else {
                ui.spinner();
                return;
            }
        };

        let mut needs_to_be_marked = false;

        ui.checkbox(&mut self.select_single, "select single");

        ui.horizontal(|ui| {
            ui.label("name contains:");
            ui.add_sized(
                [100.0, 20.0],
                egui::TextEdit::singleline(&mut self.name_contains),
            );
            needs_to_be_marked = ui
                .button("+")
                .on_hover_text("Выделить все видимые файлы")
                .clicked();
        });

        ui.horizontal(|ui| {
            self.files_open_button(ui);

            self.files_reload_button(ui, &root_copy);

            self.files_process_button(ui);

            if ui.button("clear").clicked() {
                self.state.lock().clear()
            }

            self.files_save_button(ui);
        });

        self.files_save_root_button(ui);

        egui::containers::ScrollArea::new([false, true]).show(ui, |ui| {
            if let Some(root) = &mut root_copy {
                let mut state_after = FileTreeState {
                    need_load: false,
                    need_process: false,
                };

                DataViewerApp::file_tree_entry(
                    ui,
                    root,
                    &self.select_single,
                    &self.name_contains,
                    needs_to_be_marked,
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
                        root.expand_reccurently().await;
                        if let Ok(mut out) = root_out.try_lock() {
                            out.replace(root);
                        }
                    });
                }
            }
        });
    }

    /// Recursive file tree drawer with logic embedded
    fn file_tree_entry(
        ui: &mut egui::Ui,
        entry: &mut FSRepr,
        select_single: &bool,
        name_contains: &str,
        needs_to_be_marked: bool,
        opened_files: &mut BTreeMap<String, PointState>,
        state_after: &mut FileTreeState,
    ) {
        match entry {
            FSRepr::File { path, .. } => {
                let key = path.to_str().unwrap().to_string();
                if name_contains.is_empty() || key.contains(name_contains) {
                    let cache = opened_files.entry(key.clone()).or_insert(EMPTY_POINT);
                    let mut change_set = None;
                    let mut exclusive_point = None;

                    ui.horizontal(|ui| {
                        if needs_to_be_marked {
                            cache.opened = true;
                        }

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
                            ui.hyperlink_to(filename, api_url("api/meta", path));
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
            }

            FSRepr::Directory {
                path,
                children,
                load_state,
                ..
            } => {
                let header =
                    egui::CollapsingHeader::new(path.file_name().unwrap().to_str().unwrap())
                        .id_salt(path.to_str().unwrap())
                        .show(ui, |ui| {
                            for child in children {
                                DataViewerApp::file_tree_entry(
                                    ui,
                                    child,
                                    select_single,
                                    name_contains,
                                    needs_to_be_marked,
                                    opened_files,
                                    state_after,
                                )
                            }
                        });

                if header.fully_open() && load_state == &LoadState::NotLoaded {
                    *load_state = LoadState::NeedLoad;
                    state_after.need_load = true;
                }
            }
        }
    }

    /// Isomorphic way to save currentry opened files in [PlotMode::PPV] mode
    ///
    /// Result will be saved in `PPV.tsv` file in a place according [DataViewerApp::save_text_file]
    ///
    /// # Arguments
    /// * `save_folder` - Directory where the file should be saved (on wasm side can be any).
    /// * `state_sorted` - A ref copy of [DataViewerApp::state] converted to vec (must be sorted for pretty results).
    /// * `processing_params` - A ref copy of [ViewerState] to get processing parameters.
    ///
    fn files_save_ppv(
        save_folder: &Path,
        state_sorted: &Vec<(&String, &PointState)>,
        processing_params: &ViewerState,
    ) {
        let mut content = String::new();
        {
            content.push_str("path\tvoltage\tcount_rate\tcounts\teffective_time\n");
        }

        for (name, cache) in state_sorted.iter() {
            if let PointState {
                counts: Some(counts),
                preprocess: Some(preprocess),
                ..
            } = cache
            {
                let effective_time = if processing_params.post_process.cut_bad_blocks {
                    preprocess.effective_time() as f32 * 1e-9
                } else {
                    preprocess.acquisition_time as f32 * 1e-9
                };

                let count_rate = *counts as f32 / effective_time;

                let point_name = {
                    let temp = PathBuf::from(name);
                    temp.file_name().unwrap().to_owned()
                };

                content.push_str(&format!(
                    "{point_name:?}\t{}\t{count_rate}\t{counts}\t{effective_time}\n",
                    preprocess.hv
                ));
            }
        }

        DataViewerApp::save_text_file(save_folder, "PPV", Some("tsv"), &content);
    }

    /// Isomorphic way to save currentry opened files in [PlotMode::PPT] mode
    ///
    /// Result will be saved in `PPT.tsv` file in a place according [DataViewerApp::save_text_file]
    ///
    /// # Arguments
    /// * `save_folder` - Directory where the file should be saved (on wasm side can be any).
    /// * `state_sorted` - A ref copy of [DataViewerApp::state] converted to vec (must be sorted for pretty results).
    /// * `processing_params` - A ref copy of [ViewerState] to get processing parameters.
    ///
    fn files_save_ppt(
        save_folder: &Path,
        state_sorted: &Vec<(&String, &PointState)>,
        processing_params: &ViewerState,
    ) {
        let mut content = String::new();
        {
            content.push_str("path\ttime\ttime_raw\tcount_rate\tcounts\teffective_time\n");
        }

        for (name, cache) in state_sorted.iter() {
            if let PointState {
                counts: Some(counts),
                preprocess: Some(preprocess),
                ..
            } = cache
            {
                let effective_time = if processing_params.post_process.cut_bad_blocks {
                    preprocess.effective_time() as f32 * 1e-9
                } else {
                    preprocess.acquisition_time as f32 * 1e-9
                };

                let count_rate = *counts as f32 / effective_time;

                let point_name = {
                    let temp = PathBuf::from(name);
                    temp.file_name().unwrap().to_owned()
                };

                let start_time = preprocess.start_time;

                content.push_str(&format!(
                    "{point_name:?}\t{start_time:?}\t{}\t{count_rate}\t{counts}\t{effective_time}\n",
                    start_time.and_utc().timestamp()
                ));
            }
        }

        DataViewerApp::save_text_file(save_folder, "PPT", Some("tsv"), &content);
    }

    /// Isomorphic way to save currentry opened files in [PlotMode::Histogram] mode
    ///
    /// This function will save each opened (and processed) file in a separate tsv file
    /// and a combined one in `merged.tsv`
    ///
    /// - For generated names see [DataViewerApp::save_text_file]
    /// - For data structure see [PointHistogram::to_csv]
    ///
    /// # Arguments
    /// * `save_folder` - Directory where the file should be saved (on wasm side can be any).
    /// * `state` - A ref copy of [DataViewerApp::state] converted to vec.
    ///
    fn files_save_histograms(save_folder: &Path, state: &Vec<(&String, &PointState)>) {
        let opened_hists = state
            .iter()
            .filter_map(|(name, cache)| {
                if let PointState {
                    opened: true,
                    histogram: Some(histogram),
                    ..
                } = cache
                {
                    Some((name, histogram))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        // Save each hist into separate file
        for (name, histogram) in &opened_hists {
            let data = histogram.to_csv('\t');
            DataViewerApp::save_text_file(save_folder, name, Some("tsv"), &data);
        }

        // Save merged histogram
        let merged_hist = PointHistogram::new_merged(
            &opened_hists
                .into_iter()
                .map(|(_, hist)| hist)
                .collect::<Vec<_>>(),
        );
        let merged_data = merged_hist.to_csv('\t');
        DataViewerApp::save_text_file(save_folder, "merged", Some("tsv"), &merged_data);
    }

    /// Isomorphic text file save
    ///
    /// - Native: method will save file to `$save_folder/$name` via fs
    /// - WASM: method will download file via browser
    ///
    /// Method will try to extract set name (parent folder) and run name (parent of parent folder).
    /// On success filename will be like `{run_name}-{set_name}-{name}.{pref_ext}`. On failure it will use just `{name}`.
    ///
    /// # Arguments
    ///
    /// * `save_folder` - Directory where the file should be saved (on wasm side can be any).
    /// * `name` - Desired filename (preferred without extension).
    /// * `pref_ext` - Optional desired file extension (if None - nothing will be added).
    /// * `content` - Text file content.
    ///
    fn save_text_file(save_folder: &Path, name: &str, pref_ext: Option<&str>, content: &str) {
        #[cfg(target_arch = "wasm32")]
        let _ = save_folder;
        #[cfg(target_arch = "wasm32")]
        let save_folder = PathBuf::new(); // ensure correctness

        let filename = construct_filename(name, pref_ext);

        let mut filepath = PathBuf::from(save_folder);
        filepath.push(filename);

        #[cfg(not(target_arch = "wasm32"))]
        {
            std::fs::write(filepath, content).unwrap();
        }
        #[cfg(target_arch = "wasm32")]
        download(filepath.to_str().unwrap(), content);
    }

    fn process(&mut self) {
        let changed = self.processing_params.changed;
        self.processing_params.changed = false;

        let params = self.processing_params.clone();
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
            name_contains: "".to_string(),
            state,
            current_path: None,
            processing_status,
            processing_params: ViewerState::default(),
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
}

impl eframe::App for DataViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_visuals(Visuals::dark());
        
        ctx.request_repaint_after(std::time::Duration::from_secs(1));

        egui::SidePanel::left("left").show(ctx, |ui| {
            self.params_editor(ui, ctx);

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
                        .x_axis_formatter(|mark, _| {
                            chrono::DateTime::from_timestamp_millis(mark.value as i64)
                                .unwrap()
                                .to_string()
                        })
                        .height(height - 35.0);

                    plot.show(ui, |plot_ui| {
                        let points = opened_files
                            .iter()
                            .filter_map(|(_, cache)| {
                                if let PointState {
                                    counts: Some(counts),
                                    preprocess: Some(preprocess),
                                    ..
                                } = cache
                                {
                                    if self.processing_params.post_process.cut_bad_blocks {
                                        Some([
                                            preprocess.start_time.and_utc().timestamp_millis()
                                                as f64,
                                            *counts as f64
                                                / (preprocess.effective_time() as f64 * 1e-9),
                                        ])
                                    } else {
                                        Some([
                                            preprocess.start_time.and_utc().timestamp_millis()
                                                as f64,
                                            *counts as f64
                                                / (preprocess.acquisition_time as f64 * 1e-9),
                                        ])
                                    }
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>();

                        plot_ui.points(Points::new("PPT", points).radius(3.0));
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
                                    counts: Some(counts),
                                    preprocess: Some(preprocess),
                                    ..
                                } = cache
                                {
                                    if self.processing_params.post_process.cut_bad_blocks {
                                        Some([
                                            preprocess.hv as f64,
                                            *counts as f64
                                                / (preprocess.effective_time() as f64 * 1e-9),
                                        ])
                                    } else {
                                        Some([
                                            preprocess.hv as f64,
                                            *counts as f64
                                                / (preprocess.acquisition_time as f64 * 1e-9),
                                        ])
                                    }
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>();

                        plot_ui.points(Points::new("PPV", points).radius(3.0));

                        if plot_ui.response().clicked() {
                            if let Some(pos) = plot_ui.pointer_coordinate() {
                                let clicked_file = opened_files
                                    .iter()
                                    .filter_map(|(path, cache)| {
                                        if let PointState {
                                            counts: Some(counts),
                                            preprocess: Some(preprocess),
                                            ..
                                        } = cache
                                        {
                                            // TODO: deduplicate this code
                                            let point_pos = if self
                                                .processing_params
                                                .post_process
                                                .cut_bad_blocks
                                            {
                                                PlotPoint::new(
                                                    preprocess.hv,
                                                    *counts as f64
                                                        / preprocess.effective_time() as f64
                                                        * 1e-9,
                                                )
                                            } else {
                                                PlotPoint::new(
                                                    preprocess.hv,
                                                    *counts as f64
                                                        / preprocess.acquisition_time as f64
                                                        * 1e-9,
                                                )
                                            };

                                            let distance =
                                                point_pos.to_pos2().distance(pos.to_pos2());
                                            if distance < 1e5 {
                                                return Some((path, distance));
                                            }
                                            None
                                        } else {
                                            None
                                        }
                                    })
                                    .min_by_key(|(_, distance)| (distance * 1000.0) as i64);

                                if let Some((path, _)) = clicked_file {
                                    let path = (**path).clone();
                                    self.current_path =
                                        if let Some(p) = self.current_path.to_owned() {
                                            if p != path {
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
                                counts: Some(counts),
                                preprocess:
                                    Some(Preprocess {
                                        acquisition_time,
                                        hv,
                                        ..
                                    }),
                                ..
                            } = state[current]
                            {
                                plot_ui.hline(
                                    HLine::new("selection", counts as f64 / (acquisition_time as f64 * 1e-9))
                                        .color(Color32::WHITE),
                                );
                                plot_ui.vline(VLine::new("selection", hv).color(Color32::WHITE));
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

                if filtered_viewer_button.clicked() {
                    let filepath = marked_point.unwrap();

                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let mut command = tokio::process::Command::new("filtered-viewer");

                        command
                            .arg(filepath)
                            .arg("--process")
                            .arg(serde_json::to_string(&self.processing_params.process).unwrap())
                            .arg("--postprocess")
                            .arg(
                                serde_json::to_string(&self.processing_params.post_process)
                                    .unwrap(),
                            );

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
                            process: self.processing_params.process.clone(),
                            postprocess: self.processing_params.post_process,
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
                        let search = serde_qs::to_string(&ViewerMode::Triggers {
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
                            .arg("--process")
                            .arg(serde_json::to_string(&self.processing_params.process).unwrap())
                            .arg("--postprocess")
                            .arg(
                                serde_json::to_string(&self.processing_params.post_process)
                                    .unwrap(),
                            )
                            .spawn()
                            .unwrap();
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        let search = serde_qs::to_string(&ViewerMode::Bundles {
                            filepath: PathBuf::from(filepath),
                            process: self.processing_params.process.clone(),
                            postprocess: self.processing_params.post_process,
                        })
                        .unwrap();
                        window()
                            .unwrap()
                            .open_with_url(&format!("/?{search}"))
                            .unwrap();
                    }
                }

                ui.radio_value(&mut self.plot_mode, PlotMode::Histogram, "Hist");
                ui.radio_value(&mut self.plot_mode, PlotMode::PPT, "PPT");
                ui.radio_value(&mut self.plot_mode, PlotMode::PPV, "PPV");
            });
        });
    }
}
