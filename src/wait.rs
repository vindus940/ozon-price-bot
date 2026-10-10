use std::time::Duration;

use tokio::time::{Instant, sleep};

const INTERVAL: Duration = Duration::from_millis(500);

/// Повторяет проверку раз в полсекунды, пока она не станет истинной или не выйдет время.
pub async fn until<F, Fut>(timeout: Duration, mut condition: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if condition().await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(INTERVAL).await;
    }
}
