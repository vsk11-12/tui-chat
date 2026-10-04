use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::events::AppEvent;
use crate::ollama::{self, OllamaMessage};

// ── Shared types ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Chat,
    ModelSelect,
    /// Showing the "Are you sure you want to quit?" confirmation dialog.
    QuitConfirm,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// true while tokens are still being streamed in
    pub is_streaming: bool,
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct App {
    // ── UI state ──
    pub mode: Mode,
    pub messages: Vec<Message>,
    pub scroll: u16,          // vertical scroll in the messages pane (u16::MAX = pin to bottom)
    pub status: String,
    pub is_loading: bool,
    pub should_quit: bool,

    // ── Input ──
    pub input: String,
    pub cursor_pos: usize, // char index (not byte index)

    // ── Model ──
    pub models: Vec<String>,
    pub current_model: String,
    pub model_list_selected: usize,

    // ── Channels ──
    pub event_rx: UnboundedReceiver<AppEvent>,
    event_tx: UnboundedSender<AppEvent>,

    // ── Cancellation ──
    /// Flipped to `true` when the user presses Esc during streaming.
    /// A fresh Arc is created for every new request so old tasks are unaffected.
    cancel_flag: Arc<AtomicBool>,
}

impl App {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Self {
            mode: Mode::Chat,
            messages: Vec::new(),
            scroll: 0,
            status: "Connecting to Ollama…".to_string(),
            is_loading: false,
            should_quit: false,

            input: String::new(),
            cursor_pos: 0,

            models: Vec::new(),
            current_model: "Loading…".to_string(),
            model_list_selected: 0,

            event_rx,
            event_tx,

            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    // ── Startup ───────────────────────────────────────────────────────────────

    /// Spawn a background task that fetches the local Ollama model list.
    pub fn load_models(&self) {
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            match ollama::list_models().await {
                Ok(models) => {
                    let _ = tx.send(AppEvent::ModelsLoaded(models));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::ConnectionError(e.to_string()));
                }
            }
        });
    }

    // ── Event handling ────────────────────────────────────────────────────────

    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::StreamToken(token) => {
                if let Some(msg) = self.messages.last_mut() {
                    if msg.is_streaming {
                        msg.content.push_str(&token);
                        // Strip <think>…</think> blocks (reasoning models leak these)
                        msg.content = strip_think_blocks(&msg.content);
                    }
                }
                // Stay pinned to bottom while streaming
                self.scroll = u16::MAX;
            }

            AppEvent::StreamDone => {
                if let Some(msg) = self.messages.last_mut() {
                    msg.is_streaming = false;
                }
                self.is_loading = false;
                self.status = format!("Ready  ·  {}", self.current_model);
            }

            AppEvent::StreamCancelled => {
                if let Some(msg) = self.messages.last_mut() {
                    if msg.is_streaming {
                        // Append a small indicator so it's clear the response
                        // was cut short — but only if there was already content.
                        if !msg.content.is_empty() {
                            msg.content.push_str(" ▪");
                        } else {
                            msg.content = "⊘  Cancelled".to_string();
                        }
                        msg.is_streaming = false;
                    }
                }
                self.is_loading = false;
                self.status = format!("Cancelled  ·  {}", self.current_model);
            }

            AppEvent::StreamError(err) => {
                if let Some(msg) = self.messages.last_mut() {
                    if msg.is_streaming {
                        let suffix = if msg.content.is_empty() {
                            format!("⚠  Error: {}", err)
                        } else {
                            format!("\n\n⚠  Interrupted: {}", err)
                        };
                        msg.content.push_str(&suffix);
                        msg.is_streaming = false;
                    }
                }
                self.is_loading = false;
                let display = if err.len() > 55 {
                    format!("{}…", &err[..54])
                } else {
                    err
                };
                self.status = format!("Error: {}", display);
            }

            AppEvent::ModelsLoaded(models) => {
                if models.is_empty() {
                    self.current_model = "none".to_string();
                    self.status = "No models found  ·  run: ollama pull llama3.2".to_string();
                } else {
                    self.current_model = models[0].clone();
                    self.status = format!("Ready  ·  {}", self.current_model);
                }
                self.models = models;
            }

            AppEvent::ConnectionError(_err) => {
                self.current_model = "disconnected".to_string();
                self.status = "⚠  Cannot reach Ollama  ·  is it running?".to_string();
            }
        }
    }

    // ── Chat ──────────────────────────────────────────────────────────────────

    pub fn send_message(&mut self) {
        let trimmed = self.input.trim().to_string();
        if trimmed.is_empty() || self.is_loading || !self.is_connected() {
            return;
        }

        // Build API history from already-completed messages only
        let mut api_msgs: Vec<OllamaMessage> = self
            .messages
            .iter()
            .filter(|m| !m.is_streaming)
            .map(|m| OllamaMessage {
                role: match m.role {
                    Role::User => "user".to_string(),
                    Role::Assistant => "assistant".to_string(),
                },
                content: m.content.clone(),
            })
            .collect();
        api_msgs.push(OllamaMessage {
            role: "user".to_string(),
            content: trimmed.clone(),
        });

        // Update display state
        self.input.clear();
        self.cursor_pos = 0;
        self.messages.push(Message {
            role: Role::User,
            content: trimmed,
            is_streaming: false,
        });
        self.messages.push(Message {
            role: Role::Assistant,
            content: String::new(),
            is_streaming: true,
        });
        self.is_loading = true;
        self.scroll = u16::MAX; // pin to bottom
        self.status = format!("Generating…  ·  {}", self.current_model);

        // Fresh cancel flag for this request
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.cancel_flag = cancel_flag.clone();

        // Kick off the streaming request
        let model = self.current_model.clone();
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = ollama::stream_chat(model, api_msgs, tx.clone(), cancel_flag).await {
                let _ = tx.send(AppEvent::StreamError(e.to_string()));
            }
        });
    }

    /// Signal the active streaming task to stop.
    /// Safe to call even when not streaming — it's a no-op.
    pub fn cancel_stream(&mut self) {
        if self.is_loading {
            self.cancel_flag.store(true, Ordering::Relaxed);
            // `is_loading` and `status` are updated once StreamCancelled arrives.
        }
    }

    pub fn clear_messages(&mut self) {
        if !self.is_loading {
            self.messages.clear();
            self.scroll = 0;
        }
    }

    fn is_connected(&self) -> bool {
        !matches!(
            self.current_model.as_str(),
            "none" | "disconnected" | "Loading…"
        )
    }

    // ── Model picker ──────────────────────────────────────────────────────────

    pub fn open_model_select(&mut self) {
        self.model_list_selected = self
            .models
            .iter()
            .position(|m| m == &self.current_model)
            .unwrap_or(0);
        self.mode = Mode::ModelSelect;
    }

    pub fn model_select_up(&mut self) {
        if self.model_list_selected == 0 {
            self.model_list_selected = self.models.len().saturating_sub(1);
        } else {
            self.model_list_selected -= 1;
        }
    }

    pub fn model_select_down(&mut self) {
        if !self.models.is_empty() {
            self.model_list_selected = (self.model_list_selected + 1) % self.models.len();
        }
    }

    pub fn confirm_model_select(&mut self) {
        if let Some(m) = self.models.get(self.model_list_selected).cloned() {
            self.current_model = m.clone();
            self.status = format!("Ready  ·  {}", m);
        }
        self.mode = Mode::Chat;
    }

    // ── Scrolling ─────────────────────────────────────────────────────────────

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(3);
    }

    pub fn scroll_down(&mut self) {
        // render() will clamp this to max_scroll, so saturating_add is fine
        self.scroll = self.scroll.saturating_add(3);
    }

    pub fn page_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(20);
    }

    pub fn page_down(&mut self) {
        self.scroll = self.scroll.saturating_add(20);
    }

    // ── Input editing ─────────────────────────────────────────────────────────

    pub fn insert_char(&mut self, c: char) {
        let byte = self.char_to_byte(self.cursor_pos);
        self.input.insert(byte, c);
        self.cursor_pos += 1;
    }

    pub fn delete_char_before(&mut self) {
        if self.cursor_pos == 0 {
            return;
        }
        let end = self.char_to_byte(self.cursor_pos);
        let start = self.char_to_byte(self.cursor_pos - 1);
        self.input.drain(start..end);
        self.cursor_pos -= 1;
    }

    pub fn delete_char_after(&mut self) {
        let max = self.input.chars().count();
        if self.cursor_pos >= max {
            return;
        }
        let start = self.char_to_byte(self.cursor_pos);
        let end = self.char_to_byte(self.cursor_pos + 1);
        self.input.drain(start..end);
    }

    pub fn delete_word_before(&mut self) {
        // Delete back to previous whitespace boundary
        while self.cursor_pos > 0 {
            let prev_char = self
                .input
                .chars()
                .nth(self.cursor_pos - 1)
                .unwrap_or(' ');
            self.delete_char_before();
            if prev_char == ' ' {
                break;
            }
        }
    }

    pub fn move_cursor_left(&mut self) {
        self.cursor_pos = self.cursor_pos.saturating_sub(1);
    }

    pub fn move_cursor_right(&mut self) {
        let max = self.input.chars().count();
        if self.cursor_pos < max {
            self.cursor_pos += 1;
        }
    }

    pub fn move_cursor_home(&mut self) {
        self.cursor_pos = 0;
    }

    pub fn move_cursor_end(&mut self) {
        self.cursor_pos = self.input.chars().count();
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn char_to_byte(&self, char_idx: usize) -> usize {
        self.input
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.input.len())
    }
}

// ── Think-block filter ────────────────────────────────────────────────────────

fn strip_think_blocks(s: &str) -> String {
    let mut result = String::new();
    let mut rest = s;

    loop {
        match rest.find("<think>") {
            Some(open) => {
                // Keep everything before the opening tag
                result.push_str(&rest[..open]);
                rest = &rest[open + "<think>".len()..];

                match rest.find("</think>") {
                    // Complete block — discard the inner content and the tag
                    Some(close) => {
                        rest = &rest[close + "</think>".len()..];
                    }
                    // No closing tag yet; discard the rest (still streaming)
                    None => return result,
                }
            }
            None => {
                // No opening tag — strip any stray closing tags and we're done
                result.push_str(&rest.replace("</think>", ""));
                return result;
            }
        }
    }
}
