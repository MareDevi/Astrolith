mod spikes {
    pub mod editor;
    pub mod layout;
}

use gpui_kit::{AppContext, WindowOptions};

fn main() {
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

    let editor_spike = std::env::args().any(|argument| argument == "--spike=editor");
    let layout_spike = std::env::args().any(|argument| argument == "--spike=layout");
    if layout_spike {
        tracing::info!("starting the layout spike");
    } else if editor_spike {
        tracing::info!("starting the editor spike");
    }
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

            astrolith_gpui::open(cx);
        });
    tracing::info!("Astrolith exiting");
    drop(log_guard);
}
