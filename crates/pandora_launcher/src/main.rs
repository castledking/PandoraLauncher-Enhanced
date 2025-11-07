#![deny(unused_must_use)]

use std::{collections::HashMap, sync::Arc, time::Instant};

use bridge::handle::{BackendHandle, FrontendHandle};
pub mod panic;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let panic_message = Default::default();

    crate::panic::install_hook(Arc::clone(&panic_message));
    
    let (frontend_send, frontend_recv) = tokio::sync::mpsc::channel(64);
    let (backend_send, backend_recv) = tokio::sync::mpsc::channel(64);
    
    let backend_handle = BackendHandle::from(backend_send);
    
    backend::start(FrontendHandle::from(frontend_send), backend_handle.clone(), backend_recv);
    frontend::start(panic_message, backend_handle.clone(), frontend_recv);
}
