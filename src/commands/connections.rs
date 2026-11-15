//! `orion-cli connections`: `list`, `get`, `create`, `probe`. Every call goes
//! through `orion_client::V2Admin`.

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use colored::Colorize;
use orion_api::v2::{ConnectionRevisionResponse, CreateConnectionRevisionRequest};
use orion_client::V2Admin;
use tabled::Tabled;

use super::v2::load_request;
use crate::client::OrionClient;
use crate::output::{self, OutputFormat};
use crate::utils;

#[derive(Args)]
#[command(
    long_about = "Manage immutable v2 connection revisions and execute audited health probes."
)]
pub struct ConnectionsCmd {
    #[command(subcommand)]
    command: ConnectionsSubcommand,
}

#[derive(Subcommand)]
pub enum ConnectionsSubcommand {
    /// List immutable connection revisions: every connection's, or one's
    List {
        /// Only this connection's revisions
        #[arg(long)]
        id: Option<String>,
        /// Page size (default: 50, max: 1000)
        #[arg(long)]
        limit: Option<i64>,
        /// Resume after this position: the previous page's next_cursor
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Show one connection revision (the latest when no --revision)
    Get {
        /// Connection identifier
        id: String,
        /// Revision to show (defaults to the latest)
        #[arg(long)]
        revision: Option<u64>,
    },
    /// Create a new immutable connection revision
    Create {
        /// Connection identifier
        id: String,
        /// Path to the revision JSON (`revision`, `provider_digest`,
        /// `public_config`, `secret_locators`, `bindings`), or '-' for stdin
        #[arg(short, long)]
        file: String,
        /// Bind a dependency alias to a connection of the deployment:
        /// `alias=connection`. Repeatable; a conflicting value in the file is
        /// refused
        #[arg(long = "bind", value_name = "ALIAS=CONNECTION")]
        bind: Vec<String>,
        /// Place a credential's locator at a config path: `path=env://NAME`.
        /// The value is a reference, never the secret itself. Repeatable
        #[arg(long = "secret", value_name = "PATH=LOCATOR")]
        secret: Vec<String>,
    },
    /// Run an audited health probe against a connection revision; exits 2
    /// when the backend is unhealthy
    Probe {
        /// Connection identifier
        id: String,
        /// Revision to probe (defaults to the latest)
        #[arg(long)]
        revision: Option<u64>,
    },
}

#[derive(Tabled)]
struct ConnectionRow {
    #[tabled(rename = "Connection")]
    connection: String,
    #[tabled(rename = "Revision")]
    revision: u64,
    #[tabled(rename = "Provider")]
    provider: String,
    #[tabled(rename = "Bindings")]
    bindings: String,
    #[tabled(rename = "Configuration")]
    configuration: String,
}

fn connection_row(connection: &ConnectionRevisionResponse) -> ConnectionRow {
    ConnectionRow {
        connection: connection.connection_id.clone(),
        revision: connection.revision,
        provider: utils::truncate(&connection.provider_digest, 19),
        bindings: bindings_text(connection),
        configuration: utils::truncate(&connection.configuration_digest, 19),
    }
}

fn bindings_text(connection: &ConnectionRevisionResponse) -> String {
    if connection.bindings.is_empty() {
        "-".to_string()
    } else {
        connection
            .bindings
            .iter()
            .map(|(alias, target)| format!("{alias}={target}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A revision at a glance: what it runs on, what it binds, where its
/// credentials come from, and its public settings. Locators are references,
/// never the values they resolve to.
fn print_connection(connection: &ConnectionRevisionResponse) {
    println!(
        "{} {}",
        connection.connection_id.bold(),
        format!("@{}", connection.revision).dimmed()
    );
    println!("  Provider:      {}", connection.provider_digest);
    println!("  Configuration: {}", connection.configuration_digest);
    println!(
        "  Bindings:      {}",
        if connection.bindings.is_empty() {
            "none".dimmed().to_string()
        } else {
            bindings_text(connection)
        }
    );
    println!(
        "  Secrets:       {}",
        if connection.secret_locators.is_empty() {
            "none".dimmed().to_string()
        } else {
            connection
                .secret_locators
                .iter()
                .map(|(path, locator)| format!("{path} -> {locator}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    if let Ok(public) = serde_json::to_string_pretty(&connection.public_config) {
        println!("  Public config:");
        for line in public.lines() {
            println!("    {line}");
        }
    }
}

/// `alias=value` pairs from a repeatable flag.
fn parse_pairs(values: &[String], flag: &str, what: &str) -> Result<Vec<(String, String)>> {
    values
        .iter()
        .map(|value| {
            let (left, right) = value
                .split_once('=')
                .with_context(|| format!("--{flag} expects {what}, and '{value}' has no '='"))?;
            if left.is_empty() || right.is_empty() {
                bail!("--{flag} expects {what}, and '{value}' names an empty side");
            }
            Ok((left.to_string(), right.to_string()))
        })
        .collect()
}

/// `revision`, or the highest one the connection has.
async fn resolve_revision(
    client: &OrionClient,
    connection_id: &str,
    revision: Option<u64>,
) -> Result<u64> {
    if let Some(revision) = revision {
        return Ok(revision);
    }
    client
        .list_connection_revisions(connection_id)
        .await?
        .iter()
        .map(|c| c.revision)
        .max()
        .with_context(|| format!("connection '{connection_id}' has no revisions"))
}

impl ConnectionsCmd {
    #[cfg(test)]
    pub(super) fn new(command: ConnectionsSubcommand) -> Self {
        Self { command }
    }

    pub async fn run(
        &self,
        client: &OrionClient,
        format: &OutputFormat,
        quiet: bool,
    ) -> Result<i32> {
        match &self.command {
            ConnectionsSubcommand::List { id, limit, cursor } => {
                let params = orion_client::ListParams {
                    limit: *limit,
                    cursor: cursor.clone(),
                    include_total: false,
                };
                let resp = match id {
                    Some(id) => client.list_connection_revisions_page(id, &params).await?,
                    None => client.list_connections_page(&params).await?,
                };
                if quiet {
                    for item in &resp.data {
                        println!("{}@{}", item.connection_id, item.revision);
                    }
                    return Ok(0);
                }
                if matches!(format, OutputFormat::Json | OutputFormat::Yaml) {
                    output::print_value(format, &serde_json::to_value(&resp)?)?;
                    return Ok(0);
                }
                if resp.data.is_empty() {
                    println!("{}", "No connection revisions found.".dimmed());
                    return Ok(0);
                }
                output::print_table(resp.data.iter().map(connection_row).collect());
                if let Some(next) = &resp.next_cursor {
                    println!("{}", format!("next page: --cursor {next}").dimmed());
                }
                Ok(0)
            }
            ConnectionsSubcommand::Get { id, revision } => {
                let revision = resolve_revision(client, id, *revision).await?;
                let resp = client.get_connection_revision(id, revision).await?;
                if quiet {
                    println!("{}", resp.configuration_digest);
                    return Ok(0);
                }
                if matches!(format, OutputFormat::Json | OutputFormat::Yaml) {
                    output::print_value(format, &serde_json::to_value(&resp)?)?;
                    return Ok(0);
                }
                print_connection(&resp);
                Ok(0)
            }
            ConnectionsSubcommand::Create {
                id,
                file,
                bind,
                secret,
            } => {
                let mut req: CreateConnectionRevisionRequest =
                    load_request(file, "connection revision")?;
                for (alias, target) in parse_pairs(bind, "bind", "alias=connection")? {
                    if let Some(existing) = req.bindings.insert(alias.clone(), target.clone())
                        && existing != target
                    {
                        bail!(
                            "--bind {alias}={target} conflicts with the file's '{alias}' -> \
                             '{existing}'"
                        );
                    }
                }
                for (path, locator) in parse_pairs(secret, "secret", "path=locator")? {
                    if let Some(existing) =
                        req.secret_locators.insert(path.clone(), locator.clone())
                        && existing != locator
                    {
                        bail!(
                            "--secret {path}={locator} conflicts with the file's '{path}' -> \
                             '{existing}'"
                        );
                    }
                }
                let resp = client.create_connection_revision(id, &req).await?;
                if quiet {
                    println!("{}", resp.connection.revision);
                    return Ok(0);
                }
                output::print_value(format, &serde_json::to_value(&resp)?)?;
                Ok(0)
            }
            ConnectionsSubcommand::Probe { id, revision } => {
                let revision = resolve_revision(client, id, *revision).await?;
                let resp = client.probe_connection(id, revision).await?;
                if quiet {
                    println!("{}", if resp.healthy { "healthy" } else { "unhealthy" });
                } else if matches!(format, OutputFormat::Json | OutputFormat::Yaml) {
                    output::print_value(format, &serde_json::to_value(&resp)?)?;
                } else {
                    println!(
                        "{} {}@{}",
                        if resp.healthy {
                            "HEALTHY".green().bold()
                        } else {
                            "UNHEALTHY".red().bold()
                        },
                        resp.connection_id,
                        resp.revision
                    );
                    println!("  Driver:   {}", resp.driver_id);
                    println!("  Latency:  {} ms", resp.latency_ms);
                    println!("  Probe:    {}", resp.details.probe);
                    if let Some(class) = &resp.details.error_class {
                        println!("  Error:    {class}");
                    }
                    if let Some(error) = &resp.details.error {
                        println!("  Detail:   {error}");
                    }
                    if let Some(effect) = &resp.details.effect_status {
                        println!("  Effect:   {effect}");
                    }
                }
                Ok(if resp.healthy { 0 } else { 2 })
            }
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn bindings_and_secret_flags_parse_pairs() {
        let pairs = parse_pairs(
            &["artifact_store=store".into(), "dead_letter=dlq".into()],
            "bind",
            "alias=connection",
        )
        .unwrap();
        assert_eq!(
            pairs,
            vec![
                ("artifact_store".to_string(), "store".to_string()),
                ("dead_letter".to_string(), "dlq".to_string()),
            ]
        );

        let error = parse_pairs(&["no-separator".into()], "bind", "alias=connection")
            .unwrap_err()
            .to_string();
        assert!(error.contains("no '='"), "{error}");
        assert!(
            parse_pairs(&["=store".into()], "bind", "alias=connection").is_err(),
            "an empty alias is refused"
        );
        assert!(
            parse_pairs(&["store=".into()], "bind", "alias=connection").is_err(),
            "an empty target is refused"
        );
    }
}
