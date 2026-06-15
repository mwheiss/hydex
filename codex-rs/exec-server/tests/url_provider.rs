mod common;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use codex_exec_server::Environment;
use codex_exec_server::EnvironmentManager;
use codex_exec_server::ExecServerError;
use codex_exec_server::RemoteExecServerUrlProvider;
use codex_exec_server::RemoteExecServerUrlProviderFuture;
use common::exec_server::exec_server;
use pretty_assertions::assert_eq;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::time::Instant;
use tokio::time::sleep;

struct SequenceUrlProvider {
    remaining_urls: Mutex<VecDeque<String>>,
    returned_urls: Mutex<Vec<String>>,
}

impl SequenceUrlProvider {
    fn new(urls: Vec<String>) -> Self {
        Self {
            remaining_urls: Mutex::new(urls.into()),
            returned_urls: Mutex::new(Vec::new()),
        }
    }

    fn returned_urls(&self) -> Vec<String> {
        self.returned_urls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl RemoteExecServerUrlProvider for SequenceUrlProvider {
    fn websocket_url(&self) -> RemoteExecServerUrlProviderFuture<'_> {
        let result = self
            .remaining_urls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .ok_or_else(|| ExecServerError::Protocol("test URL provider exhausted".to_string()));
        if let Ok(url) = &result {
            self.returned_urls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(url.clone());
        }
        Box::pin(async move { result })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_environment_refreshes_url_after_unauthorized_handshake() -> anyhow::Result<()> {
    let unauthorized_listener = TcpListener::bind("127.0.0.1:0").await?;
    let unauthorized_url = format!("ws://{}", unauthorized_listener.local_addr()?);
    let unauthorized_server = tokio::spawn(async move {
        let (mut stream, _) = unauthorized_listener.accept().await?;
        {
            let mut lines = BufReader::new(&mut stream).lines();
            while let Some(line) = lines.next_line().await? {
                if line.is_empty() {
                    break;
                }
            }
        }
        stream
            .write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await?;
        stream.flush().await?;
        anyhow::Ok(())
    });
    let mut exec_server = exec_server().await?;
    let exec_server_url = exec_server.websocket_url().to_string();
    let provider = Arc::new(SequenceUrlProvider::new(vec![
        unauthorized_url.clone(),
        exec_server_url.clone(),
    ]));
    let manager = EnvironmentManager::without_environments();
    manager.upsert_environment_with_url_provider("remote".to_string(), provider.clone())?;
    let environment = manager
        .get_environment("remote")
        .expect("remote environment should be registered");

    let info = environment.info().await?;

    assert_eq!(info, Environment::default_for_tests().info().await?);
    assert_eq!(
        provider.returned_urls(),
        vec![unauthorized_url, exec_server_url]
    );
    unauthorized_server.await??;
    exec_server.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_environment_refreshes_url_after_disconnect() -> anyhow::Result<()> {
    let mut first_server = exec_server().await?;
    let mut second_server = exec_server().await?;
    let first_url = first_server.websocket_url().to_string();
    let second_url = second_server.websocket_url().to_string();
    let provider = Arc::new(SequenceUrlProvider::new(vec![
        first_url.clone(),
        second_url.clone(),
    ]));
    let manager = EnvironmentManager::without_environments();
    manager.upsert_environment_with_url_provider("remote".to_string(), provider.clone())?;
    let environment = manager
        .get_environment("remote")
        .expect("remote environment should be registered");

    assert_eq!(
        environment.info().await?,
        Environment::default_for_tests().info().await?
    );
    first_server.shutdown().await?;

    let deadline = Instant::now() + Duration::from_secs(5);
    let reconnected_info = loop {
        match environment.info().await {
            Ok(info) if provider.returned_urls().len() == 2 => break info,
            Ok(_) | Err(_) if Instant::now() < deadline => sleep(Duration::from_millis(25)).await,
            Ok(_) => anyhow::bail!("timed out waiting for the refreshed exec-server URL"),
            Err(error) => return Err(error.into()),
        }
    };

    assert_eq!(
        reconnected_info,
        Environment::default_for_tests().info().await?
    );
    assert_eq!(provider.returned_urls(), vec![first_url, second_url]);
    second_server.shutdown().await?;
    Ok(())
}
