use fs2::FileExt;
use mini_agent_capabilities::{WebSearchBackend, WebSearchConfig};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const SETTINGS_FILE: &str = "web_search.json";
const CREDENTIALS_DIR: &str = "web-search-credentials";
const MAX_KEY_BYTES: usize = 4096;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchSettingsView {
    pub provider: String,
    pub deepseek_api_key_configured: bool,
    pub exa_api_key_configured: bool,
    pub kimi_api_key_configured: bool,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SavedSettings {
    provider: String,
}

#[derive(Clone)]
pub struct WebSearchSettingsStore {
    path: PathBuf,
    credentials: PathBuf,
}

impl WebSearchSettingsStore {
    pub fn machine_default() -> Result<Self, String> {
        let root = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .ok_or_else(|| "cannot resolve the user home directory".to_string())?;
        let root = root.join(".mini-agent");
        Ok(Self {
            path: root.join(SETTINGS_FILE),
            credentials: root.join(CREDENTIALS_DIR),
        })
    }

    pub fn at(path: PathBuf) -> Self {
        Self {
            credentials: path.with_file_name(CREDENTIALS_DIR),
            path,
        }
    }

    pub fn view(&self) -> Result<WebSearchSettingsView, String> {
        let _lock = self.lock()?;
        let saved = self.read_unlocked()?;
        Ok(WebSearchSettingsView {
            provider: saved.provider,
            deepseek_api_key_configured: self.configured("deepseek"),
            exa_api_key_configured: self.configured("exa"),
            kimi_api_key_configured: self.configured("kimi"),
        })
    }

    pub fn update(
        &self,
        provider: &str,
        deepseek_api_key: Option<&str>,
        exa_api_key: Option<&str>,
        kimi_api_key: Option<&str>,
    ) -> Result<WebSearchSettingsView, String> {
        if !["none", "deepseek", "exa", "kimi"].contains(&provider) {
            return Err("provider must be none, deepseek, exa, or kimi".to_string());
        }
        let _lock = self.lock()?;
        for (name, key) in [
            ("deepseek", deepseek_api_key),
            ("exa", exa_api_key),
            ("kimi", kimi_api_key),
        ] {
            if let Some(key) = key {
                self.write_key(name, key)?;
            }
        }
        self.write_settings(&SavedSettings {
            provider: provider.to_string(),
        })?;
        drop(_lock);
        self.view()
    }

    pub fn runtime_config(&self) -> Result<Option<WebSearchConfig>, String> {
        let _lock = self.lock()?;
        let saved = self.read_unlocked()?;
        if saved.provider == "none" {
            return Ok(None);
        }
        self.config_for_provider(&saved.provider)
    }

    pub fn test_search_for_provider(&self, provider: &str, query: &str) -> Result<String, String> {
        let _lock = self.lock()?;
        let config = self
            .config_for_provider(provider)?
            .ok_or_else(|| "configure this search provider's API key first".to_string())?;
        drop(_lock);
        mini_agent_capabilities::test_web_search(config, query).map_err(|error| error.to_string())
    }

    fn config_for_provider(&self, provider: &str) -> Result<Option<WebSearchConfig>, String> {
        let backend = match provider {
            "deepseek" => WebSearchBackend::DeepSeek,
            "exa" => WebSearchBackend::Exa,
            "kimi" => WebSearchBackend::Kimi,
            _ => return Err("provider must be deepseek, exa, or kimi".to_string()),
        };
        let key = self.read_key(provider)?;
        Ok(key.map(|key| WebSearchConfig::new(backend, key)))
    }

    pub fn test_search(&self, query: &str) -> Result<String, String> {
        let config = self.runtime_config()?.ok_or_else(|| {
            "select a search provider and configure its API key first".to_string()
        })?;
        mini_agent_capabilities::test_web_search(config, query).map_err(|error| error.to_string())
    }

    fn lock(&self) -> Result<File, String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "web search settings path has no parent directory".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|_| "cannot create web search settings directory".to_string())?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path.with_extension("lock"))
            .map_err(|_| "cannot open web search settings lock".to_string())?;
        file.lock_exclusive()
            .map_err(|_| "cannot lock web search settings".to_string())?;
        Ok(file)
    }

    fn read_unlocked(&self) -> Result<SavedSettings, String> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "cannot parse web search settings".to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SavedSettings {
                provider: "none".to_string(),
            }),
            Err(_) => Err("cannot read web search settings".to_string()),
        }
    }

    fn write_settings(&self, settings: &SavedSettings) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(settings)
            .map_err(|_| "cannot encode web search settings".to_string())?;
        self.write_atomic(&self.path, &bytes)?;
        Ok(())
    }

    fn write_key(&self, name: &str, key: &str) -> Result<(), String> {
        if key.len() > MAX_KEY_BYTES || key.contains(['\n', '\r', '\0']) {
            return Err("search API key is invalid or exceeds 4096 bytes".to_string());
        }
        fs::create_dir_all(&self.credentials)
            .map_err(|_| "cannot create search credential directory".to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.credentials, fs::Permissions::from_mode(0o700))
                .map_err(|_| "cannot restrict search credential directory".to_string())?;
        }
        let path = self.credentials.join(format!("{name}.key"));
        if key.trim().is_empty() {
            match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err("cannot remove search API key".to_string()),
            }
        } else {
            self.write_atomic(&path, key.trim().as_bytes())
        }
    }

    fn read_key(&self, name: &str) -> Result<Option<String>, String> {
        match fs::read_to_string(self.credentials.join(format!("{name}.key"))) {
            Ok(key) if !key.trim().is_empty() => Ok(Some(key)),
            Ok(_) => Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("cannot read search API key".to_string()),
        }
    }

    fn configured(&self, name: &str) -> bool {
        fs::metadata(self.credentials.join(format!("{name}.key")))
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
    }

    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .map_err(|_| "cannot write web search settings".to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
                .map_err(|_| "cannot restrict web search settings permissions".to_string())?;
        }
        file.write_all(bytes)
            .map_err(|_| "cannot write web search settings".to_string())?;
        drop(file);
        replace_file(&temporary, path).map_err(|_| "cannot replace web search settings".to_string())
    }
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(temporary, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_keep_keys_write_only_and_gate_runtime_exposure() {
        let root = crate::test_support::test_root();
        let store = WebSearchSettingsStore::at(root.join(SETTINGS_FILE));
        let configured = store
            .update("kimi", None, None, Some("kimi-test-key"))
            .unwrap();
        assert_eq!(configured.provider, "kimi");
        assert!(configured.kimi_api_key_configured);
        assert!(store.runtime_config().unwrap().is_some());
        let encoded = serde_json::to_string(&configured).unwrap();
        assert!(!encoded.contains("kimi-test-key"));

        let disabled = store.update("none", None, None, None).unwrap();
        assert_eq!(disabled.provider, "none");
        assert!(store.runtime_config().unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_provider_without_key_does_not_create_search_runtime() {
        let root = crate::test_support::test_root();
        let store = WebSearchSettingsStore::at(root.join(SETTINGS_FILE));
        let view = store.update("exa", None, None, None).unwrap();
        assert_eq!(view.provider, "exa");
        assert!(!view.exa_api_key_configured);
        assert!(store.runtime_config().unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(windows)]
fn replace_file(temporary: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
