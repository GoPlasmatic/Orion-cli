//! `orion-cli streams`: follow a channel's server-sent event stream.
//!
//! The shared client's streaming GET keeps the response under the same
//! ceiling, auth and change-context headers as every buffered request. This
//! command is the presentation: it turns chunks into bytes on stdout and
//! flushes each one, so a pipe sees an event as the server sends it.

use std::io::Write;

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use orion_client::{RequestOptions, paths};

use crate::client::OrionClient;

#[derive(Args)]
#[command(long_about = "Follow a channel's server-sent event stream.\n\n\
        A Standard or Enterprise node may serve a protocol whose answer is a \
        `text/event-stream` (an edition's event feed). The stream is read as \
        it arrives; Ctrl-C closes it.")]
pub struct StreamsCmd {
    #[command(subcommand)]
    command: StreamsSubcommand,
}

#[derive(Subcommand)]
pub enum StreamsSubcommand {
    /// Follow a channel's event stream until the server closes it
    Tail {
        /// Channel name to follow
        channel: String,

        /// Extra request header, repeatable (`--header 'x-trace: 1'`)
        #[arg(long = "header", value_name = "NAME: VALUE")]
        header: Vec<String>,

        /// Resume after this SSE event id
        #[arg(long)]
        last_event_id: Option<String>,
    },
}

impl StreamsCmd {
    pub async fn run(&self, client: &OrionClient) -> Result<i32> {
        match &self.command {
            StreamsSubcommand::Tail {
                channel,
                header,
                last_event_id,
            } => {
                let mut options =
                    RequestOptions::streaming().with_header("accept", "text/event-stream");
                for raw in header {
                    let (name, value) = raw.split_once(':').ok_or_else(|| {
                        anyhow::anyhow!("--header expects 'name: value', not '{raw}'")
                    })?;
                    options = options.with_header(name.trim(), value.trim());
                }
                if let Some(id) = last_event_id {
                    options = options.with_header("last-event-id", id);
                }
                let path = paths::data(channel);
                let mut stdout = std::io::stdout();
                client
                    .stream_chunks(&path, &options, |chunk| {
                        stdout.write_all(&chunk)?;
                        stdout.flush()?;
                        Ok(())
                    })
                    .await?;
                Ok(0)
            }
        }
    }
}

/// Parse one `--header` argument; kept here so `streams` and `send` agree on
/// the spelling.
pub(crate) fn parse_header(raw: &str) -> Result<(&str, &str)> {
    let (name, value) = raw
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("--header expects 'name: value', not '{raw}'"))?;
    let name = name.trim();
    if name.is_empty() {
        bail!("--header needs a name before ':'");
    }
    Ok((name, value.trim()))
}
