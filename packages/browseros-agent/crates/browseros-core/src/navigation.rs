use crate::{CoreError, PageId, ProtocolSession, pages::PageManager, timeouts};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::time::sleep;

pub struct Navigation {
    pages: Arc<PageManager>,
    page_id: PageId,
}

impl Navigation {
    #[must_use]
    pub fn new(pages: Arc<PageManager>, page_id: PageId) -> Self {
        Self { pages, page_id }
    }

    pub async fn goto(&self, url: &str) -> Result<(), CoreError> {
        let page = self.pages.get_session(self.page_id.clone()).await?;
        let _: Value = page
            .session
            .send("Page.navigate", json!({ "url": url }))
            .await?;
        wait_for_load(&page.session).await
    }

    pub async fn reload(&self) -> Result<(), CoreError> {
        let page = self.pages.get_session(self.page_id.clone()).await?;
        let _: Value = page.session.send("Page.reload", json!({})).await?;
        wait_for_load(&page.session).await
    }

    pub async fn back(&self) -> Result<(), CoreError> {
        self.history("back").await
    }

    pub async fn forward(&self) -> Result<(), CoreError> {
        self.history("forward").await
    }

    async fn history(&self, direction: &str) -> Result<(), CoreError> {
        let page = self.pages.get_session(self.page_id.clone()).await?;
        let _: Value = page
            .session
            .send(
                "Runtime.evaluate",
                json!({ "expression": format!("history.{direction}()"), "awaitPromise": true }),
            )
            .await?;
        wait_for_load(&page.session).await
    }
}

#[derive(Debug, Deserialize)]
struct EvaluateResult {
    result: RemoteObject,
}

#[derive(Debug, Deserialize)]
struct RemoteObject {
    value: Option<Value>,
}

async fn wait_for_load(session: &ProtocolSession) -> Result<(), CoreError> {
    sleep(timeouts::WAIT_FOR_CONNECTION_POLL).await;
    let deadline = tokio::time::Instant::now() + timeouts::WAIT_FOR_LOAD_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        let result = session
            .send::<_, EvaluateResult>(
                "Runtime.evaluate",
                json!({ "expression": "document.readyState", "returnByValue": true }),
            )
            .await;
        if let Ok(result) = result
            && result.result.value.as_ref().and_then(Value::as_str) == Some("complete")
        {
            return Ok(());
        }
        sleep(timeouts::WAIT_FOR_LOAD_POLL).await;
    }
    Ok(())
}
