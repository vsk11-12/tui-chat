# tui-chat

> A fast, lightweight, and responsive terminal user interface (TUI) for local Ollama LLMs built with Rust.

![Rust](https://img.shields.io/badge/Rust-2024_Edition-orange.svg)
![License](https://img.shields.io/badge/License-MIT-blue.svg)

## Features

- ⚡ **Asynchronous Streaming**: Zero-latency token rendering powered by Tokio channels and Ratatui.
- 🧹 **Reasoning Filter**: Automatically strips internal thought tags (`<think>...</think>`) on the fly during model output.
- 🎛️ **Model Switcher**: Dynamic popup menu (`Ctrl+M`) to query and select installed local models on demand.
- 📜 **Smooth Navigation**: Custom text-wrapping with vertical scrolling, page-up/down, and bottom-pinning during streaming.
- 🎨 **Minimalist Aesthetic**: Dark-mode palette optimized for long coding and chatting sessions.

## Prerequisites

- [Rust](https://www.rust-lang.org/) (2024 edition support)
- [Ollama](https://ollama.com/) running locally at `http://localhost:11434`

Ensure you have at least one model installed locally:

```bash
ollama pull llama3.2
