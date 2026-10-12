//! A native tsrs host with SDK-aware virtual configuration. No editor or Node dependency.
mod filesystem;
mod middleware;
mod transport;

pub use filesystem::SdkFileSystem;
use std::{
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tsrs_core::context::Context;
use tsrs_vfs::bundled;
pub use tsrs_vfs::{osvfs, FS};

/// Runs a complete LSP session on host-provided streams and filesystem.
/// The caller owns process lifecycle; EOF/shutdown/exit terminate the session.
pub fn run<R, W, E>(
    input: R,
    output: W,
    errors: E,
    cwd: &Path,
    fs: Arc<dyn tsrs_vfs::FS>,
) -> Result<(), String>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
    E: Write + Send + 'static,
{
    let fs = Arc::new(SdkFileSystem::new(fs, cwd.to_path_buf()));
    let state = Arc::new(Mutex::new(middleware::HostState::default()));
    let server_ref = Arc::new(OnceLock::new());
    let reader = transport::SdkReader::new(input, Arc::clone(&fs), Arc::clone(&state));
    let writer = middleware::SdkWriter::new(output, Arc::clone(&state), Arc::clone(&server_ref));
    let server = tsrs_lsp::new_server(tsrs_lsp::ServerOptions {
        in_: Box::new(reader),
        out: Box::new(writer),
        err: Box::new(errors),
        cwd: cwd.to_string_lossy().into_owned(),
        fs: Some(Arc::new(bundled::wrap_fs(fs))),
        default_library_path: bundled::lib_path(),
        typings_location: String::new(),
        parse_cache: None,
        npm_install: None,
        progress_delay: Duration::from_millis(250),
        set_parent_process_id: None,
    });
    let _ = server_ref.set(Arc::downgrade(&server));
    match server.run(&Context::background()) {
        // Closing tsrs's session can race the EOF/exit path and surface its internal
        // cancellation. Only an acknowledged shutdown makes that cancellation successful.
        Err(error)
            if error.is(tsrs_lsproto::ErrorTag::ContextCanceled)
                && state.lock().unwrap().shutdown_completed() =>
        {
            Ok(())
        }
        result => result.map_err(|e| e.to_string()),
    }
}

/// Convenience entry point for editors that launch the server over standard I/O.
pub fn run_stdio() -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    run(
        std::io::stdin(),
        std::io::stdout(),
        std::io::stderr(),
        &cwd,
        Arc::new(tsrs_vfs::osvfs::fs()),
    )
}
