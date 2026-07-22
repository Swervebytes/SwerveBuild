//! Local image generation via ComfyUI HTTP API (S18 foundation).
//!
//! Models are **not** managed as GGUFs here — download/install checkpoints
//! through ComfyUI Manager (or drop into Comfy's models/checkpoints). We only
//! detect the server, list checkpoints Comfy already has, and run a minimal
//! txt2img graph. Saves results into the attachments dir (asset protocol).

use crate::store::Store;
use base64::Engine;
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::thread;
use std::time::{Duration, Instant};

pub const DEFAULT_COMFY_URL: &str = "http://127.0.0.1:8188";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalImageStatus {
    pub reachable: bool,
    pub base_url: String,
    pub note: String,
    pub checkpoints: Vec<String>,
}

pub fn comfy_base_url() -> String {
    let prefs = Store::load().preferences;
    let u = prefs.comfy_base_url.trim();
    if u.is_empty() {
        DEFAULT_COMFY_URL.to_string()
    } else {
        u.trim_end_matches('/').to_string()
    }
}

pub fn is_available() -> bool {
    probe().reachable
}

/// Best-effort probe (short timeout). Never panics.
pub fn probe() -> LocalImageStatus {
    let base = comfy_base_url();
    match probe_inner(&base) {
        Ok(checkpoints) => LocalImageStatus {
            reachable: true,
            base_url: base,
            note: if checkpoints.is_empty() {
                "ComfyUI up — no checkpoints found; install one in Comfy Manager"
                    .into()
            } else {
                format!("ComfyUI up · {} checkpoint(s)", checkpoints.len())
            },
            checkpoints,
        },
        Err(e) => LocalImageStatus {
            reachable: false,
            base_url: base,
            note: format!("ComfyUI not reachable — {e}"),
            checkpoints: vec![],
        },
    }
}

fn probe_inner(base: &str) -> Result<Vec<String>, String> {
    // system_stats is lightweight; object_info lists checkpoints.
    let _stats = http_get(&format!("{base}/system_stats"))?;
    let info = http_get(&format!("{base}/object_info/CheckpointLoaderSimple")).ok();
    let checkpoints = info
        .as_ref()
        .map(|v| parse_checkpoint_names(v))
        .unwrap_or_default();
    Ok(checkpoints)
}

fn parse_checkpoint_names(v: &Value) -> Vec<String> {
    // Shape: { "CheckpointLoaderSimple": { "input": { "required": { "ckpt_name": [[names], ...] } } } }
    let root = v
        .get("CheckpointLoaderSimple")
        .or_else(|| v.as_object().and_then(|m| m.values().next()));
    let Some(root) = root else {
        return vec![];
    };
    let names = root
        .pointer("/input/required/ckpt_name/0")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    names
        .into_iter()
        .filter_map(|n| n.as_str().map(|s| s.to_string()))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Run a minimal SD txt2img on ComfyUI; return path under attachments.
pub fn generate(prompt: &str, negative: Option<&str>, width: u32, height: u32) -> Result<String, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("Prompt is empty".into());
    }
    if prompt.chars().count() > 2000 {
        return Err("Prompt too long (max 2000 chars)".into());
    }
    let status = probe();
    if !status.reachable {
        return Err(status.note);
    }
    let ckpt = status
        .checkpoints
        .first()
        .cloned()
        .ok_or_else(|| {
            "ComfyUI has no checkpoints. Open ComfyUI Manager and download a model first.".to_string()
        })?;

    let w = width.clamp(256, 1280);
    let h = height.clamp(256, 1280);
    let neg = negative.unwrap_or("").trim();
    let base = status.base_url;
    let client_id = uuid::Uuid::new_v4().to_string();
    let workflow = minimal_txt2img_workflow(&ckpt, prompt, neg, w, h);

    let body = json!({
        "prompt": workflow,
        "client_id": client_id,
    });
    let resp = http_post_json(&format!("{base}/prompt"), &body)?;
    let prompt_id = resp
        .get("prompt_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("Comfy /prompt missing prompt_id: {resp}"))?
        .to_string();

    let image_meta = wait_for_images(&base, &prompt_id, Duration::from_secs(300))?;
    let (filename, subfolder, img_type) = image_meta;
    let bytes = fetch_view(&base, &filename, &subfolder, &img_type)?;
    save_png_bytes(&bytes)
}

fn minimal_txt2img_workflow(
    ckpt: &str,
    positive: &str,
    negative: &str,
    width: u32,
    height: u32,
) -> Value {
    // Standard 5-node graph used by many Comfy API examples.
    json!({
        "3": {
            "class_type": "KSampler",
            "inputs": {
                "seed": (uuid::Uuid::new_v4().as_u128() % (u32::MAX as u128)) as u32,
                "steps": 20,
                "cfg": 7.0,
                "sampler_name": "euler",
                "scheduler": "normal",
                "denoise": 1.0,
                "model": ["4", 0],
                "positive": ["6", 0],
                "negative": ["7", 0],
                "latent_image": ["5", 0]
            }
        },
        "4": {
            "class_type": "CheckpointLoaderSimple",
            "inputs": { "ckpt_name": ckpt }
        },
        "5": {
            "class_type": "EmptyLatentImage",
            "inputs": { "width": width, "height": height, "batch_size": 1 }
        },
        "6": {
            "class_type": "CLIPTextEncode",
            "inputs": { "text": positive, "clip": ["4", 1] }
        },
        "7": {
            "class_type": "CLIPTextEncode",
            "inputs": { "text": negative, "clip": ["4", 1] }
        },
        "8": {
            "class_type": "VAEDecode",
            "inputs": { "samples": ["3", 0], "vae": ["4", 2] }
        },
        "9": {
            "class_type": "SaveImage",
            "inputs": { "filename_prefix": "swerve", "images": ["8", 0] }
        }
    })
}

fn wait_for_images(
    base: &str,
    prompt_id: &str,
    timeout: Duration,
) -> Result<(String, String, String), String> {
    let start = Instant::now();
    loop {
        if start.elapsed() > timeout {
            return Err(format!(
                "Comfy timed out after {}s waiting for prompt {prompt_id}",
                timeout.as_secs()
            ));
        }
        let hist = http_get(&format!("{base}/history/{prompt_id}")).unwrap_or(json!({}));
        if let Some(entry) = hist.get(prompt_id) {
            if let Some(outputs) = entry.get("outputs") {
                if let Some(found) = find_first_image(outputs) {
                    return Ok(found);
                }
            }
            if entry.get("status").and_then(|s| s.get("status_str")).and_then(|s| s.as_str())
                == Some("error")
            {
                return Err(format!("Comfy reported error for prompt {prompt_id}: {entry}"));
            }
        }
        thread::sleep(Duration::from_millis(750));
    }
}

fn find_first_image(outputs: &Value) -> Option<(String, String, String)> {
    let obj = outputs.as_object()?;
    for (_node, out) in obj {
        let images = out.get("images")?.as_array()?;
        for img in images {
            let filename = img.get("filename")?.as_str()?.to_string();
            let subfolder = img
                .get("subfolder")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let img_type = img
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("output")
                .to_string();
            return Some((filename, subfolder, img_type));
        }
    }
    None
}

fn fetch_view(
    base: &str,
    filename: &str,
    subfolder: &str,
    img_type: &str,
) -> Result<Vec<u8>, String> {
    let url = format!(
        "{base}/view?filename={}&subfolder={}&type={}",
        urlencoding_lite(filename),
        urlencoding_lite(subfolder),
        urlencoding_lite(img_type)
    );
    http_get_bytes(&url)
}

fn save_png_bytes(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > 40 * 1024 * 1024 {
        return Err("Generated image too large".into());
    }
    let dir = crate::acp::attachments_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Detect PNG magic; otherwise still write as .png if unknown.
    let ext = if bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47]) {
        "png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "jpg"
    } else {
        "png"
    };
    let path = dir.join(format!("{}.{ext}", crate::store::Store::new_id()));
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

fn urlencoding_lite(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn http_get(url: &str) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout(Duration::from_secs(8))
        .build();
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("GET {url}: {e}"))?;
    resp.into_json()
        .map_err(|e| format!("JSON from {url}: {e}"))
}

fn http_get_bytes(url: &str) -> Result<Vec<u8>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout(Duration::from_secs(60))
        .build();
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("GET {url}: {e}"))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .read_to_end(&mut buf)
        .map_err(|e| format!("read body: {e}"))?;
    Ok(buf)
}

fn http_post_json(url: &str, body: &Value) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout(Duration::from_secs(30))
        .build();
    let resp = agent
        .post(url)
        .set("Content-Type", "application/json")
        .send_json(body)
        .map_err(|e| format!("POST {url}: {e}"))?;
    resp.into_json()
        .map_err(|e| format!("JSON from {url}: {e}"))
}

/// Data-URL helper for tests / paste-style save without Comfy.
#[allow(dead_code)]
pub fn save_data_url_for_tests(data_url: &str) -> Result<String, String> {
    let payload = data_url
        .split_once(',')
        .map(|(_, v)| v)
        .unwrap_or(data_url);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|e| e.to_string())?;
    save_png_bytes(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_checkpoint_names_from_object_info_shape() {
        let v = json!({
            "CheckpointLoaderSimple": {
                "input": {
                    "required": {
                        "ckpt_name": [["modelA.safetensors", "modelB.ckpt"], "COMBO"]
                    }
                }
            }
        });
        let names = parse_checkpoint_names(&v);
        assert_eq!(names, vec!["modelA.safetensors", "modelB.ckpt"]);
    }

    #[test]
    fn urlencoding_encodes_spaces() {
        assert_eq!(urlencoding_lite("a b"), "a%20b");
    }

    #[test]
    fn default_url_is_localhost_comfy() {
        assert!(DEFAULT_COMFY_URL.contains("8188"));
    }
}
