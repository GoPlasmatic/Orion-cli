//! Deploying a compiled deployment directory — what
//! `orion-cli compile --format deployment` writes — to a target.
//!
//! The directory carries no revisions, because only the target knows them.
//! This module reads the target's history and assigns each member one:
//!
//! - a member the head deploys with the same definition keeps its revision;
//! - a member some earlier revision deployed with the same definition takes
//!   that revision back (revisions are immutable, so it names the same thing);
//! - anything else takes one past the highest revision the entity ever had,
//!   or 1 for an entity the target has never seen.
//!
//! A connection reuses a stored revision with the same provider, public
//! settings and secret locators, and otherwise creates the next one.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use orion_api::v2::{
    COMPILED_DEPLOYMENT_SCHEMA_VERSION, CompiledConnection, CompiledDeployment, CompiledRollout,
    ConnectionRevisionResponse, CreateConnectionRevisionRequest, Definition, DeploymentManifest,
    EditionTier, EntityKind, EntityRef, ManifestMember, RolloutInterval,
};
use orion_client::V2Admin;
use serde::{Deserialize, Serialize};

use crate::client::OrionClient;

/// What `compile --capabilities` recorded about the target it was compiled
/// for. CLI-owned metadata: it is never sent to the server, and a directory
/// without it still deploys.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetFingerprint {
    pub schema_version: u64,
    /// The v2 admin API revision, when the capabilities file named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_version: Option<u32>,
    /// The edition the target reported, when it reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edition: Option<EditionTier>,
    /// The channel protocols the target served, when the file listed them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_drivers: Option<Vec<String>>,
    /// Provider id → the digest the target reported for it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_digests: BTreeMap<String, String>,
}

impl TargetFingerprint {
    pub const SCHEMA_VERSION: u64 = 1;
}

/// A deployment directory, read and checked against itself.
#[derive(Debug)]
pub struct DeploymentDir {
    pub deployment: CompiledDeployment,
    /// Digest → the definition file's path, for every digest a member names.
    pub definitions: BTreeMap<String, PathBuf>,
    /// Connection id → its file.
    pub connections: BTreeMap<String, CompiledConnection>,
    /// What `compile --capabilities` recorded, when it recorded anything.
    pub target: Option<TargetFingerprint>,
}

impl DeploymentDir {
    pub fn read(root: &Path) -> Result<Self> {
        let manifest_path = root.join("deployment.json");
        let text = std::fs::read_to_string(&manifest_path).with_context(|| {
            format!(
                "reading '{}' — is this a directory `orion-cli compile --format deployment` \
                 wrote?",
                manifest_path.display()
            )
        })?;
        let deployment: CompiledDeployment = serde_json::from_str(&text)
            .with_context(|| format!("'{}' is not a deployment", manifest_path.display()))?;
        if !CompiledDeployment::supports_schema_version(deployment.schema_version) {
            bail!(
                "'{}' has schema_version {}; this orion-cli reads 1 through {}",
                manifest_path.display(),
                deployment.schema_version,
                COMPILED_DEPLOYMENT_SCHEMA_VERSION
            );
        }

        let mut seen: BTreeSet<(EntityKind, String)> = BTreeSet::new();
        let mut unique = BTreeSet::new();
        let mut definitions = BTreeMap::new();
        let mut connections = BTreeMap::new();
        // (workflow id, definition digest) of every workflow member, for
        // resolving a channel's rollout entries.
        let mut workflow_members: BTreeSet<(String, String)> = BTreeSet::new();
        for member in &deployment.members {
            let what = format!("{} '{}'", member.kind, member.id);
            if !member.rollouts.is_empty() && member.kind != EntityKind::Channel {
                bail!("{what} carries `rollouts`, and only a channel has a traffic split");
            }
            // Two workflows may share an id: they are two revisions of one
            // workflow, selected by a channel's rollouts. Every other kind
            // is unique by id, as it always was.
            if member.kind != EntityKind::Workflow
                && !unique.insert((member.kind, member.id.clone()))
            {
                bail!(
                    "deployment.json lists {} '{}' twice",
                    member.kind,
                    member.id
                );
            }
            seen.insert((member.kind, member.id.clone()));
            if member.kind == EntityKind::Connection {
                let name = member.connection.as_deref().unwrap_or(&member.id);
                let path = root.join("connections").join(format!("{name}.json"));
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading connection '{}'", path.display()))?;
                let connection: CompiledConnection = serde_json::from_str(&text)
                    .with_context(|| format!("'{}' is not a connection", path.display()))?;
                if connection.connection_id != member.id {
                    bail!(
                        "'{}' is connection '{}', and deployment.json names it '{}'",
                        path.display(),
                        connection.connection_id,
                        member.id
                    );
                }
                connections.insert(member.id.clone(), connection);
            } else if matches!(
                member.kind,
                EntityKind::Workflow | EntityKind::Channel | EntityKind::Plugin
            ) {
                let Some(digest) = member.definition.as_deref() else {
                    bail!("{} '{}' names no definition", member.kind, member.id);
                };
                let path = definition_path(root, digest, &what)?;
                definitions.insert(digest.to_string(), path);
                if member.kind == EntityKind::Workflow {
                    workflow_members.insert((member.id.clone(), digest.to_string()));
                }
                if !member.rollouts.is_empty() {
                    validate_compiled_rollouts(&member.rollouts)?;
                    for rollout in &member.rollouts {
                        let at = format!("channel '{}' rollout '{}'", member.id, rollout.workflow);
                        let path = definition_path(root, &rollout.definition, &at)?;
                        definitions.insert(rollout.definition.clone(), path);
                    }
                }
            } else {
                bail!(
                    "deployment.json lists '{}' of kind '{}', which a deployment cannot carry",
                    member.id,
                    member.kind
                );
            }
        }

        // A channel serves the workflow its `requires` names. With no
        // rollouts it must be the only revision of that id in the set; with
        // rollouts, each entry must name one of the workflow members.
        for member in &deployment.members {
            if member.kind != EntityKind::Channel {
                continue;
            }
            let required_workflows: Vec<&str> = member
                .requires
                .iter()
                .filter(|requirement| requirement.kind == EntityKind::Workflow)
                .map(|requirement| requirement.id.as_str())
                .collect();
            if required_workflows.len() > 1 {
                bail!(
                    "channel '{}' requires more than one workflow ({}); a channel serves one",
                    member.id,
                    required_workflows.join(", ")
                );
            }
            let required = required_workflows.first().copied();
            if member.rollouts.is_empty() {
                if let Some(id) = required {
                    let revisions = deployment
                        .members
                        .iter()
                        .filter(|other| other.kind == EntityKind::Workflow && other.id == id)
                        .count();
                    if revisions > 1 {
                        bail!(
                            "channel '{}' serves workflow '{id}', which has {revisions} revisions \
                             in the set; name each bucket's revision with `rollouts`",
                            member.id
                        );
                    }
                }
                continue;
            }
            let Some(required) = required else {
                bail!(
                    "channel '{}' has `rollouts` and requires no workflow",
                    member.id
                );
            };
            for rollout in &member.rollouts {
                if rollout.workflow != required {
                    bail!(
                        "channel '{}' rollout names workflow '{}', and the channel requires \
                         '{required}'",
                        member.id,
                        rollout.workflow
                    );
                }
                if !workflow_members
                    .contains(&(rollout.workflow.clone(), rollout.definition.clone()))
                {
                    bail!(
                        "channel '{}' rollout names workflow '{}' definition {}, which \
                         deployment.json does not list",
                        member.id,
                        rollout.workflow,
                        rollout.definition
                    );
                }
            }
        }

        for member in &deployment.members {
            for req in &member.requires {
                if !seen.contains(&(req.kind, req.id.clone())) {
                    bail!(
                        "{} '{}' requires {} '{}', which deployment.json does not list",
                        member.kind,
                        member.id,
                        req.kind,
                        req.id
                    );
                }
            }
        }
        let target_path = root.join("target.json");
        let target = if target_path.is_file() {
            let text = std::fs::read_to_string(&target_path)
                .with_context(|| format!("reading '{}'", target_path.display()))?;
            let target: TargetFingerprint = serde_json::from_str(&text).with_context(|| {
                format!("'{}' is not a target fingerprint", target_path.display())
            })?;
            if target.schema_version != TargetFingerprint::SCHEMA_VERSION {
                bail!(
                    "'{}' has schema_version {}; this orion-cli reads {}",
                    target_path.display(),
                    target.schema_version,
                    TargetFingerprint::SCHEMA_VERSION
                );
            }
            Some(target)
        } else {
            None
        };

        Ok(Self {
            deployment,
            definitions,
            connections,
            target,
        })
    }
}

/// The path of a definition digest in a compiled directory, or why it cannot
/// be one. `Digest::hex` is exactly 64 lowercase hex characters, so the
/// digest cannot name a path outside `definitions/`.
fn definition_path(root: &Path, digest: &str, what: &str) -> Result<PathBuf> {
    let Ok(parsed) = orion_api::Digest::parse(digest) else {
        bail!("{what} names definition '{digest}', which is not a sha256: digest");
    };
    let path = root
        .join("definitions")
        .join(format!("{}.json", parsed.hex()));
    if !path.is_file() {
        bail!(
            "{what} names definition {digest}, and '{}' is not there",
            path.display()
        );
    }
    Ok(path)
}

/// Refuse a split that the server's revision-based partition check would
/// refuse after the deployer assigned revisions: gaps, overlaps, bad bounds,
/// a repeated variant, or intervals that do not cover `[0, 100)`.
fn validate_compiled_rollouts(rollouts: &[CompiledRollout]) -> Result<()> {
    if rollouts.is_empty() {
        bail!("a channel's `rollouts` list is empty");
    }
    let mut sorted: Vec<&CompiledRollout> = rollouts.iter().collect();
    sorted.sort_by_key(|rollout| (rollout.start_bucket, rollout.end_bucket));
    let mut seen = BTreeSet::new();
    let mut cursor = 0u8;
    for rollout in &sorted {
        if !seen.insert((rollout.workflow.as_str(), rollout.definition.as_str())) {
            bail!(
                "channel rollout names workflow '{}' definition {} twice",
                rollout.workflow,
                rollout.definition
            );
        }
        if rollout.start_bucket >= rollout.end_bucket || rollout.end_bucket > 100 {
            bail!(
                "channel rollout for workflow '{}' has invalid bucket bounds [{}, {})",
                rollout.workflow,
                rollout.start_bucket,
                rollout.end_bucket
            );
        }
        if rollout.start_bucket != cursor {
            bail!(
                "channel rollouts have a gap or overlap at bucket {cursor} vs {}",
                rollout.start_bucket
            );
        }
        cursor = rollout.end_bucket;
    }
    if cursor != 100 {
        bail!("channel rollouts end at {cursor}, expected 100");
    }
    Ok(())
}

/// `(kind, id, revision, definition digest)` of one deployed member.
type Deployed = (EntityKind, String, u64, String);

/// What the target has deployed: its head, and every revision before it.
#[derive(Debug, Default)]
pub struct History {
    pub head_revision: u64,
    pub head_manifest_digest: Option<String>,
    /// What the head deploys.
    head: Vec<Deployed>,
    /// The same for every revision the target lists, head included.
    all: Vec<Deployed>,
}

impl History {
    /// The target's history: every committed revision, page by page, and
    /// the manifest of each one not compacted. A compacted revision has no
    /// members left to read.
    pub async fn read(client: &OrionClient) -> Result<Self> {
        let first = client.list_deployments(None, None).await?;
        let mut history = Self {
            head_revision: first.head_revision,
            ..Self::default()
        };
        let mut page = first;
        loop {
            for summary in page.data.iter().filter(|r| !r.members_compacted) {
                let revision = client.get_deployment(summary.revision).await?;
                history.add(
                    summary.revision,
                    &summary.manifest_digest,
                    &revision.manifest,
                );
            }
            let Some(cursor) = page.next_cursor.take() else {
                break;
            };
            page = client.list_deployments(None, Some(&cursor)).await?;
        }
        Ok(history)
    }

    /// Record one revision's members.
    fn add(&mut self, revision: u64, manifest_digest: &str, manifest: &DeploymentManifest) {
        let members: Vec<Deployed> = manifest
            .members
            .iter()
            .map(|m| {
                (
                    m.entity.kind,
                    m.entity.id.clone(),
                    m.entity.revision,
                    m.definition_digest.clone(),
                )
            })
            .collect();
        if revision == self.head_revision {
            self.head = members.clone();
            self.head_manifest_digest = Some(manifest_digest.to_string());
        }
        self.all.extend(members);
    }

    /// The revision `(kind, id)` takes for `digest`: kept, taken back, or the
    /// next one above `floor` — the highest revision this deploy pass has
    /// already assigned. Two new variants of one workflow in a canary would
    /// otherwise both take `max(history) + 1` and collide.
    pub fn revision_for_above(&self, kind: EntityKind, id: &str, digest: &str, floor: u64) -> u64 {
        let same = |list: &[Deployed]| {
            list.iter()
                .filter(|(k, i, _, d)| *k == kind && i == id && d == digest)
                .map(|(_, _, r, _)| *r)
                .max()
        };
        if let Some(revision) = same(&self.head).or_else(|| same(&self.all)) {
            return revision;
        }
        self.all
            .iter()
            .filter(|(k, i, _, _)| *k == kind && i == id)
            .map(|(_, _, r, _)| *r)
            .max()
            .unwrap_or(0)
            .max(floor)
            + 1
    }

    /// The revision of connection `id` the head deploys, if it deploys one.
    fn head_connection(&self, id: &str) -> Option<u64> {
        self.head
            .iter()
            .find(|(k, i, _, _)| *k == EntityKind::Connection && i == id)
            .map(|(_, _, r, _)| *r)
    }
}

/// The stored revision of a connection that matches `wanted`, if one does:
/// the head's when it matches, else the highest.
pub fn matching_connection_revision(
    wanted: &CompiledConnection,
    stored: &[ConnectionRevisionResponse],
    head_revision: Option<u64>,
) -> Option<(u64, String)> {
    let matches: Vec<&ConnectionRevisionResponse> = stored
        .iter()
        .filter(|row| {
            row.connection_id == wanted.connection_id
                && row.provider_digest == wanted.provider_digest
                && row.public_config == wanted.public_config
                && row.secret_locators == wanted.secret_locators
                && row.bindings == wanted.bindings
        })
        .collect();
    matches
        .iter()
        .find(|row| Some(row.revision) == head_revision)
        .or_else(|| matches.iter().max_by_key(|row| row.revision))
        .map(|row| (row.revision, row.configuration_digest.clone()))
}

/// A member with the revision it was assigned, for the summary.
#[derive(Debug, Clone)]
pub struct Assigned {
    pub kind: EntityKind,
    pub id: String,
    pub revision: u64,
    /// Whether the head deploys this revision already.
    pub unchanged: bool,
}

/// What deploying a directory sends: the manifest, and against what.
#[derive(Debug)]
pub struct Prepared {
    pub base_revision: u64,
    pub head_manifest_digest: Option<String>,
    pub manifest: DeploymentManifest,
    pub assigned: Vec<Assigned>,
}

/// Upload the directory's definitions, create the connection revisions it
/// needs, and build the manifest that deploys it on top of the target's
/// head. Writes nothing that serves: definitions and connection revisions
/// are inert until a manifest names them. With `expected_head`, refuses
/// before writing anything unless the head is that revision.
pub async fn prepare(
    client: &OrionClient,
    dir: &DeploymentDir,
    expected_head: Option<u64>,
) -> Result<Prepared> {
    let history = History::read(client).await?;
    if let Some(expected) = expected_head
        && expected != history.head_revision
    {
        bail!(
            "the target's head is revision {}, not {expected}",
            history.head_revision
        );
    }

    // Read the target once: the channel protocols, the provider digests and
    // the edition are all checks on what this directory is about to write.
    let capabilities = super::v2::capabilities(client).await?;
    if let Some(target) = &dir.target {
        if let Some(api_version) = target.api_version
            && api_version != capabilities.api_version
        {
            bail!(
                "this directory was compiled against v2 admin API {api_version}, and the \
                 target speaks {}: recompile it with the target's capabilities",
                capabilities.api_version
            );
        }
        if let Some(edition) = target.edition
            && capabilities.edition < edition
        {
            eprintln!(
                "warning: this directory was compiled against edition '{edition}', and the \
                 target reports '{}'; its members are checked against the target's capabilities",
                capabilities.edition
            );
        }
    }

    for (digest, path) in &dir.definitions {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading '{}'", path.display()))?;
        let definition: Definition = serde_json::from_str(&text)
            .with_context(|| format!("'{}' is not a definition", path.display()))?;
        if let Definition::Channel(channel) = &definition
            && !capabilities.channel_drivers.is_empty()
            && !capabilities
                .channel_drivers
                .iter()
                .any(|served| served.eq_ignore_ascii_case(&channel.protocol))
        {
            bail!(
                "channel '{}' uses protocol '{}', which the target does not serve (it serves: \
                 {}): compile with the target's capabilities",
                channel.name,
                channel.protocol,
                capabilities.channel_drivers.join(", ")
            );
        }
        let stored = client
            .upload_definition(&definition)
            .await
            .with_context(|| format!("uploading '{}'", path.display()))?;
        if &stored.digest != digest {
            bail!(
                "the target stored '{}' as {}, not {digest}: it canonicalises definitions \
                 differently from the orion-cli that compiled this directory — compile with \
                 the target's version",
                path.display(),
                stored.digest
            );
        }
        for advisory in &stored.advisories {
            eprintln!(
                "warning: '{}' {}: {} [{}]",
                path.display(),
                advisory.path,
                advisory.message,
                advisory.check
            );
        }
    }

    let mut connection_revisions: BTreeMap<String, (u64, String)> = BTreeMap::new();
    if !dir.connections.is_empty() {
        let builtin: BTreeMap<String, String> = capabilities
            .provider_backends
            .iter()
            .filter_map(|backend| {
                Some((backend.driver_id.clone(), backend.package_digest.clone()?))
            })
            .collect();
        let stored = client.list_connections().await?;
        for (id, connection) in &dir.connections {
            match builtin.get(&connection.driver) {
                Some(digest) if digest == &connection.provider_digest => {}
                Some(digest) => bail!(
                    "connection '{id}' names {} as driver {}'s digest, and the target's is \
                     {digest}",
                    connection.provider_digest,
                    connection.driver
                ),
                None => bail!(
                    "connection '{id}' runs on driver '{}', which the target does not have",
                    connection.driver
                ),
            }
            let found =
                matching_connection_revision(connection, &stored, history.head_connection(id));
            let assigned = match found {
                Some(existing) => existing,
                None => {
                    let revision = stored
                        .iter()
                        .filter(|row| &row.connection_id == id)
                        .map(|row| row.revision)
                        .max()
                        .unwrap_or(0)
                        + 1;
                    let created = client
                        .create_connection_revision(
                            id,
                            &CreateConnectionRevisionRequest {
                                revision,
                                provider_digest: connection.provider_digest.clone(),
                                public_config: connection.public_config.clone(),
                                secret_locators: connection.secret_locators.clone(),
                                bindings: connection.bindings.clone(),
                            },
                        )
                        .await
                        .with_context(|| {
                            format!("creating connection '{id}' revision {revision}")
                        })?;
                    (revision, created.connection.configuration_digest)
                }
            };
            connection_revisions.insert(id.clone(), assigned);
        }
    }

    // Every member's revision, then every requirement resolved against them.
    // Workflows are keyed by their definition too: two revisions of one id
    // are different members.
    let mut revisions: BTreeMap<(EntityKind, String, String), (u64, String)> = BTreeMap::new();
    // The highest revision this pass has assigned per (kind, id), so two new
    // variants of one workflow do not both take `history_max + 1`.
    let mut assigned_floor: BTreeMap<(EntityKind, String), u64> = BTreeMap::new();
    for member in &dir.deployment.members {
        let digest = member.definition.clone().unwrap_or_default();
        let assigned = if member.kind == EntityKind::Connection {
            connection_revisions
                .get(&member.id)
                .cloned()
                .with_context(|| format!("connection '{}' was not resolved", member.id))?
        } else {
            let key = (member.kind, member.id.clone());
            let floor = assigned_floor.get(&key).copied().unwrap_or(0);
            let revision = history.revision_for_above(member.kind, &member.id, &digest, floor);
            assigned_floor.insert(key, revision);
            (revision, digest.clone())
        };
        revisions.insert((member.kind, member.id.clone(), digest), assigned);
    }

    let revision_of = |kind: EntityKind, id: &str, digest: &str| -> Result<(u64, String)> {
        revisions
            .get(&(kind, id.to_string(), digest.to_string()))
            .cloned()
            .with_context(|| format!("{kind} '{id}' definition {digest} was assigned no revision"))
    };
    // A requirement names a kind and id, not a digest. Every kind but a
    // workflow is unique by id; a workflow's variants are disambiguated by
    // the channel's rollouts above, and a non-split channel was refused in
    // `read` when its workflow had more than one.
    let unique_revision = |kind: EntityKind, id: &str| -> Result<u64> {
        let mut found = revisions
            .iter()
            .filter(|((known_kind, known_id, _), _)| *known_kind == kind && known_id == id)
            .map(|(_, (revision, _))| *revision);
        let first = found
            .next()
            .with_context(|| format!("{kind} '{id}' was assigned no revision"))?;
        if found.next().is_some() {
            bail!(
                "{kind} '{id}' has more than one revision in the set; name each bucket's \
                 revision with a channel `rollouts` list"
            );
        }
        Ok(first)
    };

    let mut members = Vec::new();
    let mut assigned = Vec::new();
    for member in &dir.deployment.members {
        let digest = member.definition.clone().unwrap_or_default();
        let (revision, digest) = revision_of(member.kind, &member.id, &digest)?;
        let split = member.kind == EntityKind::Channel && !member.rollouts.is_empty();
        let mut requires = Vec::new();
        for req in &member.requires {
            if split && req.kind == EntityKind::Workflow {
                // The channel's split names every workflow revision it
                // serves; the compiled `requires` names the id once.
                for rollout in &member.rollouts {
                    let (workflow_revision, _) =
                        revision_of(EntityKind::Workflow, &rollout.workflow, &rollout.definition)?;
                    requires.push(EntityRef {
                        kind: EntityKind::Workflow,
                        id: req.id.clone(),
                        revision: workflow_revision,
                    });
                }
            } else {
                requires.push(EntityRef {
                    kind: req.kind,
                    id: req.id.clone(),
                    revision: unique_revision(req.kind, &req.id)?,
                });
            }
        }
        requires.sort();
        requires.dedup();
        let rollouts = member
            .rollouts
            .iter()
            .map(|rollout| {
                let (workflow_revision, _) =
                    revision_of(EntityKind::Workflow, &rollout.workflow, &rollout.definition)?;
                Ok(RolloutInterval {
                    workflow_revision,
                    start_bucket: rollout.start_bucket,
                    end_bucket: rollout.end_bucket,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        members.push(ManifestMember {
            entity: EntityRef {
                kind: member.kind,
                id: member.id.clone(),
                revision,
            },
            definition_digest: digest,
            owning_package: None,
            requires,
            rollouts,
        });
        assigned.push(Assigned {
            kind: member.kind,
            id: member.id.clone(),
            revision,
            unchanged: history
                .head
                .iter()
                .any(|(k, i, r, _)| *k == member.kind && i == &member.id && *r == revision),
        });
    }
    Ok(Prepared {
        base_revision: history.head_revision,
        head_manifest_digest: history.head_manifest_digest,
        manifest: DeploymentManifest::of_members(members),
        assigned,
    })
}

/// The default idempotency key for applying `candidate_digest` on top of
/// `base_revision`: a retry of the same apply replays it, and the same
/// manifest applied again later, over another head, is a new operation.
pub fn default_idempotency_key(base_revision: u64, candidate_digest: &str) -> String {
    format!(
        "deploy-{base_revision}-{}",
        candidate_digest.trim_start_matches(orion_api::Digest::PREFIX)
    )
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn member(kind: EntityKind, id: &str, rev: u64, digest: &str) -> ManifestMember {
        ManifestMember {
            entity: EntityRef {
                kind,
                id: id.to_string(),
                revision: rev,
            },
            definition_digest: digest.to_string(),
            owning_package: None,
            requires: Vec::new(),
            rollouts: Vec::new(),
        }
    }

    #[test]
    fn a_compiled_directory_records_the_target_it_was_built_for() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("deployment.json"),
            r#"{"schema_version":1,"members":[]}"#,
        )
        .unwrap();
        let directory = DeploymentDir::read(dir.path()).unwrap();
        assert!(directory.target.is_none());

        std::fs::write(
            dir.path().join("target.json"),
            r#"{"schema_version":1,"api_version":1,"edition":"enterprise",
                "channel_drivers":["rest","kafka"],
                "provider_digests":{"com.orion.models":"sha256:aaaa"}}"#,
        )
        .unwrap();
        let directory = DeploymentDir::read(dir.path()).unwrap();
        let target = directory.target.expect("a target record");
        assert_eq!(target.api_version, Some(1));
        assert_eq!(target.edition, Some(EditionTier::Enterprise));
        assert_eq!(
            target.channel_drivers.as_deref(),
            Some(&["rest".to_string(), "kafka".to_string()][..])
        );
        assert_eq!(
            target
                .provider_digests
                .get("com.orion.models")
                .map(String::as_str),
            Some("sha256:aaaa")
        );

        std::fs::write(dir.path().join("target.json"), r#"{"schema_version":99}"#).unwrap();
        let error = DeploymentDir::read(dir.path()).unwrap_err().to_string();
        assert!(error.contains("schema_version 99"), "{error}");
    }

    #[test]
    fn a_member_keeps_takes_back_or_advances_its_revision() {
        use EntityKind::{Channel, Workflow};
        let mut history = History {
            head_revision: 3,
            ..History::default()
        };
        let manifest = |m: ManifestMember| DeploymentManifest::of_members(vec![m]);
        history.add(
            1,
            "sha256:1",
            &manifest(member(Workflow, "w", 1, "sha256:a")),
        );
        history.add(
            2,
            "sha256:2",
            &manifest(member(Workflow, "w", 2, "sha256:b")),
        );
        // A rollback to revision 1's manifest.
        history.add(
            3,
            "sha256:3",
            &manifest(member(Workflow, "w", 1, "sha256:a")),
        );
        assert_eq!(history.revision_for_above(Workflow, "w", "sha256:a", 0), 1);
        assert_eq!(history.revision_for_above(Workflow, "w", "sha256:b", 0), 2);
        // New content goes past every revision the entity ever had, not the
        // head's: revision 2 already names something else.
        assert_eq!(history.revision_for_above(Workflow, "w", "sha256:c", 0), 3);
        assert_eq!(history.revision_for_above(Channel, "c", "sha256:z", 0), 1);
        assert_eq!(history.head_manifest_digest.as_deref(), Some("sha256:3"));
    }

    #[test]
    fn a_connection_reuses_a_revision_with_the_same_settings() {
        let wanted = CompiledConnection {
            connection_id: "db".into(),
            driver: "org.orion.sqlite".into(),
            provider_digest: "sha256:p".into(),
            public_config: json!({"connection_string": "sqlite::memory:"}),
            secret_locators: BTreeMap::new(),
            bindings: BTreeMap::new(),
        };
        let row = |rev: u64, config: Value| ConnectionRevisionResponse {
            connection_id: "db".into(),
            revision: rev,
            provider_digest: "sha256:p".into(),
            public_config: config,
            configuration_digest: format!("sha256:c{rev}"),
            ..Default::default()
        };
        let stored = vec![
            row(1, json!({"connection_string": "sqlite::memory:"})),
            row(2, json!({"connection_string": "sqlite:/tmp/x.db"})),
            row(3, json!({"connection_string": "sqlite::memory:"})),
        ];
        assert_eq!(
            matching_connection_revision(&wanted, &stored, Some(1)),
            Some((1, "sha256:c1".to_string()))
        );
        assert_eq!(
            matching_connection_revision(&wanted, &stored, Some(2)),
            Some((3, "sha256:c3".to_string()))
        );
        let other = CompiledConnection {
            public_config: json!({"connection_string": "sqlite:/tmp/y.db"}),
            ..wanted.clone()
        };
        assert_eq!(matching_connection_revision(&other, &stored, None), None);

        // A binding is part of the revision's identity: the same settings
        // over a different dependency must not reuse the old revision.
        let mut bound = wanted;
        bound
            .bindings
            .insert("artifact_store".into(), "store-b".into());
        assert_eq!(matching_connection_revision(&bound, &stored, None), None);
        let bound_row = |rev: u64, target: &str| ConnectionRevisionResponse {
            connection_id: "db".into(),
            revision: rev,
            provider_digest: "sha256:p".into(),
            public_config: json!({"connection_string": "sqlite::memory:"}),
            bindings: [("artifact_store".to_string(), target.to_string())]
                .into_iter()
                .collect(),
            configuration_digest: format!("sha256:bound{rev}"),
            ..Default::default()
        };
        let stored = vec![bound_row(1, "store-a"), bound_row(2, "store-b")];
        assert_eq!(
            matching_connection_revision(&bound, &stored, Some(2)),
            Some((2, "sha256:bound2".to_string()))
        );
    }
}
