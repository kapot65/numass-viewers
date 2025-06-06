#![warn(clippy::all, rust_2018_idioms)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use viewers::app;

#[cfg(target_family = "unix")]
use tikv_jemallocator::Jemalloc;
#[cfg(target_family = "unix")]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

// When compiling natively:
#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> eframe::Result<()> {
    use egui_extras::install_image_loaders;
    use processing::storage::FSRepr;
    use {clap::Parser, std::path::PathBuf};

    #[derive(Parser, Debug)]
    #[clap(author, version, about, long_about = None)]
    struct Opt {
        #[clap(long)]
        directory: Option<PathBuf>,
        #[clap(long)]
        cache_directory: Option<String>,
    }

    // abort programm if any of threads panic
    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        orig_hook(panic_info);
        std::process::exit(1);
    }));

    let opt = Opt::parse();

    // Log to stdout (if you run with `RUST_LOG=debug`).
    tracing_subscriber::fmt::init();

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "data-viewer",
        native_options,
        Box::new(|ctx| {
            install_image_loaders(&ctx.egui_ctx);
            let app = app::DataViewerApp::default();
            if let Some(directory) = opt.directory {
                *app.root.try_lock().unwrap() = Some(FSRepr::new(directory))
            }
            Ok(Box::new(app))
        }),
    )
}

// when compiling to web using trunk.
#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::web_sys::{self, window};
    use egui_extras::install_image_loaders;
    use processing::viewer::ViewerMode;
    use viewers::{bundle_viewer, filtered_viewer, point_viewer, trigger_viewer};
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::spawn_local;

    fn get_canvas_element_by_id(canvas_id: &str) -> Option<web_sys::HtmlCanvasElement> {
        let document = web_sys::window()?.document()?;
        let canvas = document.get_element_by_id(canvas_id)?;
        canvas.dyn_into::<web_sys::HtmlCanvasElement>().ok()
    }

    fn get_canvas_element_by_id_or_die(canvas_id: &str) -> web_sys::HtmlCanvasElement {
        get_canvas_element_by_id(canvas_id)
            .unwrap_or_else(|| panic!("Failed to find canvas with id {canvas_id:?}"))
    }

    // Make sure panics are logged using `console.error`.
    console_error_panic_hook::set_once();

    // Redirect tracing to console.log and friends:
    tracing_wasm::set_as_global_default();

    fn set_title(title: &str) {
        window().unwrap().document().unwrap().set_title(title)
    }

    let request = match window().unwrap().location().search() {
        Ok(search) => {
            let search = search.trim_start_matches('?');
            serde_qs::from_str::<ViewerMode>(search).ok()
        }
        _ => None,
    };

    let web_runner = eframe::WebRunner::new();
    let web_options = eframe::WebOptions::default();

    match request {
        Some(ViewerMode::FilteredEvents {
            filepath,
            range,
            process,
            postprocess,
        }) => {
            set_title(format!("filtered {filepath:?}").as_str());
            spawn_local(async move {
                let app = filtered_viewer::FilteredViewer::init_with_point(
                    filepath,
                    process,
                    postprocess,
                    range,
                )
                .await;

                web_runner
                    .start(
                        get_canvas_element_by_id_or_die("the_canvas_id"), // hardcode it
                        web_options,
                        Box::new(move |ctx| {
                            install_image_loaders(&ctx.egui_ctx);
                            Ok(Box::new(app))
                        }),
                    )
                    .await
                    .expect("failed to start eframe");
            })
        }

        Some(ViewerMode::Waveforms { filepath }) => {
            set_title(filepath.to_str().unwrap());

            spawn_local(async move {
                web_runner
                    .start(
                        get_canvas_element_by_id_or_die("the_canvas_id"), // hardcode it
                        web_options,
                        Box::new(|ctx| {
                            install_image_loaders(&ctx.egui_ctx);
                            Ok(Box::new(point_viewer::PointViewer::init_with_point(filepath)))
                        }),
                    )
                    .await
                    .expect("failed to start eframe");
            })
        }

        Some(ViewerMode::Bundles {
            filepath,
            process,
            postprocess,
        }) => {
            set_title(filepath.to_str().unwrap());

            let app = bundle_viewer::BundleViewer::init_with_point(filepath, process, postprocess);

            spawn_local(async move {
                web_runner
                    .start(
                        get_canvas_element_by_id_or_die("the_canvas_id"), // hardcode it
                        web_options,
                        Box::new(|ctx| {
                            install_image_loaders(&ctx.egui_ctx);
                            Ok(Box::new(app))
                        }),
                    )
                    .await
                    .expect("failed to start eframe");
            })
        }

        Some(ViewerMode::Triggers { filepath }) => {
            set_title(filepath.to_str().unwrap());

            spawn_local(async move {
                web_runner
                    .start(
                        get_canvas_element_by_id_or_die("the_canvas_id"), // hardcode it
                        web_options,
                        Box::new(|ctx| {
                            install_image_loaders(&ctx.egui_ctx);
                            Ok(Box::new(trigger_viewer::TriggerViewer::init_with_point(filepath)))
                        }),
                    )
                    .await
                    .expect("failed to start eframe");
            })
        }

        None => {
            spawn_local(async move {

                

                web_runner
                    .start(
                        get_canvas_element_by_id_or_die("the_canvas_id"), // hardcode it
                        web_options,
                        Box::new(|ctx| {
                            install_image_loaders(&ctx.egui_ctx);
                            let app = app::DataViewerApp::default();
                            Ok(Box::new(app))
                        }),
                    )
                    .await
                    .expect("failed to start eframe");
            });
        }
    }
}
