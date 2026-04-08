use async_rust_lsp::rules::mutex_across_await::check_mutex_across_await;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use tracing::{debug, info};

/// Shared document store: URI -> text content
type DocumentStore = Arc<RwLock<std::collections::HashMap<String, String>>>;

struct Backend {
    client: Client,
    documents: DocumentStore,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            documents: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Parse and publish diagnostics for a document.
    async fn analyze_document(&self, uri: Url, text: &str) {
        let diagnostics = check_mutex_across_await(text);

        debug!("Publishing {} diagnostic(s) for {}", diagnostics.len(), uri);

        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        info!("async-rust-lsp initialized");
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                // Code actions for future quick-fixes
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "async-rust-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        info!("Client acknowledged initialization");
    }

    async fn shutdown(&self) -> Result<()> {
        info!("async-rust-lsp shutting down");
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text_document.text.clone();

        // Only analyze Rust files
        if params.text_document.language_id != "rust" {
            return;
        }

        {
            let mut docs = self.documents.write().await;
            docs.insert(uri.to_string(), text.clone());
        }

        self.analyze_document(uri, &text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();

        // We request FULL sync, so there's always exactly one change with the full text.
        let text = match params.content_changes.into_iter().next() {
            Some(change) => change.text,
            None => return,
        };

        {
            let mut docs = self.documents.write().await;
            docs.insert(uri.to_string(), text.clone());
        }

        self.analyze_document(uri, &text).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri.clone();

        // Re-analyze on save in case text was provided
        if let Some(text) = params.text {
            self.analyze_document(uri, &text).await;
        } else {
            // Re-analyze with stored content
            let text = {
                let docs = self.documents.read().await;
                docs.get(&uri.to_string()).cloned()
            };
            if let Some(text) = text {
                self.analyze_document(uri, &text).await;
            }
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.clone();

        // Clear diagnostics on close
        {
            let mut docs = self.documents.write().await;
            docs.remove(&uri.to_string());
        }

        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        // Placeholder: future quick-fixes will be added here
        let _ = params;
        Ok(None)
    }
}

#[tokio::main]
async fn main() {
    // Log to a file so we don't pollute stdio (LSP uses stdio for protocol messages)
    let log_file = tracing_appender::rolling::never(std::env::temp_dir(), "async-rust-lsp.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(log_file);
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("async_rust_lsp=debug".parse().unwrap()),
        )
        .init();

    info!("Starting async-rust-lsp v{}", env!("CARGO_PKG_VERSION"));

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
