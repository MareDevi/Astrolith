#[cfg(debug_assertions)]
mod spikes {
    pub mod editor;
    pub mod layout;
}

#[cfg(debug_assertions)]
use gpui_kit::{AppContext, WindowOptions};

/// The M00 spikes are development-only windows. A release binary that is asked
/// for one must say so instead of falling through to the application window.
#[cfg(not(debug_assertions))]
fn refuse_spike_request() {
    if let Some(argument) = std::env::args()
        .skip(1)
        .find(|argument| argument.starts_with("--spike="))
    {
        eprintln!(
            "{argument} is unavailable: the M00 spikes are compiled only in development builds."
        );
        std::process::exit(2);
    }
}

fn main() {
    if std::env::args()
        .skip(1)
        .any(|argument| argument == "--version" || argument == "-V")
    {
        println!("astrolith {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let config = astrolith_observability::LogConfig::from_env();
    for warning in &config.warnings {
        eprintln!("Logging configuration: {warning}");
    }
    let log_guard = match astrolith_observability::init(config) {
        Ok(guard) => Some(guard),
        Err(error) => {
            eprintln!("Could not initialize logging: {error}");
            None
        }
    };
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        pid = std::process::id(),
        log_directory = ?log_guard
            .as_ref()
            .and_then(astrolith_observability::LogGuard::log_directory),
        "Astrolith starting"
    );

    #[cfg(not(debug_assertions))]
    refuse_spike_request();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            #[cfg(debug_assertions)]
            {
                let editor_spike = std::env::args().any(|argument| argument == "--spike=editor");
                let layout_spike = std::env::args().any(|argument| argument == "--spike=layout");
                if layout_spike {
                    tracing::info!("starting the layout spike");
                } else if editor_spike {
                    tracing::info!("starting the editor spike");
                }

                if layout_spike {
                    if let Err(error) =
                        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                            window.set_window_title("Astrolith · Layout spike");
                            cx.new(|cx| spikes::layout::LayoutSpike::new(window, cx))
                        })
                    {
                        tracing::error!(error = %format_args!("{error:#}"), "could not open the layout spike");
                        eprintln!("Could not open the layout spike: {error:#}");
                        std::process::exit(1);
                    }
                    return;
                }

                if editor_spike {
                    spikes::editor::EditorSpike::bind_keys(cx);
                    if let Err(error) =
                        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                            window.set_window_title("Astrolith · Editor spike");
                            cx.new(|cx| spikes::editor::EditorSpike::new(window, cx))
                        })
                    {
                        tracing::error!(error = %format_args!("{error:#}"), "could not open the editor spike");
                        eprintln!("Could not open the editor spike: {error:#}");
                        std::process::exit(1);
                    }
                    return;
                }
            }

            astrolith_gpui::open(cx);
        });
    tracing::info!("Astrolith exiting");
    drop(log_guard);
}
