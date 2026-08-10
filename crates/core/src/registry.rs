//! Durable store of instance metadata: one JSON file, no database.
//!
//! Groups are **derived from tags, never stored**. That is what gives "one
//! instance, N groups" for free — there is no group entity to keep in sync,
//! and fanning a command out to a group just means resolving the tag to the
//! ids it currently maps to, at the moment the command is sent. Retagging an
//! instance mid-show therefore takes effect on the very next cue without any
//! reconciliation step.
//!
//! Every in-memory `Instance` holds its token in plain text; only the on-disk
//! JSON is encrypted, at the `save`/`load_or_new` boundary. See `crypto.rs`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::RwLock;

use crate::crypto::CredentialCipher;
use crate::instance::{Instance, InstanceId};

pub struct Registry {
    path: PathBuf,
    instances: RwLock<HashMap<InstanceId, Instance>>,
    cipher: CredentialCipher,
}

impl Registry {
    /// Loads instances from `path` if it exists, otherwise starts empty. The
    /// encryption key lives in `credentials.key` next to `path`, generated on
    /// first run.
    pub fn load_or_new(path: PathBuf) -> anyhow::Result<Self> {
        let key_path = path
            .parent()
            .map(|p| p.join("credentials.key"))
            .unwrap_or_else(|| PathBuf::from("credentials.key"));
        let cipher = CredentialCipher::load_or_create(&key_path)?;

        let instances = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            if raw.trim().is_empty() {
                HashMap::new()
            } else {
                let list: Vec<Instance> = serde_json::from_str(&raw)?;
                list.into_iter()
                    .map(|mut i| {
                        if let Some(stored) = i.credentials.token.take() {
                            i.credentials.token = Some(cipher.decrypt_or_pass_through(&stored)?);
                        }
                        Ok::<_, anyhow::Error>((i.id, i))
                    })
                    .collect::<anyhow::Result<HashMap<_, _>>>()?
            }
        } else {
            HashMap::new()
        };

        Ok(Self {
            path,
            instances: RwLock::new(instances),
            cipher,
        })
    }

    fn save(&self) -> anyhow::Result<()> {
        let mut list: Vec<Instance> = self
            .instances
            .read()
            .expect("registry lock poisoned")
            .values()
            .cloned()
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        for instance in &mut list {
            if let Some(plaintext) = &instance.credentials.token {
                instance.credentials.token = Some(self.cipher.encrypt(plaintext)?);
            }
        }
        let raw = serde_json::to_string_pretty(&list)?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, raw)?;
        Ok(())
    }

    pub fn list(&self) -> Vec<Instance> {
        let instances = self.instances.read().expect("registry lock poisoned");
        let mut list: Vec<Instance> = instances.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    pub fn get(&self, id: &InstanceId) -> Option<Instance> {
        self.instances
            .read()
            .expect("registry lock poisoned")
            .get(id)
            .cloned()
    }

    pub fn upsert(&self, instance: Instance) -> anyhow::Result<()> {
        instance.validate()?;
        {
            let mut instances = self.instances.write().expect("registry lock poisoned");
            instances.insert(instance.id, instance);
        }
        self.save()
    }

    pub fn remove(&self, id: &InstanceId) -> anyhow::Result<Option<Instance>> {
        let removed = {
            let mut instances = self.instances.write().expect("registry lock poisoned");
            instances.remove(id)
        };
        if removed.is_some() {
            self.save()?;
        }
        Ok(removed)
    }

    /// Tag -> member ids. Computed, never stored — see the module docs.
    pub fn groups(&self) -> BTreeMap<String, Vec<InstanceId>> {
        let instances = self.instances.read().expect("registry lock poisoned");
        let mut groups: BTreeMap<String, Vec<InstanceId>> = BTreeMap::new();
        for instance in instances.values() {
            for tag in &instance.tags {
                groups.entry(tag.clone()).or_default().push(instance.id);
            }
        }
        for members in groups.values_mut() {
            members.sort_by_key(|id| id.0);
        }
        groups
    }

    /// Every instance carrying `tag`, in display order.
    ///
    /// Returns an empty vec for an unknown tag rather than an error — but
    /// callers fanning a command out must **not** treat that as success. An
    /// empty group means the cue did nothing, and a show-control operator
    /// pressing a button that silently does nothing is the exact failure this
    /// project exists to avoid. See `web`'s group handler, which turns it
    /// into a 404.
    pub fn members_of(&self, tag: &str) -> Vec<Instance> {
        let instances = self.instances.read().expect("registry lock poisoned");
        let mut list: Vec<Instance> = instances
            .values()
            .filter(|i| i.tags.iter().any(|t| t == tag))
            .cloned()
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rookery-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_instance_can_belong_to_multiple_groups() {
        let registry = Registry::load_or_new(tempdir().join("registry.json")).unwrap();
        let instance = Instance::new("gfx-1", "192.168.1.50").with_tags(&["stage", "backup"]);
        let id = instance.id;
        registry.upsert(instance).unwrap();

        let groups = registry.groups();
        assert_eq!(groups["stage"], vec![id]);
        assert_eq!(groups["backup"], vec![id]);
        assert_eq!(registry.members_of("stage").len(), 1);
    }

    #[test]
    fn persists_across_reload_with_ports_intact() {
        let dir = tempdir();
        let path = dir.join("registry.json");
        {
            let registry = Registry::load_or_new(path.clone()).unwrap();
            let mut instance = Instance::new("gfx-1", "192.168.1.50");
            instance.osc_port = 9000;
            instance.http_port = 8080;
            registry.upsert(instance).unwrap();
        }
        let reloaded = Registry::load_or_new(path).unwrap();
        let list = reloaded.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].osc_port, 9000);
        assert_eq!(list[0].http_port, 8080);
    }

    #[test]
    fn a_registry_written_without_ports_gets_weblinkeds_defaults() {
        let dir = tempdir();
        let path = dir.join("registry.json");
        // What an operator hand-writing the file would most likely produce.
        std::fs::write(
            &path,
            r#"[{"id":"6f1b4e0e-0000-4000-8000-000000000001","name":"gfx-1","host":"10.0.0.5"}]"#,
        )
        .unwrap();
        let registry = Registry::load_or_new(path).unwrap();
        let list = registry.list();
        assert_eq!(list[0].osc_port, crate::instance::DEFAULT_OSC_PORT);
        assert_eq!(list[0].http_port, crate::instance::DEFAULT_HTTP_PORT);
        assert_eq!(list[0].osc_prefix, crate::instance::DEFAULT_OSC_PREFIX);
        assert!(list[0].poll, "polling should default on");
    }

    #[test]
    fn token_never_touches_disk_in_plaintext() {
        let dir = tempdir();
        let path = dir.join("registry.json");
        let mut instance = Instance::new("gfx-1", "192.168.1.50");
        instance.credentials.token = Some("super-secret".to_string());

        let registry = Registry::load_or_new(path.clone()).unwrap();
        registry.upsert(instance).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("super-secret"));

        assert_eq!(
            registry.list()[0].credentials.token.as_deref(),
            Some("super-secret")
        );
        let reloaded = Registry::load_or_new(path).unwrap();
        assert_eq!(
            reloaded.list()[0].credentials.token.as_deref(),
            Some("super-secret")
        );
    }

    #[test]
    fn a_bad_instance_is_refused_before_it_reaches_disk() {
        let dir = tempdir();
        let path = dir.join("registry.json");
        let registry = Registry::load_or_new(path.clone()).unwrap();

        // A URL where a host belongs — the single most likely thing to type.
        let bad = Instance::new("gfx-1", "http://192.168.1.50:7654");
        assert!(registry.upsert(bad).is_err());
        assert!(registry.list().is_empty());
        assert!(
            !path.exists(),
            "a rejected instance must not create the file"
        );
    }
}
