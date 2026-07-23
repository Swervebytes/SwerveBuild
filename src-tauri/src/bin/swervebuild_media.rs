//! Media worker process (S24) — health shell only.
//!
//! Spawned by the app supervisor. Not for interactive use.
//! Usage: `swervebuild-media --token <uuid>`

fn main() {
    let code = swerve_build_lib::media_worker::worker_main(&std::env::args().collect::<Vec<_>>());
    std::process::exit(code);
}
