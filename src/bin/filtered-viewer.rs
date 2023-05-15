#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#[cfg(target_arch = "wasm32")]
fn main() {
    todo!()
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
        /// size of neighboor events will be shown (in nanoseconds)
        #[clap(long, default_value_t = 5000)]
        neighborhood: usize,
        /// convert amplitudes to kev
        #[clap(long)]
        convert_kev: bool,
        /// algorithm params serialized to json (default Max)
        #[clap(long)]
        algorithm: Option<String>
    }

    let args = Opt::parse();
    let filepath = args.filepath;
    let neighborhood = args.neighborhood;
    let range = args.min..args.max;
    let convert_kev = args.convert_kev;

    let algorithm = if let Some(algorithm) = args.algorithm {
        serde_json::from_str(&algorithm).expect("cant parse algorithm param")
    } else {
        processing::Algorithm::Max
    };

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        format!("filtered {filepath:?}").as_str(),
        native_options,
        Box::new(move |_| {
            Box::new(FilteredViewer::init_with_point(filepath, algorithm, range, convert_kev, neighborhood))
        }),
    )
    .unwrap();
}
