#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_arch = "wasm32"))]
use {
    chrono::DateTime,
    chrono::NaiveDateTime,
    eframe::egui::{self, mutex::Mutex, Ui},
    egui_plot::{Legend, Line, Plot, Points},
    processing::numass::{self, ExternalMeta, NumassMeta, Reply},
    processing::storage::load_meta,
    processing::storage::FSRepr,
    processing::storage::LoadState,
    serde::{Deserialize, Serialize},
    std::collections::BTreeMap,
    std::io::BufRead,
    std::path::PathBuf,
    std::sync::Arc,
    std::time::SystemTime,
    tokio::spawn,
};

#[cfg(target_family = "unix")]
use tikv_jemallocator::Jemalloc;
#[cfg(target_family = "unix")]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

#[cfg(target_arch = "wasm32")]
fn main() {
    panic!("this binary is not meant to be run in browser")
}
#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() {
    use clap::Parser;
    use egui_extras::install_image_loaders;

    #[derive(Parser, Debug)]
    #[clap(author, version, about, long_about = None)]
    struct Opt {
        #[clap(long)]
        directory: Option<PathBuf>,
        #[clap(long)]
        cache_directory: Option<String>,
    }

    let opt = Opt::parse();

    // Log to stdout (if you run with `RUST_LOG=debug`).
    tracing_subscriber::fmt::init();

    let app = FaradeyViewerApp::default();

    if let Some(directory) = opt.directory {
        *app.root.try_lock().unwrap() = Some(FSRepr::new(directory))
    }

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "faradey viewer",
        native_options,
        Box::new(move |ctx| {
            install_image_loaders(&ctx.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .unwrap();
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(PartialEq, Clone, Copy)]
enum PlotMode {
    Lines,
    Ppt,
    Ppv,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
struct FileTreeState {
    pub need_process: bool,
    pub need_load: bool,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FaradeyPointState {
    pub opened: bool,
    pub modified: Option<SystemTime>,
    pub times_millis: Option<Vec<i64>>, // ? is it good format
    pub values: Option<Vec<f64>>,
    pub start_time: Option<NaiveDateTime>,
    pub hv: Option<f64>,
}

#[cfg(not(target_arch = "wasm32"))]
const EMPTY_FARADEY_POINT: FaradeyPointState = FaradeyPointState {
    opened: false,
    modified: None,
    times_millis: None,
    values: None,
    hv: None,
    start_time: None,
};

// TODO: add error handling
#[cfg(not(target_arch = "wasm32"))]
async fn process_faradey_point(filepath: PathBuf) -> Option<FaradeyPointState> {
    let modified = processing::storage::load_modified_time(filepath.clone()).await; // TODO: remove clone

    let meta = load_meta(&filepath).await;
    let (hv, start_time) = if let Some(NumassMeta::Reply(Reply::AcquirePoint {
        // acquisition_time, // TODO: take start time from meta
        start_time,
        external_meta:
            Some(ExternalMeta {
                hv1_value: Some(hv),
                ..
            }),
        ..
    })) = meta
    {
        (Some(hv as f64), Some(start_time))
    } else {
        (None, None)
    };

    let table_data = if let Ok(mut point_file) = tokio::fs::File::open(&filepath).await {
        let message = dataforge::read_df_message::<numass::NumassMeta>(&mut point_file)
            .await
            .unwrap();
        message.data.unwrap()
    } else {
        panic!("{filepath:?} open failed")
    };

    let (times_millis, values): (Vec<_>, Vec<_>) = table_data
        .lines()
        .skip(1)
        .filter_map(|line| {
            line.map(|line| {
                let parts = line.split('\t').collect::<Vec<_>>();

                let timestamp = DateTime::parse_from_rfc3339(parts[0]).unwrap();

                let value = parts[1].parse::<f64>().unwrap();
                (timestamp.timestamp_millis(), value)
            })
            .ok()
        })
        .unzip();

    Some(FaradeyPointState {
        modified,
        opened: true,
        times_millis: Some(times_millis),
        values: Some(values),
        start_time,
        hv,
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub struct FaradeyViewerApp {
    pub root: Arc<tokio::sync::Mutex<Option<FSRepr>>>,

    select_single: bool,

    /// Фильтр по имени файла (прячет файлы, не содержащие подстроки в имени в виджете файлового дерева)
    name_contains: String,

    plot_mode: PlotMode,
    state: Arc<Mutex<BTreeMap<String, FaradeyPointState>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl FaradeyViewerApp {
    /// files open button with logic embedded
    fn files_open_button(&mut self, ui: &mut Ui) {
        if ui.button("open").clicked() {
            let root = Arc::clone(&self.root);
            spawn(async move {
                if let Some(root_path) = rfd::FileDialog::new().pick_folder() {
                    root.lock().await.replace(FSRepr::new(root_path));
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
        if ui.button("apply").clicked() {
            self.process()
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
        });

        egui::containers::ScrollArea::new([false, true]).show(ui, |ui| {
            if let Some(root) = &mut root_copy {
                let mut state_after = FileTreeState {
                    need_load: false,
                    need_process: false,
                };

                FaradeyViewerApp::file_tree_entry(
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
        opened_files: &mut BTreeMap<String, FaradeyPointState>,
        state_after: &mut FileTreeState,
    ) {
        match entry {
            FSRepr::File { path, .. } => {
                let key = path.to_str().unwrap().to_string();
                if name_contains.is_empty() || key.contains(name_contains) {
                    let cache = opened_files
                        .entry(key.clone())
                        .or_insert(EMPTY_FARADEY_POINT);
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
                        ui.label(filename);
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
                                FaradeyViewerApp::file_tree_entry(
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

    pub fn process(&mut self) {
        let state = Arc::clone(&self.state);

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

        for filepath in files_to_processed {
            let configuration_local = state.clone();

            spawn(async move {
                let point_state = process_faradey_point(filepath.clone().into()).await;

                let point_state = point_state.unwrap_or(EMPTY_FARADEY_POINT);

                let mut conf: egui::mutex::MutexGuard<'_, BTreeMap<String, FaradeyPointState>> =
                    configuration_local.lock();
                conf.insert(filepath.to_owned(), point_state);
            });
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for FaradeyViewerApp {
    fn default() -> Self {
        let state = Arc::new(Mutex::new(BTreeMap::new()));

        Self {
            root: Arc::new(tokio::sync::Mutex::new(None)),
            select_single: false,
            name_contains: "".to_string(),
            state,
            plot_mode: PlotMode::Lines,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl eframe::App for FaradeyViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));

        egui::SidePanel::left("left").show(ctx, |ui| {
            ui.separator();

            self.files_editor(ui);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let state = self.state.lock();

            let mut left_border = 0.0;
            let mut right_border = 0.0;

            let opened_files = state
                .iter()
                .filter(|(_, cache)| cache.opened)
                .collect::<Vec<_>>();

            let height = {
                let mut y = 0.0;
                ctx.input(|i| y = i.viewport().inner_rect.unwrap().size().y);
                y
            };
            match self.plot_mode {
                PlotMode::Lines => {
                    let plot = Plot::new("Lines Plot")
                        .legend(Legend::default())
                        .height(height - 35.0);

                    plot.show(ui, |plot_ui| {
                        let bounds = plot_ui.plot_bounds();
                        left_border = bounds.min()[0] as f32;
                        right_border = bounds.max()[0] as f32;

                        opened_files.iter().for_each(|(name, cache)| {
                            if let FaradeyPointState {
                                opened: true,
                                times_millis: Some(times_millis),
                                values: Some(values),
                                ..
                            } = cache
                            {
                                let x = times_millis
                                    .iter()
                                    .map(|&t| (t - times_millis[0]) as f64 * 1e-3)
                                    .collect::<Vec<_>>();

                                plot_ui.line(Line::new(
                                    name.to_owned(),
                                    x.iter()
                                        .zip(values)
                                        .map(|(&x, &y)| [x, y])
                                        .collect::<Vec<_>>(),
                                ));
                            }
                        })
                    });
                }
                PlotMode::Ppt => {
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
                                if let FaradeyPointState {
                                    values: Some(values),
                                    start_time: Some(start_time),
                                    ..
                                } = cache
                                {
                                    let mean = values.iter().sum::<f64>() / values.len() as f64;
                                    Some([start_time.and_utc().timestamp_millis() as f64, mean])
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>();

                        plot_ui.points(Points::new("PPT", points).radius(3.0));
                    });
                }
                PlotMode::Ppv => {
                    let plot = Plot::new("Point/Voltage")
                        .legend(Legend::default())
                        .height(height - 35.0);

                    plot.show(ui, |plot_ui| {
                        let points = opened_files
                            .iter()
                            .filter_map(|(_, cache)| {
                                if let FaradeyPointState {
                                    values: Some(values),
                                    hv: Some(hv),
                                    ..
                                } = cache
                                {
                                    let mean = values.iter().sum::<f64>() / values.len() as f64;
                                    Some([*hv, mean])
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>();

                        plot_ui.points(Points::new("PPV", points).radius(3.0));
                    });
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                ui.radio_value(&mut self.plot_mode, PlotMode::Lines, "Lines");
                ui.radio_value(&mut self.plot_mode, PlotMode::Ppt, "PPT");
                ui.radio_value(&mut self.plot_mode, PlotMode::Ppv, "PPV");
            });
        });
    }
}
