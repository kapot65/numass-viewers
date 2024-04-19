#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#[cfg(target_arch = "wasm32")]
fn main() {
    panic!("this binary is not meant to be run in browser")
}
#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() {
    use clap::Parser;
    use viewers::filtered_viewer::FilteredViewer;

    #[derive(Parser, Debug)]
    #[clap(author, version, about, long_about = None)]
    struct Opt {
        /// point file location
        filepath: std::path::PathBuf,
        /// minimal amplitude in range
        #[clap(long, default_value_t = 0.0)]
        min: f32,
        /// maximal amplitude in range
        #[clap(long, default_value_t = 27.0)]
        max: f32,
        /// processing params serialized to json
        #[clap(long)]
        processing: Option<String>
    }

    let args = Opt::parse();
    let filepath = args.filepath;
    let range = args.min..args.max;

    let processing = if let Some(processing) = args.processing {
        serde_json::from_str(&processing).expect("cant parse algorithm param")
    } else {
        processing::process::ProcessParams::default()
    };

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        format!("filtered {filepath:?}").as_str(),
        native_options,
        Box::new(move |_| {
            Box::new(FilteredViewer::init_with_point(filepath, processing, range))
        }),
    )
    .unwrap();
}
