//! Adapter plug-ins in the station (F-ACQ-12; CR-026): signed manifests in
//! `<data dir>/plugins/<id>/`, the executable checked against the manifest's hash before it is
//! ever started, frames from the device runtime decoded by the plug-in, and only the parameters the
//! manifest declares accepted, each through normal ingest and alarm evaluation.

use std::collections::HashMap;
use std::path::Path;

use caelum_alarms::signing::{built_in_keys, digest, verify_config, TrustPolicy};
use caelum_devices::plugin::{AdapterPlugin, SDK_VERSION};
use caelum_domain::ids::DeviceId;
use caelum_domain::parameters::Parameter;
use caelum_security::audit::{Actor, AuditAction};
use serde::Deserialize;
use serde_json::json;

use super::runtime::Ingested;
use super::{Station, StationError};
use crate::plugin_host::ProcessPlugin;

/// Configuration kind of an adapter plug-in manifest.
pub const PLUGIN_KIND: &str = "adapter-plugin";
/// Device adapter prefix of plug-in devices (`plugin:<id>`).
pub const PLUGIN_ADAPTER_PREFIX: &str = "plugin:";

/// The signed manifest of an adapter plug-in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    /// Manifest version.
    pub version: String,
    /// Plug-in ID (`[a-z0-9-]`).
    pub id: String,
    /// SDK version it was written for (must equal this build's).
    pub sdk_version: u32,
    /// Executable file name inside the plug-in folder.
    pub executable: String,
    /// SHA-256 of the executable (hex).
    pub executable_sha256: String,
    /// The vendor's notify characteristic carrying its frames (128-bit UUID).
    pub data_uuid: String,
    /// The only catalog parameters the plug-in may report.
    pub parameters: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    plugin: PluginManifest,
}

impl PluginManifest {
    /// Parses and checks a manifest.
    pub fn parse(text: &str) -> Result<Self, String> {
        let f: ManifestFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let m = f.plugin;
        let ok_id = !m.id.is_empty()
            && m.id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !ok_id {
            return Err("id must be lower-case letters, digits and '-'".into());
        }
        if m.sdk_version != SDK_VERSION {
            return Err(format!(
                "SDK version {} is not this build's {SDK_VERSION}",
                m.sdk_version
            ));
        }
        if m.executable.contains(['/', '\\']) || m.executable.contains("..") {
            return Err("executable must be a file name in the plug-in folder".into());
        }
        if m.executable_sha256.len() != 64
            || !m.executable_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("executable_sha256 must be 64 hexadecimal digits".into());
        }
        uuid::Uuid::parse_str(&m.data_uuid).map_err(|_| "data_uuid is not a UUID".to_owned())?;
        if m.parameters.is_empty() {
            return Err("at least one parameter".into());
        }
        for p in &m.parameters {
            Parameter::from_key(p).ok_or_else(|| format!("{p} is not a catalog parameter"))?;
        }
        Ok(m)
    }
}

/// An installed plug-in.
pub(super) struct Installed {
    pub(super) manifest: PluginManifest,
    pub(super) decoder: Box<dyn AdapterPlugin>,
}

/// Loads one plug-in folder: manifest signature (kind `adapter-plugin`), executable hash.
pub fn load_plugin(dir: &Path) -> Result<(PluginManifest, ProcessPlugin), String> {
    let text = std::fs::read(dir.join("manifest.toml")).map_err(|e| format!("manifest: {e}"))?;
    let sig = std::fs::read_to_string(dir.join("manifest.sig.toml"))
        .map_err(|e| format!("signature: {e}"))?;
    let verified = verify_config(
        PLUGIN_KIND,
        &text,
        &sig,
        &built_in_keys(),
        TrustPolicy::for_this_build(),
    )
    .map_err(|e| format!("signature not accepted: {e}"))?;
    let m =
        PluginManifest::parse(std::str::from_utf8(&text).map_err(|_| "manifest is not UTF-8")?)?;
    if m.version != verified.version {
        return Err("manifest version is not the signed version".into());
    }
    let exe = dir.join(&m.executable);
    let bytes = std::fs::read(&exe).map_err(|e| format!("executable: {e}"))?;
    if !digest(&bytes).eq_ignore_ascii_case(&m.executable_sha256) {
        return Err("the executable does not match the signed hash".into());
    }
    Ok((m.clone(), ProcessPlugin::new(&m.id, exe)))
}

impl Station {
    /// Loads every plug-in in `<data dir>/plugins/` (at start). Refusals are audited.
    pub(super) fn load_plugins(&mut self) -> Result<(), StationError> {
        let Ok(entries) = std::fs::read_dir(self.data_dir.join("plugins")) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match load_plugin(&path) {
                Ok((manifest, plugin)) => {
                    let id = manifest.id.clone();
                    self.log().append(
                        &Actor::system(),
                        AuditAction::ConfigApplied,
                        Some(&format!("plugin:{id}")),
                        &json!({ "kind": PLUGIN_KIND, "version": manifest.version, "executable_sha256": manifest.executable_sha256 }),
                    )?;
                    self.plugins.insert(
                        id,
                        Installed {
                            manifest,
                            decoder: Box::new(plugin),
                        },
                    );
                }
                Err(reason) => {
                    self.log().append(
                        &Actor::system(),
                        AuditAction::PluginRefused,
                        Some(&format!("plugin:{name}")),
                        &json!({ "kind": PLUGIN_KIND, "refused": reason }),
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Installs a plug-in in process (test equipment and built-in decoders; the manifest is
    /// still parsed and checked).
    pub fn install_plugin(&mut self, manifest: PluginManifest, decoder: Box<dyn AdapterPlugin>) {
        self.plugins
            .insert(manifest.id.clone(), Installed { manifest, decoder });
    }

    /// The data characteristics of installed plug-ins, for the device runtime (notifications only).
    pub fn plugin_data_uuids(&self) -> HashMap<String, uuid::Uuid> {
        self.plugins
            .values()
            .filter_map(|p| {
                Some((
                    p.manifest.id.clone(),
                    uuid::Uuid::parse_str(&p.manifest.data_uuid).ok()?,
                ))
            })
            .collect()
    }

    /// One frame from a plug-in device (adapter `plugin:<id>`): decoded by its plug-in, readings
    /// outside the manifest dropped and counted, the rest through ingest and evaluation.
    pub fn ingest_plugin_frame(
        &mut self,
        device: DeviceId,
        frame: &[u8],
    ) -> Result<Ingested, StationError> {
        let record = self
            .db
            .device(&device.to_string())?
            .ok_or_else(|| StationError::Rejected("unknown device".into()))?;
        let id = record
            .adapter
            .strip_prefix(PLUGIN_ADAPTER_PREFIX)
            .ok_or_else(|| StationError::Rejected("not a plug-in device".into()))?
            .to_owned();
        let now = self.now()?;
        let Some(installed) = self.plugins.get_mut(&id) else {
            return Err(StationError::Rejected(format!(
                "plug-in {id} is not installed"
            )));
        };
        let decoded = match installed.decoder.decode(frame) {
            Ok(d) => d,
            Err(_) => {
                self.acquisition.count_dropped(device, "plugin_error");
                return Ok(Ingested::default());
            }
        };
        let allowed = &installed.manifest.parameters;
        let mut kept = Vec::new();
        for d in decoded {
            if allowed.iter().any(|p| p == d.parameter.spec().key) {
                kept.push(d);
            } else {
                self.acquisition
                    .count_dropped(device, "plugin_undeclared_parameter");
            }
        }
        let outcome = self
            .acquisition
            .plugin_readings(&self.db, device, kept, now)
            .map_err(|e| StationError::Rejected(e.to_string()))?;
        let mut ingested = Ingested {
            stored: outcome.observations.len(),
            ..Ingested::default()
        };
        for obs in &outcome.observations {
            ingested.crew = Some(obs.crew_member_id.to_string());
            let out = self.observe(obs)?;
            ingested.output.events.extend(out.events);
            ingested.output.notices.extend(out.notices);
        }
        Ok(ingested)
    }
}
