mod api;
mod application;
mod domain;
mod infrastructure;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    api::run().await
}
