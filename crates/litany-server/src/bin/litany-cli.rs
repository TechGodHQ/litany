#[tokio::main]
async fn main() -> anyhow::Result<()> {
    litany_server::run_cli().await
}
