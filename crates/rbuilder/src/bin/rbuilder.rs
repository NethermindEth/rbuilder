//! Instantiation of cli::run on our sample configuration.
//! This runs the default included rbuilder!

use rbuilder::{
    live_builder::{cli, config::Config},
    utils::build_info::print_version_info,
};
use tokio::runtime::Builder;

fn main() -> eyre::Result<()> {
    let runtime = Builder::new_multi_thread()
        .worker_threads(32)
        .max_blocking_threads(8096)
        .enable_all()
        .build()
        .expect("Failed to create runtime");

    runtime.block_on(async { cli::run::<Config>(print_version_info, None).await })
}
