use mini_agent_core::ToolError;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_IMAGES_PER_REQUEST: usize = 4;
pub const MAX_INLINE_REQUEST_BYTES: usize = 8 * 1024 * 1024;
const MAX_STORED_IMAGES: usize = 16;
const FILE_EXPIRY_SECONDS: u64 = 7 * 24 * 60 * 60;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const VISION_MODEL: &str = "deepseek-v4-flash-vision-exp";
const GLM_VISION_MODEL: &str = "glm-5.3-flash";

#[cfg(test)]
pub const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

pub trait FileUploader: Send + Sync {
    fn upload(&self, filename: &str, media_type: &str, bytes: &[u8]) -> Result<String, ToolError>;
}

struct NoUpload;

impl FileUploader for NoUpload {
    fn upload(&self, _: &str, _: &str, _: &[u8]) -> Result<String, ToolError> {
        Err(ToolError("Files API is not configured".to_string()))
    }
}

pub struct DeepSeekFiles {
    api_key: String,
    endpoint: String,
}

impl DeepSeekFiles {
    pub fn new(api_key: String, base_url: &str) -> Self {
        let base_url = base_url.trim().trim_end_matches('/');
        Self {
            api_key,
            endpoint: format!("{base_url}/files"),
        }
    }
}

impl FileUploader for DeepSeekFiles {
    fn upload(&self, filename: &str, media_type: &str, bytes: &[u8]) -> Result<String, ToolError> {
        let endpoint = self.endpoint.clone();
        let api_key = self.api_key.clone();
        let filename = filename.to_string();
        let media_type = media_type.to_string();
        let bytes = bytes.to_vec();
        run_blocking(async move {
            let part = reqwest::multipart::Part::bytes(bytes)
                .file_name(filename)
                .mime_str(&media_type)
                .map_err(|error| ToolError(format!("invalid image media type: {error}")))?;
            let form = reqwest::multipart::Form::new()
                .text("purpose", "user_data")
                .text("expires_after[anchor]", "created_at")
                .text("expires_after[seconds]", FILE_EXPIRY_SECONDS.to_string())
                .part("file", part);
            let client = reqwest::Client::builder()
                .timeout(UPLOAD_TIMEOUT)
                .build()
                .map_err(|error| ToolError(format!("cannot build files client: {error}")))?;
            let response = client
                .post(&endpoint)
                .bearer_auth(&api_key)
                .multipart(form)
                .send()
                .await
                .map_err(|error| ToolError(format!("files upload failed: {error}")))?;
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "response body unavailable".to_string());
            if !status.is_success() {
                return Err(ToolError(format!("files upload failed ({status}): {body}")));
            }
            let value: Value = serde_json::from_str(&body)
                .map_err(|error| ToolError(format!("invalid files response: {error}")))?;
            value
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| id.starts_with("file-"))
                .map(str::to_string)
                .ok_or_else(|| ToolError("files response is missing file_id".to_string()))
        })
    }
}

#[derive(Clone)]
pub struct ImageStore {
    inner: Arc<Mutex<Inner>>,
    uploader: Arc<dyn FileUploader>,
}

struct Inner {
    dir: Option<PathBuf>,
    order: VecDeque<String>,
    records: HashMap<String, StoredImage>,
}

#[derive(Clone)]
pub struct StoredImage {
    pub id: String,
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
    pub file_id: Option<String>,
    pub display_path: String,
}

impl ImageStore {
    pub fn memory_only() -> Self {
        Self::with_uploader(Arc::new(NoUpload))
    }

    pub fn for_provider(api_key: String, base_url: &str) -> Self {
        if uses_deepseek_files(base_url) {
            Self::with_uploader(Arc::new(DeepSeekFiles::new(api_key, base_url)))
        } else {
            Self::memory_only()
        }
    }

    pub fn with_uploader(uploader: Arc<dyn FileUploader>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                dir: None,
                order: VecDeque::new(),
                records: HashMap::new(),
            })),
            uploader,
        }
    }

    pub fn bind_session_file(&self, session_jsonl: &Path) {
        let dir = session_jsonl
            .parent()
            .map(|parent| parent.join("attachments"));
        if let Some(dir) = &dir {
            let _ = fs::create_dir_all(dir);
            self.reload_from_dir(dir);
        }
        self.inner.lock().unwrap().dir = dir;
    }

    fn reload_from_dir(&self, dir: &Path) {
        let mut candidates = match fs::read_dir(dir) {
            Ok(entries) => entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_file() && declared_media_type(path).is_some())
                .collect::<Vec<_>>(),
            Err(_) => return,
        };
        candidates.sort_by_key(|path| std::cmp::Reverse(attachment_recency(path)));
        let mut loaded = Vec::new();
        for path in candidates {
            if loaded.len() >= MAX_STORED_IMAGES {
                break;
            }
            if let Some(stored) = load_attachment(dir, &path) {
                loaded.push(stored);
            }
        }
        loaded.reverse();
        let mut inner = self.inner.lock().unwrap();
        for stored in loaded {
            if inner.order.len() >= MAX_STORED_IMAGES {
                break;
            }
            if inner.records.contains_key(&stored.id) {
                continue;
            }
            inner.order.push_back(stored.id.clone());
            inner.records.insert(stored.id.clone(), stored);
        }
    }

    pub fn get(&self, id: &str) -> Option<StoredImage> {
        self.inner.lock().unwrap().records.get(id).cloned()
    }

    fn insert(&self, stored: StoredImage) -> StoredImage {
        let mut inner = self.inner.lock().unwrap();
        if let Some(dir) = &inner.dir {
            let _ = fs::create_dir_all(dir);
            let ext = extension_for(stored.media_type);
            let path = dir.join(format!("{}{ext}", stored.id));
            let _ = fs::write(&path, &stored.bytes);
            let _ = fs::write(
                dir.join(format!("{}.json", stored.id)),
                json!({
                    "id": stored.id,
                    "file_id": stored.file_id,
                    "media_type": stored.media_type,
                    "bytes": stored.bytes.len(),
                    "display_path": stored.display_path,
                })
                .to_string(),
            );
        }
        while inner.order.len() >= MAX_STORED_IMAGES {
            if let Some(old) = inner.order.pop_front() {
                inner.records.remove(&old);
            }
        }
        inner.order.push_back(stored.id.clone());
        inner.records.insert(stored.id.clone(), stored.clone());
        stored
    }

    pub fn save(
        &self,
        display_path: &str,
        media_type: &'static str,
        bytes: Vec<u8>,
    ) -> Result<StoredImage, ToolError> {
        let id = next_attachment_id();
        let filename = Path::new(display_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image");
        let file_id = self.uploader.upload(filename, media_type, &bytes).ok();
        Ok(self.insert(StoredImage {
            id,
            media_type,
            bytes,
            file_id,
            display_path: display_path.to_string(),
        }))
    }
}

fn load_attachment(dir: &Path, path: &Path) -> Option<StoredImage> {
    let id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| stem.starts_with("att-"))?
        .to_string();
    let bytes = fs::read(path).ok()?;
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let media_type = detect_image(&bytes)?;
    let meta = read_attachment_meta(&dir.join(format!("{id}.json")));
    let file_id = meta
        .as_ref()
        .and_then(|value| value.get("file_id"))
        .and_then(Value::as_str)
        .filter(|id| id.starts_with("file-"))
        .map(str::to_string);
    let display_path = meta
        .as_ref()
        .and_then(|value| value.get("display_path"))
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&id)
                .to_string()
        });
    Some(StoredImage {
        id,
        media_type,
        bytes,
        file_id,
        display_path,
    })
}

fn read_attachment_meta(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn attachment_recency(path: &Path) -> (u128, u64) {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let rest = stem.strip_prefix("att-").unwrap_or(stem);
    let (nanos, seq) = rest.rsplit_once('-').unwrap_or((rest, "0"));
    (nanos.parse().unwrap_or(0), seq.parse().unwrap_or(0))
}

fn next_attachment_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let seq = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("att-{nanos}-{seq}")
}

fn extension_for(media_type: &str) -> &'static str {
    match media_type {
        "image/jpeg" => ".jpg",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        _ => ".png",
    }
}

pub fn detect_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 8 && bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        Some("image/jpeg")
    } else if bytes.len() >= 6 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

pub fn declared_media_type(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        _ => None,
    }
}

pub fn format_envelope(stored: &StoredImage) -> String {
    let file_id = stored
        .file_id
        .as_deref()
        .map(|id| format!(" file_id=\"{id}\""))
        .unwrap_or_default();
    format!(
        "<path>{}</path>\n<type>image</type>\n<mini_agent_image id=\"{}\"{file_id} media_type=\"{}\" bytes=\"{}\"/>",
        stored.display_path,
        stored.id,
        stored.media_type,
        stored.bytes.len()
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageEnvelope {
    pub id: String,
    pub file_id: Option<String>,
    pub media_type: String,
}

pub fn parse_envelope(content: &str) -> Option<ImageEnvelope> {
    let start = content.find("<mini_agent_image ")?;
    let tag = content[start..].split("/>").next()?;
    Some(ImageEnvelope {
        id: attr(tag, "id")?,
        file_id: attr(tag, "file_id"),
        media_type: attr(tag, "media_type")?,
    })
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let start = tag.find(&key)? + key.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

pub fn vision_model_for(configured: &str, has_images: bool) -> String {
    if !has_images {
        return configured.to_string();
    }
    if is_deepseek_text_model(configured) {
        VISION_MODEL.to_string()
    } else if is_glm_53_text_model(configured) {
        GLM_VISION_MODEL.to_string()
    } else {
        configured.to_string()
    }
}

pub fn uses_deepseek_files(base_url: &str) -> bool {
    base_url.to_ascii_lowercase().contains("api.deepseek.com")
}

fn is_deepseek_text_model(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    !model.contains("vision")
        && (model.starts_with("deepseek-v4-flash") || model.starts_with("deepseek-v4-pro"))
}

fn is_glm_53_text_model(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model == "glm-5.3" || (model.starts_with("glm-5.3") && !model.contains("flash"))
}

#[derive(Clone, Debug)]
pub enum ProjectedImage {
    FileId(String),
    Inline { data_url: String },
    Missing(String),
}

pub fn project_images(contents: &[String], store: &ImageStore) -> Vec<Option<ProjectedImage>> {
    let mut resolved = contents
        .iter()
        .map(|content| parse_envelope(content).map(|envelope| resolve_envelope(&envelope, store)))
        .collect::<Vec<_>>();
    let live = resolved
        .iter()
        .enumerate()
        .filter_map(|(index, image)| match image {
            Some(ProjectedImage::FileId(_) | ProjectedImage::Inline { .. }) => Some(index),
            _ => None,
        })
        .collect::<Vec<_>>();
    if live.len() > MAX_IMAGES_PER_REQUEST {
        let drop = live.len() - MAX_IMAGES_PER_REQUEST;
        for index in live.into_iter().take(drop) {
            resolved[index] = Some(ProjectedImage::Missing(
                "older image omitted; at most 4 images are sent per request".to_string(),
            ));
        }
    }
    let any_inline = resolved
        .iter()
        .any(|image| matches!(image, Some(ProjectedImage::Inline { .. })));
    if any_inline {
        for (index, image) in resolved.iter_mut().enumerate() {
            if let Some(ProjectedImage::FileId(_)) = image {
                *image = match parse_envelope(&contents[index])
                    .and_then(|envelope| store.get(&envelope.id))
                {
                    Some(stored) => Some(inline_projection(&stored)),
                    None => Some(ProjectedImage::Missing(
                        "image attachment is no longer available".to_string(),
                    )),
                };
            }
        }
        let mut encoded = 0_usize;
        for image in &mut resolved {
            if let Some(ProjectedImage::Inline { data_url, .. }) = image {
                encoded = encoded.saturating_add(data_url.len());
                if encoded > MAX_INLINE_REQUEST_BYTES {
                    *image = Some(ProjectedImage::Missing(
                        "image omitted; inline payload exceeds 8 MiB".to_string(),
                    ));
                }
            }
        }
    }
    resolved
}

fn resolve_envelope(envelope: &ImageEnvelope, store: &ImageStore) -> ProjectedImage {
    if let Some(file_id) = envelope.file_id.clone() {
        return ProjectedImage::FileId(file_id);
    }
    match store.get(&envelope.id) {
        Some(stored) => {
            if let Some(file_id) = stored.file_id.clone() {
                ProjectedImage::FileId(file_id)
            } else {
                inline_projection(&stored)
            }
        }
        None => ProjectedImage::Missing(format!(
            "image attachment {} is no longer available",
            envelope.id
        )),
    }
}

fn inline_projection(stored: &StoredImage) -> ProjectedImage {
    ProjectedImage::Inline {
        data_url: format!(
            "data:{};base64,{}",
            stored.media_type,
            b64_encode(&stored.bytes)
        ),
    }
}

fn b64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        let b1 = if i + 1 < bytes.len() { bytes[i + 1] } else { 0 };
        let b2 = if i + 2 < bytes.len() { bytes[i + 2] } else { 0 };
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | b2 as u32;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        if i + 1 < bytes.len() {
            out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < bytes.len() {
            out.push(TABLE[(n & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}

pub fn wire_image_block(image: &ProjectedImage) -> Value {
    match image {
        ProjectedImage::FileId(file_id) => json!({
            "type": "input_image",
            "file_id": file_id
        }),
        ProjectedImage::Inline { data_url, .. } => json!({
            "type": "input_image",
            "image_url": data_url
        }),
        ProjectedImage::Missing(note) => json!({
            "type": "input_text",
            "text": note
        }),
    }
}

/// GLM-5.3-Flash documents Chat Completions `type: image_url` with nested
/// `image_url.url` (URL or Base64 data URL) on user `messages[].content[]`.
/// Image turns post Coding Plan `{base}/chat/completions`, not Responses.
pub fn wire_glm_image_block(image: &ProjectedImage) -> Value {
    match image {
        ProjectedImage::Inline { data_url, .. } => json!({
            "type": "image_url",
            "image_url": { "url": data_url }
        }),
        ProjectedImage::FileId(_) | ProjectedImage::Missing(_) => wire_image_block(image),
    }
}

pub fn is_glm_model(model: &str) -> bool {
    model.to_ascii_lowercase().starts_with("glm-")
}

fn run_blocking<T>(
    fut: impl std::future::Future<Output = Result<T, ToolError>> + Send + 'static,
) -> Result<T, ToolError>
where
    T: Send + 'static,
{
    let join = std::thread::Builder::new()
        .name("mini-agent-files".into())
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| ToolError(format!("cannot start files runtime: {error}")))?
                .block_on(fut)
        })
        .map_err(|error| ToolError(format!("cannot start files thread: {error}")))?;
    join.join()
        .unwrap_or_else(|_| Err(ToolError("files thread panicked".to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubFiles;

    impl FileUploader for StubFiles {
        fn upload(&self, _: &str, _: &str, _: &[u8]) -> Result<String, ToolError> {
            Ok("file-api-test".to_string())
        }
    }

    #[test]
    fn detect_png_magic() {
        assert_eq!(detect_image(TINY_PNG), Some("image/png"));
        assert_eq!(detect_image(b"not-an-image"), None);
    }

    #[test]
    fn vision_model_swaps_deepseek_text_routes() {
        assert_eq!(
            vision_model_for("deepseek-v4-flash", true),
            "deepseek-v4-flash-vision-exp"
        );
        assert_eq!(
            vision_model_for("deepseek-v4-flash", false),
            "deepseek-v4-flash"
        );
        assert_eq!(
            vision_model_for("deepseek-v4-flash-vision-exp", true),
            "deepseek-v4-flash-vision-exp"
        );
        assert_eq!(vision_model_for("gpt-4o", true), "gpt-4o");
        assert_eq!(vision_model_for("glm-5.3", true), "glm-5.3-flash");
        assert_eq!(vision_model_for("glm-5.3", false), "glm-5.3");
        assert_eq!(vision_model_for("glm-5.3-flash", true), "glm-5.3-flash");
        assert!(is_glm_model("glm-5.3-flash"));
        assert!(is_glm_model("GLM-5.3"));
        assert!(!is_glm_model("deepseek-v4-flash"));
        assert!(!uses_deepseek_files("https://open.bigmodel.cn/api/v1"));
        assert!(uses_deepseek_files("https://api.deepseek.com"));
    }

    #[test]
    fn glm_image_block_nests_data_url() {
        let image = ProjectedImage::Inline {
            data_url: "data:image/png;base64,abcd".to_string(),
        };
        let block = wire_glm_image_block(&image);
        assert_eq!(block["type"], "image_url");
        assert_eq!(block["image_url"]["url"], "data:image/png;base64,abcd");
        let deepseek = wire_image_block(&image);
        assert_eq!(deepseek["type"], "input_image");
        assert_eq!(deepseek["image_url"], "data:image/png;base64,abcd");
    }

    #[test]
    fn bind_session_reloads_bytes_without_reupload() {
        let root = std::env::temp_dir().join(format!(
            "mini-agent-img-reload-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let session = root.join("session.jsonl");
        fs::write(&session, "").unwrap();
        let store = ImageStore::with_uploader(Arc::new(StubFiles));
        store.bind_session_file(&session);
        let stored = store
            .save("shot.png", "image/png", TINY_PNG.to_vec())
            .unwrap();
        let id = stored.id.clone();
        assert_eq!(stored.file_id.as_deref(), Some("file-api-test"));
        drop(store);

        let restored = ImageStore::memory_only();
        restored.bind_session_file(&session);
        let got = restored.get(&id).expect("attachment should reload");
        assert_eq!(got.bytes, TINY_PNG);
        assert_eq!(got.file_id.as_deref(), Some("file-api-test"));
        assert_eq!(got.display_path, "shot.png");
        let envelope = format_envelope(&got);
        let projected = project_images(&[envelope], &restored);
        assert!(matches!(
            projected[0],
            Some(ProjectedImage::FileId(ref file_id)) if file_id == "file-api-test"
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bind_session_reloads_inline_images_without_file_id() {
        let root = std::env::temp_dir().join(format!(
            "mini-agent-img-inline-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let session = root.join("session.jsonl");
        fs::write(&session, "").unwrap();
        let store = ImageStore::memory_only();
        store.bind_session_file(&session);
        let stored = store
            .save("pic.png", "image/png", TINY_PNG.to_vec())
            .unwrap();
        let id = stored.id.clone();
        assert!(stored.file_id.is_none());
        drop(store);

        let restored = ImageStore::memory_only();
        restored.bind_session_file(&session);
        let got = restored.get(&id).expect("inline attachment should reload");
        assert_eq!(got.bytes, TINY_PNG);
        assert!(got.file_id.is_none());
        let envelope = format_envelope(&got);
        let projected = project_images(&[envelope], &restored);
        assert!(matches!(projected[0], Some(ProjectedImage::Inline { .. })));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn save_uploads_and_projects_file_id() {
        let store = ImageStore::with_uploader(Arc::new(StubFiles));
        let stored = store
            .save("shot.png", "image/png", TINY_PNG.to_vec())
            .unwrap();
        let out = format_envelope(&stored);
        assert!(out.contains("file_id=\"file-api-test\""));
        let projected = project_images(&[out], &store);
        assert!(matches!(
            projected[0],
            Some(ProjectedImage::FileId(ref id)) if id == "file-api-test"
        ));
    }

    #[test]
    fn older_images_are_placeholder_after_four() {
        let store = ImageStore::with_uploader(Arc::new(StubFiles));
        let contents = (0..5)
            .map(|i| {
                format!(
                    "<mini_agent_image id=\"att-{i}\" file_id=\"file-api-{i}\" media_type=\"image/png\" bytes=\"8\"/>"
                )
            })
            .collect::<Vec<_>>();
        let projected = project_images(&contents, &store);
        assert!(matches!(projected[0], Some(ProjectedImage::Missing(_))));
        assert_eq!(
            projected
                .iter()
                .filter(|image| matches!(image, Some(ProjectedImage::FileId(_))))
                .count(),
            4
        );
    }
}
