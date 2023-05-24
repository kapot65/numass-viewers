#![warn(clippy::all, rust_2018_idioms)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use viewers::app;

// When compiling natively:
#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> eframe::Result<()> {
    use backend::{expand_dir, CACHE_DIRECTORY};
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
    if let Some(cache_directory) = opt.cache_directory {
        if std::env::var(CACHE_DIRECTORY).is_err() {
            std::env::set_var(CACHE_DIRECTORY, cache_directory)
        } else {
            panic!("cache directory is set via CLI and ENV at the same time!")
        }
    }

    // Log to stdout (if you run with `RUST_LOG=debug`).
    tracing_subscriber::fmt::init();

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "data-viewer",
        native_options,
        Box::new(|_| {
            let app = app::DataViewerApp::new();
            if let Some(directory) = opt.directory {
                *app.root.lock() = expand_dir(directory)
            }
            Box::new(app)
        }),
    )
}

// when compiling to web using trunk.
#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::web_sys::window;
    use backend::ProcessRequest;
    use viewers::{
        filtered_viewer, point_viewer
    };
    use wasm_bindgen_futures::spawn_local;

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
            serde_qs::from_str::<ProcessRequest>(search).ok()
        }
        _ => None,
    };

    let web_runner = eframe::WebRunner::new();
    let web_options = eframe::WebOptions::default();

    if let Some(ProcessRequest::FilterEvents {
        filepath,
        range,
        neighborhood,
        algorithm,
        convert_kev
    }) = request
    {
        set_title(format!("filtered {filepath:?}").as_str());
        spawn_local(async move {
            web_runner.start(
                "the_canvas_id", // hardcode it
                web_options,
                Box::new(move |_| {
                    let app = filtered_viewer::FilteredViewer::init_with_point(
                        filepath, 
                        algorithm, 
                        range, 
                        convert_kev, 
                        neighborhood);
                    Box::new(app)
                }),
            )
            .await
            .expect("failed to start eframe");
        })
    } else if let Some(ProcessRequest::SplitTimeChunks { filepath }) = request {

        set_title(filepath.to_str().unwrap());

        spawn_local(async move {
            web_runner.start(
                "the_canvas_id", // hardcode it
                web_options,
                Box::new(|_| {
                    Box::new(point_viewer::PointViewer::init_with_point(filepath))
                }),
            )
            .await
            .expect("failed to start eframe");
        })
    } else {
        spawn_local(async move {
            web_runner.start(
                "the_canvas_id", // hardcode it
                web_options,
                Box::new(|_| {
                    let app = app::DataViewerApp::new();
                    Box::new(app)
                }),
            )
            .await
            .expect("failed to start eframe");
        });
    }
}
