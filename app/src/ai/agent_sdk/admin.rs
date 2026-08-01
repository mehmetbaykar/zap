//! General-purpose administrative commands in the Zap CLI.

use anyhow::Result;
use serde::Serialize;
use warp_cli::agent::OutputFormat;
use warpui::AppContext;
use warpui::platform::TerminationMode;

#[derive(Serialize)]
struct WhoamiOutput {
    uid: &'static str,
    #[serde(rename = "type")]
    principal_type: &'static str,
}

/// Print the local identity used by the de-clouded CLI.
pub fn whoami(ctx: &mut AppContext, output_format: OutputFormat) -> Result<()> {
    let info = WhoamiOutput {
        uid: "local",
        principal_type: "local",
    };

    match output_format {
        OutputFormat::Json => println!("{}", serde_json::to_string(&info)?),
        OutputFormat::Pretty => println!("Local user: {}", info.uid),
        OutputFormat::Text => println!("{}:{}", info.principal_type, info.uid),
        OutputFormat::Ndjson => println!("{}", serde_json::to_string(&info)?),
    }

    ctx.terminate_app(TerminationMode::ForceTerminate, None);

    Ok(())
}
