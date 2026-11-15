//! `orion-cli circuit-breakers`: read the node-local breaker map and reset
//! one key. A reset fans out to every node in cluster mode; the read says
//! whose map it is.

use anyhow::Result;
use clap::{Args, Subcommand};
use colored::Colorize;
use orion_api::{CircuitBreakerResetResponse, CircuitBreakerStatesResponse, Data};
use tabled::Tabled;

use crate::client::OrionClient;
use crate::output::{self, OutputFormat};
use crate::utils;
use orion_client::paths;

#[derive(Args)]
#[command(
    long_about = "Inspect connector circuit breakers (node-local) and reset one. \
                  A reset fans out to every node in cluster mode."
)]
pub struct CircuitBreakersCmd {
    #[command(subcommand)]
    command: CircuitBreakersSubcommand,
}

#[derive(Subcommand)]
enum CircuitBreakersSubcommand {
    /// List this node's breaker states
    List,
    /// Reset one breaker (`channel:connector`), on every node in cluster mode
    Reset {
        /// Breaker key, as `list` prints it
        key: String,
    },
}

#[derive(Tabled)]
struct BreakerRow {
    #[tabled(rename = "Key")]
    key: String,
    #[tabled(rename = "State")]
    state: String,
}

impl CircuitBreakersCmd {
    pub async fn run(
        &self,
        client: &OrionClient,
        format: &OutputFormat,
        quiet: bool,
        yes: bool,
    ) -> Result<i32> {
        match &self.command {
            CircuitBreakersSubcommand::List => {
                let Data { data } = client
                    .get::<Data<CircuitBreakerStatesResponse>>(paths::CIRCUIT_BREAKERS)
                    .await?;
                if quiet {
                    for key in data.breakers.keys() {
                        println!("{key}");
                    }
                    return Ok(0);
                }
                if matches!(format, OutputFormat::Json | OutputFormat::Yaml) {
                    output::print_value(format, &serde_json::to_value(&data)?)?;
                    return Ok(0);
                }
                println!(
                    "{} {} ({} · {})",
                    "Circuit breakers".bold(),
                    if data.enabled {
                        "enabled".green().to_string()
                    } else {
                        "disabled".dimmed().to_string()
                    },
                    format!("scope {}", data.scope).dimmed(),
                    format!("node {}", data.instance_id).dimmed()
                );
                if data.breakers.is_empty() {
                    println!("  {}", "No breaker states.".dimmed());
                    return Ok(0);
                }
                let rows: Vec<BreakerRow> = data
                    .breakers
                    .iter()
                    .map(|(key, state)| BreakerRow {
                        key: key.clone(),
                        state: state.clone(),
                    })
                    .collect();
                output::print_table(rows);
                Ok(0)
            }
            CircuitBreakersSubcommand::Reset { key } => {
                if !utils::confirm(&format!("Reset circuit breaker '{key}'?"), yes)? {
                    println!("Cancelled.");
                    return Ok(0);
                }
                let Data { data } = client
                    .post_empty::<Data<CircuitBreakerResetResponse>>(&paths::circuit_breaker(key))
                    .await?;
                if quiet {
                    println!("{}", data.key);
                    return Ok(0);
                }
                output::print_value(format, &serde_json::to_value(&data)?)?;
                Ok(0)
            }
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn client(handler: impl Fn(&str, &str) -> Value + Send + Sync + 'static) -> OrionClient {
        OrionClient::with_mock_transport(move |method, path, _| Ok(handler(method, path))).unwrap()
    }

    #[tokio::test]
    async fn list_reads_the_nodes_breaker_map() {
        let client = client(|method, path| {
            assert_eq!(method, "GET");
            assert_eq!(path, paths::CIRCUIT_BREAKERS);
            json!({"data": {"enabled": true, "scope": "node", "instance_id": "node-1",
                            "breakers": {"orders:db": "open"}}})
        });
        let command = CircuitBreakersCmd {
            command: CircuitBreakersSubcommand::List,
        };
        assert_eq!(
            command
                .run(&client, &OutputFormat::Json, true, true)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn reset_posts_the_key_and_reports_the_answer() {
        let client = client(|method, path| {
            assert_eq!(method, "POST");
            assert_eq!(
                path,
                "/api/v2/admin/connectors/circuit-breakers/orders%3Adb"
            );
            json!({"data": {"reset": true, "key": "orders:db", "found_on_this_node": false}})
        });
        let command = CircuitBreakersCmd {
            command: CircuitBreakersSubcommand::Reset {
                key: "orders:db".into(),
            },
        };
        assert_eq!(
            command
                .run(&client, &OutputFormat::Json, true, true)
                .await
                .unwrap(),
            0
        );
    }
}
