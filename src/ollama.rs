use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::Result;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::events::AppEvent;

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OllamaMessage {
    pub role: String,
    pub content: String,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [OllamaMessage],
    stream: bool,
}

/// One newline-delimited JSON object from /api/chat (stream=true)
#[derive(Deserialize)]
struct ChatChunk {
    message: ChunkMessage,
    done: bool,
}

#[derive(Deserialize)]
struct ChunkMessage {
    content: String,
}

#[derive(Deserialize)]
struct ModelsResponse {
    models: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    name: String,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Fetch the list of locally installed Ollama models.
pub async fn list_models() -> Result<Vec<String>> {
    let client = Client::new();
    let resp = client
        .get("http://localhost:11434/api/tags")
        .timeout(Duration::from_secs(5))
        .send()
        .await?;

    let data: ModelsResponse = resp.json().await?;
    let mut names: Vec<String> = data.models.into_iter().map(|m| m.name).collect();
    names.sort();
    Ok(names)
}

/// Send a chat request and stream tokens back via `tx`.
///
/// `cancel` is polled after each chunk; when set to `true` by the main thread
/// (Esc key) the function sends `StreamCancelled` and returns cleanly.
///
/// The caller is responsible for spawning this as a tokio task.
pub async fn stream_chat(
    model: String,
    messages: Vec<OllamaMessage>,
    tx: UnboundedSender<AppEvent>,
    cancel: Arc<AtomicBool>,
) -> Result<()> {
    let client = Client::new();
    let body = ChatRequest {
        model: &model,
        messages: &messages,
        stream: true,
    };

    let resp = client
        .post("http://localhost:11434/api/chat")
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let err = resp
            .text()
            .await
            .unwrap_or_else(|_| "Unknown API error".into());
        let _ = tx.send(AppEvent::StreamError(err));
        return Ok(());
    }

    let mut byte_stream = resp.bytes_stream();
    // Incomplete line carried over between chunks
    let mut buf = String::new();

    while let Some(chunk) = byte_stream.next().await {
        // ── Cancellation check ────────────────────────────────────────────────
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(AppEvent::StreamCancelled);
            return Ok(());
        }

        let bytes = chunk?;
        buf.push_str(&String::from_utf8_lossy(&bytes));

        // Process every complete newline-terminated JSON object
        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim().to_string();
            buf = buf[nl + 1..].to_string();

            if line.is_empty() {
                continue;
            }

            match serde_json::from_str::<ChatChunk>(&line) {
                Ok(chunk) => {
                    if chunk.done {
                        let _ = tx.send(AppEvent::StreamDone);
                    } else if !chunk.message.content.is_empty() {
                        let _ = tx.send(AppEvent::StreamToken(chunk.message.content));
                    }
                }
                Err(_) => {} // Skip malformed lines (e.g. error JSON with different schema)
            }
        }
    }

    Ok(())
}
