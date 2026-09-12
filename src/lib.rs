pub mod app;
pub mod cf_html;
pub mod clip;
pub mod config;
pub mod cp1252;
pub mod doc;
pub mod download;
pub mod facts;
pub mod html_text;
pub mod icon;
pub mod install;
pub mod markdown;
pub mod model;
pub mod paths;
pub mod remote;
pub mod render;
pub mod rewrite;
pub mod rules;
pub mod startup;
pub mod tray;
pub mod window;

/// Work reaching the event loop from somewhere that cannot touch the UI.
///
/// The webview is not `Send`, so hotkey callbacks and, later, the rewrite
/// worker all have to arrive as events rather than calling into it directly.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Hotkey,
    /// The dropdown changed. Re-runs the model pass on what is on show.
    SetMode(config::Mode),
    /// Put the result currently on show back on the clipboard, for when
    /// something else has since overwritten it.
    Copy,
    /// A rewrite finished. The id says which hotkey press it belongs to, so a
    /// slow one cannot overwrite newer work.
    RewriteFinished {
        job: u64,
        result: Result<doc::Doc, String>,
    },
    /// The tray toggle changed, carrying the state it now shows.
    LaunchAtStartup(bool),
    /// The user asked to install the model weights.
    InstallModel,
    DownloadProgress(download::Progress),
    DownloadFinished(Result<(), String>),
    /// The page's title bar was pressed, so the window should follow the mouse.
    Drag,
    /// The maximise button, or a double click on the title bar.
    Maximize,
    /// A click landed in the preview, so the user wants to interact with it.
    Activate,
    Hide,
    Exit,
}
