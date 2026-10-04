/// Events produced by background tasks and sent to the main loop.
#[derive(Debug)]
pub enum AppEvent {
    /// A streamed token from the model
    StreamToken(String),
    /// The model finished generating
    StreamDone,
    /// An error occurred during generation
    StreamError(String),
    /// The user cancelled the stream mid-flight (Esc)
    StreamCancelled,
    /// Ollama responded with the installed model list
    ModelsLoaded(Vec<String>),
    /// Could not reach the Ollama daemon at all
    ConnectionError(String),
}
